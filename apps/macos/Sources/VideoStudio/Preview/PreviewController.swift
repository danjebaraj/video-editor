import AppKit
import WebKit
import Quartz
import AVFoundation
import MarkdownCore

/// Read-only rendered preview of the spec (WKWebView with the bundled markdown-it, KaTeX,
/// Mermaid and highlight.js; everything renders offline). `![label](media:<id>)` shows as a
/// media card with a thumbnail, duration, kind and status (spec §5).
@MainActor
final class PreviewController: NSObject, WKScriptMessageHandler, WKNavigationDelegate, QLPreviewPanelDataSource {
    let webView: WKWebView
    unowned let document: ProjectDocument
    private var ready = false
    private var pending: (() -> Void)?
    private(set) var lastTopLine = 0
    private var quickLookURL: URL?
    private var renderWork: DispatchWorkItem?
    var onRendered: (() -> Void)?
    let media: MediaSchemeHandler

    static var resourcesURL: URL { Bundle.main.resourceURL!.appendingPathComponent("Preview", isDirectory: true) }

    init(document: ProjectDocument) {
        self.document = document
        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "allowFileAccessFromFileURLs")
        let controller = WKUserContentController()
        // Forward JS errors to the app log.
        controller.addUserScript(WKUserScript(source: """
            window.addEventListener('error', function (e) { try { window.webkit.messageHandlers.md.postMessage({type: 'jserror', message: String(e.message) + ' @' + e.filename + ':' + e.lineno}); } catch (_) {} });
            window.addEventListener('unhandledrejection', function (e) { try { window.webkit.messageHandlers.md.postMessage({type: 'jserror', message: 'unhandled: ' + String(e.reason)}); } catch (_) {} });
            """, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        config.userContentController = controller
        media = MediaSchemeHandler.install(on: config)
        webView = WKWebView(frame: .zero, configuration: config)
        super.init()
        media.document = document
        controller.add(WeakMessageHandler(self), name: "md")
        webView.navigationDelegate = self
        webView.setValue(false, forKey: "drawsBackground")
        Self.disableOcclusionThrottling(webView)
        #if DEBUG
        if webView.responds(to: Selector(("setInspectable:"))) { webView.isInspectable = true }
        #endif
        webView.loadFileURL(Self.resourcesURL.appendingPathComponent("index.html"), allowingReadAccessTo: Self.resourcesURL)
    }

    /// Keeps WebKit painting when the window is covered by other windows.
    static func disableOcclusionThrottling(_ webView: WKWebView) {
        let sel = NSSelectorFromString("_setWindowOcclusionDetectionEnabled:")
        if webView.responds(to: sel) { webView.perform(sel, with: false) }
    }

    // MARK: Rendering

    struct MediaInfo: Encodable {
        var kind: String
        var name: String
        /// `ok`, `moved`, `changed` or `missing`
        var status: String
        var meta: String
        var duration: Double?
        /// Thumbnails available for hover scrubbing (1 for stills).
        var frames: Int
    }

    struct Payload: Encodable {
        var text: String
        var theme: String
        var showFrontMatter: Bool
        var customCSS: String
        var scrollLine: Int?
        var remoteImages: Bool
        var islandOutlines: Bool
        var media: [String: MediaInfo]
    }

    static func theme(for appearance: NSAppearance?) -> String {
        switch AppSettings.shared.previewTheme {
        case "light": return "light"
        case "dark": return "dark"
        default:
            let app = appearance ?? NSApp.effectiveAppearance
            return app.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? "dark" : "light"
        }
    }

    static func describe(_ item: MediaItem) -> String {
        var bits: [String] = [MediaItem.kinds.first { $0.key == item.kind }?.label.replacingOccurrences(of: "s$", with: "", options: .regularExpression) ?? item.kind.capitalized]
        if let d = item.probe?.duration_s, d > 0 { bits.append(Fmt.duration(d)) }
        if let w = item.probe?.width, let h = item.probe?.height { bits.append("\(w)×\(h)") }
        return bits.joined(separator: " · ")
    }

    func payload(text: String, scrollLine: Int?) -> Payload {
        var css = ""
        let path = AppSettings.shared.customCSSPath
        if !path.isEmpty { css = (try? String(contentsOfFile: (path as NSString).expandingTildeInPath, encoding: .utf8)) ?? "" }
        var info: [String: MediaInfo] = [:]
        for item in document.project.media {
            let duration = item.probe?.duration_s
            let frames = item.kind == "video" && (duration ?? 0) > 1 ? MediaSchemeHandler.scrubFrames : 1
            info[item.id] = MediaInfo(kind: item.kind, name: item.fileName, status: document.mediaStatus(item.id), meta: Self.describe(item), duration: duration, frames: frames)
        }
        return Payload(text: text, theme: Self.theme(for: webView.effectiveAppearance), showFrontMatter: AppSettings.shared.showFrontMatter, customCSS: css, scrollLine: scrollLine,
                       remoteImages: AppSettings.shared.loadRemoteImages, islandOutlines: AppSettings.shared.showIslandOutlines, media: info)
    }

    /// Renders shortly after edits stop, or right away.
    func scheduleRender(immediate: Bool = false) {
        renderWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.render(text: self.document.editor.text, scrollLine: immediate ? self.document.editor.topVisibleLine() : nil)
        }
        renderWork = work
        if immediate { work.perform() } else { DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: work) }
    }

    func render(text: String, scrollLine: Int?) {
        let p = payload(text: text, scrollLine: scrollLine)
        run {
            guard let data = try? JSONEncoder().encode(p), let json = String(data: data, encoding: .utf8) else { return }
            self.webView.evaluateJavaScript("MD.render(\(json))") { _, error in
                if let error { NSLog("Preview render error: %@", String(describing: error)) }
            }
        }
    }

    func scrollToLine(_ line: Int) {
        run { self.webView.evaluateJavaScript("MD.scrollToLine(\(line), false)") }
    }

    func topLine(_ completion: @escaping (Int) -> Void) {
        guard ready else { completion(lastTopLine); return }
        webView.evaluateJavaScript("MD.topLine()") { result, _ in
            completion((result as? Int) ?? (result as? NSNumber)?.intValue ?? self.lastTopLine)
        }
    }

    /// Evaluates JavaScript in the preview once it is ready (used by the self test).
    func evaluate(_ js: String, completion: @escaping (Any?) -> Void) {
        run { self.webView.evaluateJavaScript(js) { result, _ in completion(result) } }
    }

    private func run(_ block: @escaping () -> Void) {
        if ready { block() } else { pending = block }
    }

    // MARK: Messages from JS

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let body = message.body as? [String: Any], let type = body["type"] as? String else { return }
        switch type {
        case "ready":
            ready = true
            pending?()
            pending = nil
        case "scroll":
            lastTopLine = body["line"] as? Int ?? lastTopLine
        case "rendered":
            onRendered?()
        case "jserror":
            NSLog("Preview JS error: %@", body["message"] as? String ?? "?")
        case "link":
            handleLink(body["href"] as? String ?? "")
        case "task":
            if let line = body["line"] as? Int { toggleTask(line: line) }
        case "media":
            if let id = body["id"] as? String { openMedia(id) }
        default: break
        }
    }

    /// Clicking a media card plays it in Quick Look; a missing item opens the relink sheet.
    func openMedia(_ id: String) {
        if let url = document.mediaURL(id), FileManager.default.fileExists(atPath: url.path) {
            quickLook(url)
        } else {
            document.reveal(FixLink(kind: "media", target: id, line: nil))
        }
    }

    /// A checkbox clicked in the preview toggles that task item in the spec (an undoable edit).
    func toggleTask(line: Int) {
        let ed = document.editor
        guard ed.textView.isEditable, line < ed.scan.lineStarts.count else { NSSound.beep(); return }
        let range = ed.lineRangeChars(line...line)
        let text = ed.nsText.substring(with: range) as NSString
        let box = text.range(of: "[", options: [])
        guard box.location != NSNotFound, ed.handleTodoClick(at: range.location + box.location + 1) else { NSSound.beep(); return }
        scheduleRender()
    }

    private func handleLink(_ href: String) {
        guard !href.isEmpty else { return }
        if let url = URL(string: href), let scheme = url.scheme?.lowercased(), ["http", "https", "mailto"].contains(scheme) {
            NSWorkspace.shared.open(url)
        } else if href.hasPrefix(MediaLink.prefix) {
            openMedia(String(href.dropFirst(MediaLink.prefix.count)))
        }
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        NSLog("Preview navigation failed: %@", error.localizedDescription)
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        NSLog("Preview load failed: %@", error.localizedDescription)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        // Fallback in case the DOMContentLoaded message was missed.
        webView.evaluateJavaScript("typeof MD") { result, _ in
            if (result as? String) == "object", !self.ready {
                self.ready = true
                self.pending?()
                self.pending = nil
            } else if (result as? String) != "object" {
                NSLog("Preview: MD object missing after load (%@)", String(describing: result))
            }
        }
    }

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction, decisionHandler: @escaping @MainActor (WKNavigationActionPolicy) -> Void) {
        // Only our own page loads; everything else is handled through JS click messages.
        if navigationAction.navigationType == .other, navigationAction.request.url?.lastPathComponent == "index.html" {
            decisionHandler(.allow)
        } else if navigationAction.navigationType == .linkActivated, let url = navigationAction.request.url {
            decisionHandler(.cancel)
            if url.scheme == "http" || url.scheme == "https" { NSWorkspace.shared.open(url) }
        } else {
            decisionHandler(navigationAction.request.url?.isFileURL == true ? .allow : .cancel)
        }
    }

    // MARK: Quick Look

    func quickLook(_ url: URL) {
        quickLookURL = url
        guard let panel = QLPreviewPanel.shared() else { return }
        panel.dataSource = self
        panel.reloadData()
        panel.makeKeyAndOrderFront(nil)
    }

    nonisolated func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { 1 }

    nonisolated func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> (any QLPreviewItem)! {
        MainActor.assumeIsolated { (quickLookURL ?? URL(fileURLWithPath: "/")) as NSURL }
    }
}

/// Avoids a retain cycle between WKUserContentController and its handler.
final class WeakMessageHandler: NSObject, WKScriptMessageHandler {
    weak var target: WKScriptMessageHandler?
    init(_ target: WKScriptMessageHandler) { self.target = target }
    func userContentController(_ c: WKUserContentController, didReceive message: WKScriptMessage) {
        target?.userContentController(c, didReceive: message)
    }
}

/// Serves pictures to the preview through app-owned URL schemes, so the page's content security
/// policy can allow only these:
/// - `vs-media://thumb/<id>/<n>`: a picture of a media item — the image itself, or frame `n` of
///   `scrubFrames` across a video. Only items in the project are served.
/// - `mdraft-remote://fetch/<percent-encoded URL>`: a web image, only while "Load remote images" is on.
@MainActor
final class MediaSchemeHandler: NSObject, WKURLSchemeHandler {
    static let mediaScheme = "vs-media"
    static let remoteScheme = "mdraft-remote"
    static let scrubFrames = 12

    weak var document: ProjectDocument?
    private var stopped = Set<ObjectIdentifier>()
    private static let session = URLSession(configuration: .ephemeral)
    private static let cache = NSCache<NSString, NSData>()

    static func install(on config: WKWebViewConfiguration) -> MediaSchemeHandler {
        let handler = MediaSchemeHandler()
        config.setURLSchemeHandler(handler, forURLScheme: mediaScheme)
        config.setURLSchemeHandler(handler, forURLScheme: remoteScheme)
        config.websiteDataStore = .nonPersistent()
        return handler
    }

    func webView(_ webView: WKWebView, start task: any WKURLSchemeTask) {
        guard let url = task.request.url else { return fail(task, "no URL") }
        if url.scheme == Self.remoteScheme { return fetchRemote(url, task) }
        let parts = url.path(percentEncoded: false).split(separator: "/").map(String.init)
        guard url.host == "thumb", let id = parts.first, let item = document?.project.mediaItem(id), let file = document?.mediaURL(id) else {
            return fail(task, "not a project media item: \(url.absoluteString)")
        }
        let index = parts.count > 1 ? Int(parts[1]) ?? 0 : 0
        let key = "\(file.path)|\(item.fingerprint?.quick_hash ?? "")|\(index)" as NSString
        if let hit = Self.cache.object(forKey: key) { return respond(task, url: url, data: hit as Data, mime: "image/jpeg") }
        let kind = item.kind
        let duration = item.probe?.duration_s ?? 0
        DispatchQueue.global(qos: .userInitiated).async {
            let data = Self.picture(of: file, kind: kind, duration: duration, index: index)
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                guard let data else { return self.fail(task, "no picture for \(id)") }
                Self.cache.setObject(data as NSData, forKey: key)
                self.respond(task, url: url, data: data, mime: "image/jpeg")
            }
        }
    }

    func webView(_ webView: WKWebView, stop task: any WKURLSchemeTask) {
        stopped.insert(ObjectIdentifier(task))
    }

    /// A JPEG of the item at most 640 px wide: the image, or a video frame.
    nonisolated static func picture(of file: URL, kind: String, duration: Double, index: Int) -> Data? {
        var cg: CGImage?
        if kind == "video" {
            let gen = AVAssetImageGenerator(asset: AVURLAsset(url: file))
            gen.appliesPreferredTrackTransform = true
            gen.maximumSize = CGSize(width: 640, height: 640)
            let t = duration > 1 ? duration * (Double(index) + 0.5) / Double(scrubFrames) : 0
            cg = try? gen.copyCGImage(at: CMTime(seconds: t, preferredTimescale: 600), actualTime: nil)
        } else if kind == "image", let src = CGImageSourceCreateWithURL(file as CFURL, nil) {
            let opts: [CFString: Any] = [kCGImageSourceCreateThumbnailFromImageAlways: true, kCGImageSourceCreateThumbnailWithTransform: true, kCGImageSourceThumbnailMaxPixelSize: 1280]
            cg = CGImageSourceCreateThumbnailAtIndex(src, 0, opts as CFDictionary)
        }
        guard let cg else { return nil }
        return NSBitmapImageRep(cgImage: cg).representation(using: .jpeg, properties: [.compressionFactor: 0.8])
    }

    private func fetchRemote(_ url: URL, _ task: any WKURLSchemeTask) {
        guard AppSettings.shared.loadRemoteImages, url.host == "fetch", url.query == nil, url.fragment == nil,
              let raw = String(url.path(percentEncoded: true).drop { $0 == "/" }).removingPercentEncoding,
              let remote = URL(string: raw), ["http", "https"].contains(remote.scheme?.lowercased() ?? "") else {
            return fail(task, "remote images are off")
        }
        Self.session.dataTask(with: remote) { data, _, error in
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                guard let data, error == nil, let sniffed = ImageIngest.sniff(data) else { return self.fail(task, "download failed: \(error?.localizedDescription ?? "not an image")") }
                self.respond(task, url: url, data: data, mime: sniffed.mime)
            }
        }.resume()
    }

    private func respond(_ task: any WKURLSchemeTask, url: URL, data: Data, mime: String) {
        guard !stopped.contains(ObjectIdentifier(task)) else { return }
        task.didReceive(URLResponse(url: url, mimeType: mime, expectedContentLength: data.count, textEncodingName: nil))
        task.didReceive(data)
        task.didFinish()
    }

    private func fail(_ task: any WKURLSchemeTask, _ reason: String) {
        guard !stopped.contains(ObjectIdentifier(task)) else { return }
        task.didFailWithError(URLError(.resourceUnavailable))
    }
}
