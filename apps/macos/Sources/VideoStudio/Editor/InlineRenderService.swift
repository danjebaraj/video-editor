import AppKit
import WebKit
import MarkdownCore

/// Renders Mermaid diagrams and math blocks to images for the editor's inline display, using the same
/// Mermaid and KaTeX as the preview in a hidden web view. One item renders at a time; results are cached.
@MainActor
final class InlineRenderService: NSObject, WKNavigationDelegate {
    static let shared = InlineRenderService()

    enum Kind: String { case mermaid, math }
    struct Key: Hashable { var kind: Kind; var source: String; var dark: Bool }

    private var cache: [Key: NSImage] = [:]
    private var cacheOrder: [Key] = []
    private var waiting: [Key: [(NSImage?) -> Void]] = [:]
    private var queue: [Key] = []
    private var busy = false
    private var loaded = false
    private lazy var webView: WKWebView = makeWebView()
    private var window: NSWindow?
    static let maxCached = 200

    func cached(_ key: Key) -> NSImage? { cache[key] }

    /// Calls `done` on the main actor with the rendered image (nil if rendering failed).
    func render(_ key: Key, done: @escaping (NSImage?) -> Void) {
        if let img = cache[key] { done(img); return }
        if waiting[key] != nil { waiting[key]!.append(done); return }
        waiting[key] = [done]
        queue.append(key)
        pump()
    }

    private func makeWebView() -> WKWebView {
        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "allowFileAccessFromFileURLs")
        let wv = WKWebView(frame: NSRect(x: 0, y: 0, width: 1200, height: 900), configuration: config)
        wv.setValue(false, forKey: "drawsBackground")
        let w = NSWindow(contentRect: NSRect(x: -12000, y: -12000, width: 1200, height: 900), styleMask: [.borderless], backing: .buffered, defer: false)
        w.contentView = wv
        w.isReleasedWhenClosed = false
        PreviewController.disableOcclusionThrottling(wv)
        w.orderBack(nil)
        w.alphaValue = 0
        window = w
        wv.navigationDelegate = self
        wv.loadFileURL(PreviewController.resourcesURL.appendingPathComponent("inline.html"), allowingReadAccessTo: PreviewController.resourcesURL)
        return wv
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        loaded = true
        pump()
    }

    private func pump() {
        _ = webView
        guard loaded, !busy, !queue.isEmpty else { return }
        busy = true
        let key = queue.removeFirst()
        Task { @MainActor in
            let image = await self.renderNow(key)
            if let image { self.store(image, for: key) }
            let callbacks = self.waiting.removeValue(forKey: key) ?? []
            self.busy = false
            for cb in callbacks { cb(image) }
            self.pump()
        }
    }

    private func renderNow(_ key: Key) async -> NSImage? {
        webView.appearance = NSAppearance(named: key.dark ? .darkAqua : .aqua)
        let result: Any?
        do {
            result = try await webView.callAsyncJavaScript("return await window.Inline.render(kind, src, dark)",
                                                           arguments: ["kind": key.kind.rawValue, "src": key.source, "dark": key.dark],
                                                           contentWorld: .page)
        } catch {
            return nil
        }
        guard let size = result as? [String: Any], let w = (size["w"] as? NSNumber)?.doubleValue, let h = (size["h"] as? NSNumber)?.doubleValue,
              w > 0, h > 0 else { return nil }
        let config = WKSnapshotConfiguration()
        config.rect = NSRect(x: 0, y: 0, width: min(w, 1200), height: min(h, 4000))
        if h > webView.frame.height { webView.setFrameSize(NSSize(width: 1200, height: min(h + 20, 4000))) }
        config.afterScreenUpdates = true
        return try? await webView.takeSnapshot(configuration: config)
    }

    private func store(_ image: NSImage, for key: Key) {
        cache[key] = image
        cacheOrder.append(key)
        if cacheOrder.count > Self.maxCached { cache.removeValue(forKey: cacheOrder.removeFirst()) }
    }
}
