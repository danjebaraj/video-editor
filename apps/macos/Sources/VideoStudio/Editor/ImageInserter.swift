import AppKit
import ImageIO
import UniformTypeIdentifiers
import MarkdownCore

/// Overrides used by the in-app self test.
@MainActor
enum TestHooks {
    static var autoConvertPaste: Bool?
    /// Called with each image the paste/drop reader hands to the pipeline, before it is processed.
    static var onImageInput: (([ImageInput]) -> Void)?
    /// Answers the "update @old-name in your unsent message?" question.
    static var autoUpdateMentions: Bool?
}

/// An image arriving from a paste, a drop or the file picker.
enum ImageInput: Sendable, Equatable {
    case file(URL)
    case data(Data, name: String)

    var label: String {
        switch self {
        case .file(let url): return url.lastPathComponent
        case .data(_, let name): return name
        }
    }

    /// The bytes and source name. Files are size-checked before they are read.
    func load(maxBytes: Int) throws -> (Data, String) {
        switch self {
        case .data(let data, let name):
            guard data.count <= maxBytes else { throw ImageIngestError.tooLarge }
            return (data, name)
        case .file(let url):
            let v = try url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
            guard v.isRegularFile == true else { throw ImageIngestError.invalid }
            guard (v.fileSize ?? 0) <= maxBytes else { throw ImageIngestError.tooLarge }
            return (try Data(contentsOf: url, options: .mappedIfSafe), url.lastPathComponent)
        }
    }

    // MARK: Reading the clipboard or a drop

    /// Image types the pasteboard can hold, most specific first in the pasteboard's own order.
    static var readableImageTypes: Set<String> {
        Set(NSImage.imageTypes + [UTType.png.identifier, UTType.tiff.identifier])
    }

    /// The images on a pasteboard (spec 5.2), or nil when it holds none and normal paste/drop applies:
    /// 1. file URLs (Live Photo packages give their still, paired movies are skipped; Markdown files and
    ///    folders are left to the normal handling), else 2. the first decodable image type, named
    ///    `pasted-image.<ext>`. Image data that comes with rich text (a copy from Word or Excel) is text.
    static func read(_ pb: NSPasteboard) -> [ImageInput]? {
        if let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], !urls.isEmpty {
            let files = ImageSources.files(from: urls).filter { url in
                !FileIndex.isMarkdown(url) && (try? url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) != true
            }
            return files.isEmpty ? nil : files.map { .file($0) }
        }
        let types = pb.types ?? []
        guard !hasRichText(types) else { return nil }
        let readable = readableImageTypes
        for type in types where readable.contains(type.rawValue) {
            guard let data = pb.data(forType: type), ImageIngest.sniff(data) != nil else { continue }
            let ext = UTType(type.rawValue)?.preferredFilenameExtension ?? "png"
            return [.data(data, name: "pasted-image.\(ext)")]
        }
        return nil
    }

    private static func hasRichText(_ types: [NSPasteboard.PasteboardType]) -> Bool {
        types.contains(.string) && (types.contains(.rtf) || types.contains(.html) || types.contains(.rtfd))
    }

    /// A cheap check from the types alone (no data read), for Paste validation and drag feedback.
    static func canRead(_ pb: NSPasteboard) -> Bool {
        if pb.canReadObject(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) { return true }
        let types = pb.types ?? []
        let readable = readableImageTypes
        return !hasRichText(types) && types.contains { readable.contains($0.rawValue) }
    }
}

/// Paste handling, image ingest into the image folder, and link/image helpers.
extension EditorController {
    struct ImageRef {
        var range: NSRange
        var pathRange: NSRange
        var raw: String
        var alt: String
        var isTag: Bool
        var url: URL?
        var isRemote: Bool
    }

    // MARK: Paste

    func handlePaste(_ pb: NSPasteboard) -> Bool {
        // 1–2. Image files or image data.
        if let images = ImageInput.read(pb) {
            ingestImages(images)
            return true
        }
        // 3. HTML with images or lists (Teams, Outlook, Loop, browsers): plain text would lose the images and the
        //    list markers and nesting, so convert the HTML to Markdown and store each embedded image in Assets.
        if let html = pb.string(forType: .html), HTMLMarkdown.hasImages(html) || HTMLMarkdown.hasLists(html),
           !isInCodeBlock(textView.selectedRange().location) {
            pasteHTMLWithImages(html)
            return true
        }
        guard let string = pb.string(forType: .string), !string.isEmpty else { return false }
        let sel = textView.selectedRange()
        let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
        // 4. URL over a selection → link.
        if sel.length > 0, trimmed.range(of: "^(https?|mailto|ftp)://?\\S+$", options: .regularExpression) != nil || trimmed.hasPrefix("mailto:") {
            let label = nsText.substring(with: sel)
            if !label.contains("\n") {
                let rep = "[\(label)](\(trimmed))"
                apply(TextEdit(range: sel, replacement: rep, selection: NSRange(location: sel.location + (rep as NSString).length, length: 0)), actionName: "Paste Link")
                return true
            }
        }
        guard !isInCodeBlock(sel.location) else { return false }
        // 5. Tabular data → offer a Markdown table.
        if let sep = MarkdownTable.detectDelimiter(trimmed), sep == "\t" || trimmed.components(separatedBy: "\n").count >= 2,
           let table = MarkdownTable.fromDelimited(trimmed, separator: sep), table.columnCount >= 2 {
            if askConvert(kind: sep == "\t" ? "tab-separated data" : "CSV data") {
                apply(MarkdownEditing.insertBlock(nsText, selection: sel, block: table.formatted()), actionName: "Paste Table")
                return true
            }
            return false
        }
        if let html = pb.string(forType: .html), let rows = HTMLTableParser.rows(from: html), rows.count >= 1, let header = rows.first {
            if askConvert(kind: "an HTML table") {
                var t = MarkdownTable(header: header, rows: Array(rows.dropFirst()))
                t.layoutWidth = tableLayoutWidth
                apply(MarkdownEditing.insertBlock(nsText, selection: sel, block: t.formatted()), actionName: "Paste Table")
                return true
            }
        }
        return false
    }

    private func askConvert(kind: String) -> Bool {
        if let auto = TestHooks.autoConvertPaste { return auto }
        let alert = NSAlert()
        alert.messageText = "Convert to a Markdown table?"
        alert.informativeText = "The clipboard holds \(kind)."
        alert.addButton(withTitle: "Convert to Table")
        alert.addButton(withTitle: "Paste as Text")
        return alert.runModal() == .alertFirstButtonReturn
    }

    // MARK: Inserting images

    func insertImageFiles(_ urls: [URL]) {
        insertMediaFiles(urls)
    }

    /// Adds files to the project by reference and inserts one `![name](media:<id>)` per file at
    /// the caret as one undo step. Nothing is copied (spec §5, §6).
    func insertMediaFiles(_ urls: [URL]) {
        guard !urls.isEmpty, textView.isEditable else { return }
        let ids = document.addMedia(urls)
        let blocks = ids.compactMap { id -> String? in
            guard let item = document.project.mediaItem(id) else { return nil }
            return MediaLink.markdown(id: id, alt: item.fileName) + "\n"
        }
        finishIngest(blocks: blocks, errors: ids.isEmpty ? ["Those files couldn't be added."] : [])
    }

    /// Images arriving from a paste or drop. A file becomes a media reference. Clipboard data
    /// has no source file, so it is stored in the project's `.assets` sidecar and referenced
    /// from there: the one case the app writes media (spec §5).
    func ingestImages(_ inputs: [ImageInput]) {
        TestHooks.onImageInput?(inputs)
        guard !inputs.isEmpty, textView.isEditable else { return }
        var blocks: [String] = []
        var errors: [String] = []
        for input in inputs {
            switch input {
            case .file(let url):
                if let id = document.addMedia([url]).first, let item = document.project.mediaItem(id) {
                    blocks.append(MediaLink.markdown(id: id, alt: item.fileName) + "\n")
                } else {
                    errors.append("\(input.label) couldn't be added.")
                }
            case .data(let data, let name):
                if let id = document.addPastedImage(data) {
                    blocks.append(MediaLink.markdown(id: id, alt: name) + "\n")
                } else {
                    errors.append("The pasted image couldn't be stored.")
                }
            }
        }
        status.busyCount += 1
        finishIngest(blocks: blocks, errors: errors)
    }

    /// Rich paste of HTML that holds images: `data:` images go to the sidecar, `file:` images
    /// become media references, web images stay links; others (`blob:`, `cid:`) can't be read
    /// from the clipboard and are reported.
    func pasteHTMLWithImages(_ html: String) {
        guard textView.isEditable else { NSSound.beep(); return }
        let result = HTMLMarkdown.convert(html, tableWidth: tableLayoutWidth)
        var links: [String] = []
        var errors: [String] = []
        for img in result.images {
            let alt = img.alt.replacingOccurrences(of: "[\\[\\]\n]", with: "", options: .regularExpression)
            if let (data, _) = HTMLMarkdown.decodeDataURL(img.src) {
                if let id = document.addPastedImage(data) { links.append(MediaLink.markdown(id: id, alt: alt.isEmpty ? "pasted image" : alt)) }
                else { links.append(""); errors.append("A pasted image couldn't be stored.") }
            } else if let url = URL(string: img.src), url.isFileURL {
                if let id = document.addMedia([url]).first { links.append(MediaLink.markdown(id: id, alt: alt.isEmpty ? url.lastPathComponent : alt)) }
                else { links.append(""); errors.append("\(url.lastPathComponent) couldn't be added.") }
            } else if let url = URL(string: img.src), ["http", "https"].contains(url.scheme?.lowercased() ?? "") {
                links.append("![\(alt)](\(img.src.contains(" ") ? "<\(img.src)>" : img.src))")
            } else {
                links.append("")
                errors.append("An image couldn't be pasted: the app it came from didn't include its data.")
            }
        }
        status.busyCount += 1
        finishHTMLPaste(HTMLMarkdown.fill(result.markdown, with: links), errors: errors)
    }

    private func finishHTMLPaste(_ markdown: String, errors: [String]) {
        status.busyCount -= 1
        if !errors.isEmpty { status.show(Array(Set(errors)).sorted().joined(separator: "  "), seconds: 8) }
        let md = markdown.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !md.isEmpty, textView.isEditable else { return }
        let sel = textView.selectedRange()
        if md.contains("\n") {
            apply(MarkdownEditing.insertBlock(nsText, selection: sel, block: md), actionName: "Paste")
        } else {
            apply(TextEdit(range: sel, replacement: md, selection: NSRange(location: sel.location + (md as NSString).length, length: 0)), actionName: "Paste")
        }
        invalidateImageCache()
    }

    private func finishIngest(blocks: [String], errors: [String]) {
        status.busyCount -= 1
        if !errors.isEmpty { status.show(errors.joined(separator: "  "), seconds: 8) }
        guard !blocks.isEmpty else { return }
        let sel = textView.selectedRange()
        // Each image is its own block: start on a fresh line.
        let ns = nsText
        let atLineStart = sel.location == 0 || ns.character(at: sel.location - 1) == 10
        let rep = (atLineStart ? "" : "\n") + blocks.joined()
        apply(TextEdit(range: sel, replacement: rep, selection: NSRange(location: sel.location + (rep as NSString).length, length: 0)),
              actionName: blocks.count > 1 ? "Insert Images" : "Insert Image")
        invalidateImageCache()
    }

    /// `![](https://)` with the caret in the brackets, or the selection as alt text with `https://` selected.
    func insertImageLink() {
        let sel = textView.selectedRange()
        let t = ImageReference.linkTemplate(selection: nsText.substring(with: sel))
        apply(TextEdit(range: sel, replacement: t.text, selection: NSRange(location: sel.location + t.selection.location, length: t.selection.length)),
              actionName: "Image Link")
    }

    /// Replaces the whole text with `new` as one undo step, touching only the part that changed.
    func replaceText(with new: String, actionName: String) {
        let old = nsText, n = new as NSString
        guard old as String != new else { return }
        var start = 0
        while start < old.length, start < n.length, old.character(at: start) == n.character(at: start) { start += 1 }
        var oldEnd = old.length, newEnd = n.length
        while oldEnd > start, newEnd > start, old.character(at: oldEnd - 1) == n.character(at: newEnd - 1) { oldEnd -= 1; newEnd -= 1 }
        let sel = textView.selectedRange()
        let delta = newEnd - oldEnd
        let caret = sel.location >= oldEnd ? sel.location + delta : min(sel.location, start)
        apply(TextEdit(range: NSRange(location: start, length: oldEnd - start), replacement: n.substring(with: NSRange(location: start, length: newEnd - start)),
                       selection: NSRange(location: max(0, caret), length: 0)), actionName: actionName)
    }

    // MARK: Remove image, content-hash names

    /// Removes every reference to the image at `idx` (outside code), leaving the file in place.
    func removeImage(at idx: Int) {
        guard let ref = imageReference(at: idx) else { return }
        let path = ImageNaming.decodeMarkdownPath(ref.raw)
        replaceText(with: ImageReferenceScanner.removingReferences(to: path, in: text), actionName: "Remove Image")
    }

    /// A file URL with symbolic links resolved, also for a file that no longer exists.
    nonisolated static func canonical(_ url: URL) -> URL { URL(fileURLWithPath: FileIndex.canonicalPath(url)) }

    // MARK: Image references in source

    private static let mdImageRx = try! NSRegularExpression(pattern: "!\\[([^\\]\\n]*)\\]\\(([^)\\n]*)\\)")
    private static let tagImageRx = try! NSRegularExpression(pattern: "<img\\s[^>]*?src=\"([^\"]+)\"[^>]*>", options: .caseInsensitive)

    func imageReference(at idx: Int) -> ImageRef? {
        guard idx >= 0, idx <= nsText.length else { return nil }
        let lr = nsText.lineRange(for: NSRange(location: min(idx, max(nsText.length - 1, 0)), length: 0))
        for m in Self.mdImageRx.matches(in: text, range: lr) where NSLocationInRange(idx, m.range) {
            return makeRef(range: m.range, pathRange: m.range(at: 2), alt: nsText.substring(with: m.range(at: 1)), isTag: false)
        }
        for m in Self.tagImageRx.matches(in: text, range: lr) where NSLocationInRange(idx, m.range) {
            let tag = nsText.substring(with: m.range)
            let alt = tag.range(of: "alt=\"([^\"]*)\"", options: .regularExpression).map { String(tag[$0].dropFirst(5).dropLast()) } ?? ""
            return makeRef(range: m.range, pathRange: m.range(at: 1), alt: alt, isTag: true)
        }
        return nil
    }

    private func makeRef(range: NSRange, pathRange: NSRange, alt: String, isTag: Bool) -> ImageRef {
        let raw = nsText.substring(with: pathRange)
        let path = ImageNaming.decodeMarkdownPath(raw)
        let remote = path.hasPrefix("http://") || path.hasPrefix("https://")
        var url: URL?
        if remote { url = nil }
        else if path.hasPrefix("/") { url = URL(fileURLWithPath: path) }
        else if path.hasPrefix("file://") { url = URL(string: path) }
        else if path.hasPrefix(MediaLink.prefix) { url = document.mediaURL(String(path.dropFirst(MediaLink.prefix.count))) }
        else { url = nil }
        return ImageRef(range: range, pathRange: pathRange, raw: raw, alt: alt, isTag: isTag, url: url, isRemote: remote)
    }

    func setImageWidth(at idx: Int) {
        guard let ref = imageReference(at: idx) else { return }
        var current = ""
        if ref.isTag {
            let tag = nsText.substring(with: ref.range)
            if let r = tag.range(of: "width=\"(\\d+)\"", options: .regularExpression) { current = String(tag[r].filter(\.isNumber)) }
        }
        guard let w = Prompt.text(title: "Set Image Width", message: "Width in pixels (leave empty to remove):", initial: current.isEmpty ? "300" : current) else { return }
        let width = w.filter(\.isNumber)
        let path = ImageNaming.decodeMarkdownPath(ref.raw)
        let altAttr = ref.alt.isEmpty ? "" : " alt=\"\(ref.alt.replacingOccurrences(of: "\"", with: "&quot;"))\""
        let rep = width.isEmpty ? "![\(ref.alt)](\(ImageNaming.markdownPath(path, style: .angleBrackets)))"
                                : "<img src=\"\(path)\"\(altAttr) width=\"\(width)\">"
        apply(TextEdit(range: ref.range, replacement: rep, selection: NSRange(location: ref.range.location + (rep as NSString).length, length: 0)), actionName: "Set Image Width")
    }

    // MARK: Links (Cmd-click)


    /// Opens a Markdown link at a location. Returns true if handled.
    /// The link under a character index (or just before it, for a click on a link's last character).
    func link(at idx: Int) -> LinkFinder.Link? {
        guard nsText.length > 0 else { return nil }
        let lr = nsText.lineRange(for: NSRange(location: min(idx, nsText.length - 1), length: 0))
        let line = nsText.substring(with: lr)
        for i in [idx, idx - 1] where i >= lr.location && i < NSMaxRange(lr) {
            if var l = LinkFinder.link(in: line, at: i - lr.location) {
                l.range.location += lr.location
                return l
            }
        }
        return nil
    }

    /// ⌘-click: web and mail links open in the default app, media opens in its viewer, and
    /// `#heading` links jump to the heading.
    func openLink(at idx: Int) -> Bool {
        guard let link = link(at: idx) else { return false }
        if let url = LinkFinder.externalURL(link.target) {
            NSWorkspace.shared.open(url)
            return true
        }
        let target = ImageNaming.decodeMarkdownPath(link.target)
        if target.hasPrefix(MediaLink.prefix) {
            let id = String(target.dropFirst(MediaLink.prefix.count))
            if let url = document.mediaURL(id), FileManager.default.fileExists(atPath: url.path) {
                NSWorkspace.shared.open(url)
            } else {
                document.reveal(FixLink(kind: "media", target: id, line: nil))
            }
            return true
        }
        if target.hasPrefix("#") {
            let wanted = target.dropFirst().lowercased().replacingOccurrences(of: "-", with: " ")
            if let item = document.outline.first(where: { $0.title.lowercased().replacingOccurrences(of: "[^a-z0-9 ]", with: "", options: .regularExpression) == wanted }) {
                goToLine(item.line)
                return true
            }
        }
        return false
    }

}

/// `media:<id>` links: how the spec refers to project media (spec §5).
enum MediaLink {
    static let prefix = "media:"

    static func markdown(id: String, alt: String) -> String {
        let a = alt.replacingOccurrences(of: "[", with: "").replacingOccurrences(of: "]", with: "")
        return "![\(a)](\(prefix)\(id))"
    }

    private static let rx = try! NSRegularExpression(pattern: "media:([A-Za-z0-9_-]+)")

    /// The media IDs a text refers to, in order.
    static func ids(in text: String) -> [String] {
        let ns = text as NSString
        return rx.matches(in: text, range: NSRange(location: 0, length: ns.length)).map { ns.substring(with: $0.range(at: 1)) }
    }
}
