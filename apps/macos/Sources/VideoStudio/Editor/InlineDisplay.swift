import AppKit
import MarkdownCore

/// Draws images, Mermaid diagrams and math blocks under their Markdown source in the editor when
/// View › Show Images and Diagrams Inline is on. The text never changes: room is made with paragraph
/// spacing after the item's last line, and the rendering is drawn into that space. An item whose
/// source holds the caret shows only its text (clicking a rendering puts the caret there).
@MainActor
final class InlineDisplay {
    unowned let editor: EditorController

    struct Item {
        var renderable: InlineRenderable
        var image: NSImage?
        var size: NSSize = .zero
        /// Where it was last drawn, in text view coordinates (for clicks).
        var frame: NSRect = .zero
    }

    private(set) var items: [Item] = []
    /// Extra space after a line (line index → points), applied as paragraph spacing.
    private(set) var spacing: [Int: CGFloat] = [:]
    private var work: DispatchWorkItem?
    private var revealed: ClosedRange<Int>?
    private var lastWidth: CGFloat = 0
    /// Line starts and length of the text the items were found in.
    private var lineStarts: [Int] = []
    private var textLength = 0

    static let gap: CGFloat = 6
    static let imageMaxHeight: CGFloat = 420
    static let diagramMaxHeight: CGFloat = 640
    private static var imageCache = NSCache<NSString, NSImage>()

    init(editor: EditorController) { self.editor = editor }

    var isEnabled: Bool { AppSettings.shared.inlineRendering }

    // MARK: Updating

    /// Recomputes the items soon (after an edit) or now.
    func scheduleRefresh(delay: Double = 0.25) {
        work?.cancel()
        let w = DispatchWorkItem { [weak self] in self?.refresh() }
        work = w
        DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: w)
    }

    func refresh() {
        work?.cancel()
        guard isEnabled else {
            items = []
            apply([:])
            return
        }
        // A fresh scan of the current text: the editor's own scan can lag behind an edit that hasn't been processed yet.
        let text = editor.text
        let scan = BlockScanner.scan(text)
        lineStarts = scan.lineStarts
        textLength = (text as NSString).length
        let found = InlineRenderables.find(in: text, scan: scan)
        let dark = isDark
        var next: [Item] = []
        for r in found {
            var item = Item(renderable: r)
            switch r.kind {
            case .image(let path, _):
                item.image = loadImage(path)
            case .mermaid(let src), .math(let src):
                let key = InlineRenderService.Key(kind: r.kind.isMermaid ? .mermaid : .math, source: src, dark: dark)
                if let img = InlineRenderService.shared.cached(key) {
                    item.image = img
                } else {
                    InlineRenderService.shared.render(key) { [weak self] img in
                        guard img != nil, let self, self.isEnabled else { return }
                        self.scheduleRefresh(delay: 0.02)
                    }
                }
            }
            item.size = displaySize(item)
            next.append(item)
        }
        items = next
        revealed = caretLines
        apply(computeSpacing())
    }

    /// Lines shifted by an edit before the next refresh; keeps spacing on the right lines while typing.
    func shiftLines(after line: Int, by delta: Int) {
        guard delta != 0, !spacing.isEmpty else { return }
        var out: [Int: CGFloat] = [:]
        for (l, v) in spacing { out[l > line ? l + delta : l] = v }
        spacing = out
    }

    func selectionChanged() {
        guard isEnabled, !items.isEmpty else { return }
        let now = caretLines
        guard now != revealed else { return }
        let touched = items.filter { overlaps($0.renderable.lines, now) != overlaps($0.renderable.lines, revealed) }
        revealed = now
        if !touched.isEmpty { apply(computeSpacing()) }
    }

    /// The text view's width changed: images fit the new width.
    func widthChanged() {
        let w = availableWidth
        guard isEnabled, abs(w - lastWidth) > 1 else { return }
        lastWidth = w
        scheduleRefresh(delay: 0.1)
    }

    func appearanceChanged() {
        guard isEnabled else { return }
        scheduleRefresh(delay: 0.05)
    }

    private var isDark: Bool {
        editor.textView.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
    }

    /// The lines the selection covers: items there show their text instead of the rendering.
    private var caretLines: ClosedRange<Int>? {
        guard !lineStarts.isEmpty else { return nil }
        let sel = editor.textView.selectedRange()
        func line(_ loc: Int) -> Int {
            var lo = 0, hi = lineStarts.count - 1
            while lo < hi { let mid = (lo + hi + 1) / 2; if lineStarts[mid] <= loc { lo = mid } else { hi = mid - 1 } }
            return lo
        }
        let a = line(sel.location), b = line(NSMaxRange(sel))
        return a...max(a, b)
    }

    private func overlaps(_ a: ClosedRange<Int>, _ b: ClosedRange<Int>?) -> Bool { b.map { a.overlaps($0) } ?? false }

    private func isShown(_ item: Item) -> Bool { item.image != nil && !overlaps(item.renderable.lines, revealed) }

    private func computeSpacing() -> [Int: CGFloat] {
        var out: [Int: CGFloat] = [:]
        for item in items where isShown(item) {
            out[item.renderable.anchorLine, default: Self.gap] += item.size.height + Self.gap
        }
        return out
    }

    /// Sets paragraph spacing on the lines whose value changed and redraws.
    private func apply(_ new: [Int: CGFloat]) {
        let old = spacing
        spacing = new
        let changed = Set(old.keys).union(new.keys).filter { old[$0] != new[$0] }
        if let storage = editor.textView.textStorage, !changed.isEmpty {
            // Only when the text is still the one the lines were computed for; otherwise the pending refresh redoes it.
            guard storage.length == textLength, !lineStarts.isEmpty else { scheduleRefresh(delay: 0.05); return }
            storage.beginEditing()
            for line in changed where line < lineStarts.count {
                let start = lineStarts[line]
                let end = line + 1 < lineStarts.count ? lineStarts[line + 1] : storage.length
                guard end > start, end <= storage.length else { continue }
                storage.addAttribute(.paragraphStyle, value: editor.highlighter.paragraphStyle(spacingAfter: new[line]), range: NSRange(location: start, length: end - start))
            }
            storage.endEditing()
        }
        editor.textView.needsDisplay = true
    }

    // MARK: Sizes and images

    private var availableWidth: CGFloat {
        let tv = editor.textView
        let pad = tv.textContainer?.lineFragmentPadding ?? 5
        return max(80, (tv.textContainer?.size.width ?? tv.bounds.width) - 2 * pad - 4)
    }

    private func displaySize(_ item: Item) -> NSSize {
        guard let img = item.image, img.size.width > 0, img.size.height > 0 else { return .zero }
        let maxH: CGFloat
        switch item.renderable.kind {
        case .image: maxH = Self.imageMaxHeight
        case .mermaid: maxH = Self.diagramMaxHeight
        case .math: maxH = Self.diagramMaxHeight
        }
        let scale = min(1, availableWidth / img.size.width, maxH / img.size.height)
        return NSSize(width: floor(img.size.width * scale), height: floor(img.size.height * scale))
    }

    /// Local images only: `assets/…` (the Assets folder), absolute paths and `file:` URLs. Web images aren't loaded.
    private func loadImage(_ path: String) -> NSImage? {
        let url: URL?
        if path.hasPrefix(MediaLink.prefix) { url = editor.document.mediaURL(String(path.dropFirst(MediaLink.prefix.count))) }
        else if path.hasPrefix("/") { url = URL(fileURLWithPath: path) }
        else if path.hasPrefix("file://") { url = URL(string: path) }
        else { url = nil }
        guard let url else { return nil }
        let mtime = (try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate)?.timeIntervalSince1970 ?? 0
        let key = "\(url.path)|\(mtime)" as NSString
        if let img = Self.imageCache.object(forKey: key) { return img }
        guard mtime > 0, let img = NSImage(contentsOf: url) else { return nil }
        // Use the pixel size at 2x (a Retina screenshot shows at its real size).
        if let rep = img.representations.first, rep.pixelsWide > 0 {
            img.size = NSSize(width: CGFloat(rep.pixelsWide) / 2, height: CGFloat(rep.pixelsHigh) / 2)
        }
        Self.imageCache.setObject(img, forKey: key)
        return img
    }

    // MARK: Drawing and clicks

    func draw(in dirtyRect: NSRect) {
        guard isEnabled, !items.isEmpty else { updateMinHeight(0); return }
        var yOffset: [Int: CGFloat] = [:]
        var lowest: CGFloat = 0
        defer { updateMinHeight(lowest > 0 ? lowest + Self.gap + editor.textView.textContainerInset.height : 0) }
        for i in items.indices {
            guard isShown(items[i]), let img = items[i].image else { items[i].frame = .zero; continue }
            let line = items[i].renderable.anchorLine
            guard let bottom = editor.textBottom(ofLine: line) else { continue }
            let y = bottom + (yOffset[line] ?? Self.gap)
            yOffset[line] = (yOffset[line] ?? Self.gap) + items[i].size.height + Self.gap
            let x = editor.textView.textContainerOrigin.x + (editor.textView.textContainer?.lineFragmentPadding ?? 5)
            let frame = NSRect(x: x, y: y, width: items[i].size.width, height: items[i].size.height)
            items[i].frame = frame
            lowest = max(lowest, frame.maxY)
            guard frame.intersects(dirtyRect) else { continue }
            NSGraphicsContext.saveGraphicsState()
            let path = NSBezierPath(roundedRect: frame, xRadius: 4, yRadius: 4)
            if case .image = items[i].renderable.kind {
                path.addClip()
                img.draw(in: frame, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: [.interpolation: NSImageInterpolation.high])
                NSColor.separatorColor.setStroke()
                path.lineWidth = 1
                path.stroke()
            } else {
                img.draw(in: frame, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: [.interpolation: NSImageInterpolation.high])
            }
            NSGraphicsContext.restoreGraphicsState()
        }
    }

    /// Keeps the text view tall enough for the lowest rendering (set after drawing, never during it).
    private func updateMinHeight(_ h: CGFloat) {
        let tv = editor.textView
        guard abs(tv.inlineMinHeight - h) > 0.5 else { return }
        DispatchQueue.main.async { [weak tv] in tv?.inlineMinHeight = h }
    }

    /// A click on a rendering puts the caret at the start of its source, which shows the text instead.
    func handleClick(at point: NSPoint) -> Bool {
        guard isEnabled, let item = items.first(where: { isShown($0) && $0.frame.contains(point) }) else { return false }
        let first = item.renderable.lines.lowerBound
        let loc = min(first < lineStarts.count ? lineStarts[first] : textLength, editor.textView.textStorage?.length ?? 0)
        editor.textView.window?.makeFirstResponder(editor.textView)
        editor.textView.setSelectedRange(NSRange(location: loc, length: 0))
        editor.textView.scrollRangeToVisible(NSRange(location: loc, length: 0))
        selectionChanged()
        return true
    }

    func isOverRendering(_ point: NSPoint) -> Bool {
        isEnabled && items.contains { isShown($0) && $0.frame.contains(point) }
    }
}

extension InlineRenderable.Kind {
    var isMermaid: Bool { if case .mermaid = self { return true } else { return false } }
}
