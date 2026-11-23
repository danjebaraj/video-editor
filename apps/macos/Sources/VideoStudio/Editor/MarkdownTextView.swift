import AppKit
import MarkdownCore

/// NSTextView subclass that adds Markdown smart editing, popups, paste/drop handling and context menus.
final class MarkdownTextView: NSTextView {
    weak var controller: EditorController?
    private var hoverWork: DispatchWorkItem?
    private var hoverPopover: NSPopover?
    private var trackingArea: NSTrackingArea?

    private var c: EditorController? { controller }

    /// The height inline renderings need. Room made after the note's last line doesn't make the view taller
    /// by itself, so a rendering at the very end would sit out of scroll reach without this.
    var inlineMinHeight: CGFloat = 0 {
        didSet { if inlineMinHeight != oldValue { setFrameSize(frame.size) } }
    }

    override func setFrameSize(_ newSize: NSSize) {
        var size = newSize
        if inlineMinHeight > size.height { size.height = inlineMinHeight }
        super.setFrameSize(size)
    }

    /// A hover panel describes the text as it was; close it when the text changes (an island quick fix, for example).
    override func didChangeText() {
        super.didChangeText()
        hoverWork?.cancel()
        hoverPopover?.close()
    }

    // MARK: Drawing (island tint and problem underlines)

    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        c?.drawIslands(in: rect)
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        c?.drawIslandProblems(in: dirtyRect)
        c?.inlineDisplay.draw(in: dirtyRect)
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        c?.inlineDisplay.appearanceChanged()
    }

    // MARK: Keys

    override func doCommand(by selector: Selector) {
        if c?.completion.handle(selector) == true { return }
        super.doCommand(by: selector)
    }

    override func keyDown(with event: NSEvent) {
        super.keyDown(with: event)
    }

    override func insertNewline(_ sender: Any?) {
        guard let c, isEditable else { return super.insertNewline(sender) }
        if c.completion.handle(#selector(insertNewline(_:))) { return }
        var sel = selectedRange()
        if sel.length == 0, !c.isInCodeBlock(sel.location) {
            // `@td: text` becomes `- [ ] text` before the new line (which then continues the list).
            if c.convertTodoShortcut(line: c.scan.line(at: sel.location), beforeMoreEdits: true) { sel = selectedRange() }
        }
        if sel.length == 0 {
            if c.tableEnter() { return }
            if !c.isInCodeBlock(sel.location), let e = MarkdownEditing.listContinuation(c.nsText, caret: sel.location) {
                c.apply(e, actionName: "Typing")
                return
            }
        }
        super.insertNewline(sender)
    }

    override func insertTab(_ sender: Any?) {
        guard let c, isEditable else { return super.insertTab(sender) }
        if c.completion.handle(#selector(insertTab(_:))) { return }
        if c.tableTab(backward: false) { return }
        // `@t:3,4` then Tab inserts a table.
        if c.expandTableShortcut() { return }
        let sel = selectedRange()
        // Tab on an `@td:` line expands it in place (no new todo below, unlike Return).
        if sel.length == 0, !c.isInCodeBlock(sel.location), c.convertTodoShortcut(line: c.scan.line(at: sel.location)) {
            let end = NSMaxRange(MarkdownEditing.lineRange(c.nsText, for: selectedRange()))
            let lineEnd = end > 0 && end <= c.nsText.length && c.nsText.character(at: end - 1) == 10 ? end - 1 : end
            setSelectedRange(NSRange(location: lineEnd, length: 0))
            return
        }
        let lineRange = MarkdownEditing.lineRange(c.nsText, for: sel)
        let line = c.nsText.substring(with: lineRange)
        if MarkdownEditing.isListLine(line) || (sel.length > 0 && line.contains("\n")) {
            c.perform(.indent)
            return
        }
        insertText(AppSettings.shared.indentUnit, replacementRange: sel)
    }

    override func insertBacktab(_ sender: Any?) {
        guard let c, isEditable else { return super.insertBacktab(sender) }
        if c.tableTab(backward: true) { return }
        c.perform(.outdent)
    }

    // The floating table toolbar follows keyboard focus; update once focus has settled.
    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        DispatchQueue.main.async { [weak self] in self?.c?.updateTableToolbar() }
        return ok
    }

    override func resignFirstResponder() -> Bool {
        let ok = super.resignFirstResponder()
        DispatchQueue.main.async { [weak self] in self?.c?.updateTableToolbar() }
        return ok
    }

    override func cancelOperation(_ sender: Any?) {
        if c?.completion.handle(#selector(cancelOperation(_:))) == true { return }
        if let d = c?.document, d.showFindBar {
            d.showFindBar = false
            return
        }
        super.cancelOperation(sender)
    }

    override func insertText(_ string: Any, replacementRange: NSRange) {
        guard let c, isEditable else { return super.insertText(string, replacementRange: replacementRange) }
        let s = (string as? String) ?? (string as? NSAttributedString)?.string ?? ""
        var sel = replacementRange.location == NSNotFound ? selectedRange() : replacementRange
        if replacementRange.location == NSNotFound { sel = selectedRange() }
        // Wrap a selection with paired characters.
        let pairs: [String: String] = ["`": "`", "*": "*", "_": "_", "(": ")", "[": "]", "\"": "\""]
        if AppSettings.shared.autoPair, sel.length > 0, let close = pairs[s] {
            let inner = c.nsText.substring(with: sel)
            c.apply(TextEdit(range: sel, replacement: s + inner + close, selection: NSRange(location: sel.location + 1, length: (inner as NSString).length)), actionName: "Typing")
            return
        }
        // `@t:3,4` then Space inserts a table (the space isn't typed).
        if s == " ", replacementRange.location == NSNotFound, c.expandTableShortcut() { return }
        super.insertText(string, replacementRange: replacementRange)
        let caret = selectedRange().location
        if s == "/" { maybeOpenSlash(at: caret - 1) }
    }

    private func maybeOpenSlash(at loc: Int) {
        guard let c, loc >= 0 else { return }
        let ns = c.nsText
        if loc > 0 {
            let prev = ns.character(at: loc - 1)
            guard prev == 10 || prev == 32 || prev == 9 else { return }
        }
        guard !c.isInCodeBlock(loc), !c.isInInlineCode(loc) else { return }
        // Not inside a URL: look back to the previous whitespace.
        let lineStart = ns.lineRange(for: NSRange(location: loc, length: 0)).location
        let before = ns.substring(with: NSRange(location: lineStart, length: loc - lineStart))
        if let lastToken = before.split(whereSeparator: { $0 == " " || $0 == "\t" }).last, lastToken.contains("://") { return }
        c.completion.showSlash(in: c, anchor: loc)
    }

    // MARK: Paste & drop

    override func paste(_ sender: Any?) {
        guard let c, isEditable else { return super.paste(sender) }
        if c.handlePaste(NSPasteboard.general) { return }
        if let s = NSPasteboard.general.string(forType: .string), c.pasteRenamingIslands(s) { return }
        pasteAsPlainText(sender)
    }

    /// Paste stays enabled for a clipboard that holds only an image or files (a plain-text view would disable it).
    override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(paste(_:)), isEditable, c != nil, ImageInput.canRead(.general) { return true }
        return super.validateUserInterfaceItem(item)
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        if acceptsImageDrag(sender.draggingPasteboard) { return .copy }
        return super.draggingEntered(sender)
    }

    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
        if acceptsImageDrag(sender.draggingPasteboard) {
            // Move the caret to follow the drop point.
            let p = convert(sender.draggingLocation, from: nil)
            let idx = characterIndexForInsertion(at: p)
            setSelectedRange(NSRange(location: idx, length: 0))
            return .copy
        }
        return super.draggingUpdated(sender)
    }

    /// Dropped files go through the image pipeline (non-images report "Choose a supported image.");
    /// Markdown files are imported as notes. Dropped image data is handled like a paste.
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        let pb = sender.draggingPasteboard
        guard let c else { return super.performDragOperation(sender) }
        if let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], !urls.isEmpty {
            // Every dropped file becomes a media reference; nothing is copied (spec §5, §6).
            if isEditable { c.insertMediaFiles(urls) }
            return true
        }
        if isEditable, let images = ImageInput.read(pb) {
            c.ingestImages(images)
            return true
        }
        return super.performDragOperation(sender)
    }

    private func acceptsImageDrag(_ pb: NSPasteboard) -> Bool {
        pb.canReadObject(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) || (isEditable && ImageInput.canRead(pb))
    }

    // MARK: Mouse: ⌘-click links, image hover

    override func mouseDown(with event: NSEvent) {
        if let c, c.inlineDisplay.handleClick(at: convert(event.locationInWindow, from: nil)) { return }
        if event.modifierFlags.contains(.command), let c {
            let idx = characterIndexForInsertion(at: convert(event.locationInWindow, from: nil))
            if c.openLink(at: idx) { return }
        }
        // A click on a task item's box toggles it.
        if let c, event.clickCount == 1, event.modifierFlags.intersection([.command, .shift, .option, .control]).isEmpty,
           let idx = todoBoxIndex(at: convert(event.locationInWindow, from: nil)), c.handleTodoClick(at: idx) { return }
        super.mouseDown(with: event)
    }

    /// The character under a point, when the point is on a glyph (not past the end of a line).
    private func todoBoxIndex(at p: NSPoint) -> Int? {
        let ins = characterIndexForInsertion(at: p)
        guard ins >= 0, ins <= (string as NSString).length else { return nil }
        // Use the glyph's own rectangle so a click left of the box (in the indent) doesn't count.
        for candidate in [ins - 1, ins] where candidate >= 0 && candidate < (string as NSString).length {
            if let r = rectForCharacter(candidate), r.insetBy(dx: -1, dy: -1).contains(p) { return candidate }
        }
        return nil
    }

    private func rectForCharacter(_ i: Int) -> NSRect? {
        guard let tlm = textLayoutManager, let tcm = tlm.textContentManager,
              let start = tcm.location(tcm.documentRange.location, offsetBy: i), let end = tcm.location(start, offsetBy: 1),
              let range = NSTextRange(location: start, end: end) else { return nil }
        var rect: NSRect?
        tlm.enumerateTextSegments(in: range, type: .standard, options: []) { _, frame, _, _ in
            rect = frame.offsetBy(dx: self.textContainerOrigin.x, dy: self.textContainerOrigin.y)
            return false
        }
        return rect
    }

    /// With ⌘ held, the pointer becomes a hand over a link (⌘-click opens it).
    private func updateLinkCursor(_ event: NSEvent) {
        guard let c, event.modifierFlags.contains(.command), let window, window.isKeyWindow else { return }
        let p = convert(event.locationInWindow, from: nil)
        guard bounds.contains(p) else { return }
        if c.link(at: characterIndexForInsertion(at: p)) != nil { NSCursor.pointingHand.set() } else { NSCursor.iBeam.set() }
    }

    override func flagsChanged(with event: NSEvent) {
        super.flagsChanged(with: event)
        if event.modifierFlags.contains(.command) { updateLinkCursor(event) } else { NSCursor.iBeam.set() }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let trackingArea { removeTrackingArea(trackingArea) }
        let area = NSTrackingArea(rect: visibleRect, options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(area)
        trackingArea = area
    }

    override func mouseMoved(with event: NSEvent) {
        super.mouseMoved(with: event)
        updateLinkCursor(event)
        hoverWork?.cancel()
        let point = convert(event.locationInWindow, from: nil)
        let work = DispatchWorkItem { [weak self] in self?.showImageHover(at: point) }
        hoverWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5, execute: work)
    }

    private func showImageHover(at point: NSPoint) {
        guard let c else { return }
        let idx = characterIndexForInsertion(at: point)
        if showIslandHover(idx, at: point) { return }
        // No hover preview when it's turned off, or while images are already drawn inline.
        guard AppSettings.shared.imageHoverPreview, !AppSettings.shared.inlineRendering,
              let info = c.imageReference(at: idx), let url = info.url, let image = NSImage(contentsOf: url) else {
            hoverPopover?.close(); return
        }
        if hoverPopover?.isShown == true { return }
        let size = image.size
        let scale = min(1, 240 / max(size.width, size.height, 1))
        let view = NSImageView(image: image)
        view.imageScaling = .scaleProportionallyUpOrDown
        view.frame = NSRect(x: 0, y: 0, width: max(40, size.width * scale), height: max(40, size.height * scale))
        let vc = NSViewController()
        vc.view = view
        let pop = NSPopover()
        pop.behavior = .transient
        pop.contentViewController = vc
        pop.show(relativeTo: NSRect(origin: point, size: NSSize(width: 1, height: 1)), of: self, preferredEdge: .maxY)
        hoverPopover = pop
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak pop] in pop?.close() }
    }

    /// Hovering an island marker shows its name, any problem and "Ask Assistant about this island".
    private func showIslandHover(_ idx: Int, at point: NSPoint) -> Bool {
        guard let c, let marker = c.islandMarker(at: idx) else { return false }
        if hoverPopover?.isShown == true { return true }
        let island = c.islandScan.islands.first { $0.start == marker || $0.end == marker }
        let problem = c.islandScan.problems.first { $0.marker == marker }
        let pop = NSPopover()
        let vc = NSViewController()
        vc.view = IslandHover.view(island: island, marker: marker, problem: problem) { [weak self, weak pop] in
            pop?.close()
            if let island { self?.c?.askAboutIsland(island) }
        }
        pop.behavior = .transient
        pop.contentViewController = vc
        pop.show(relativeTo: NSRect(origin: point, size: NSSize(width: 1, height: 1)), of: self, preferredEdge: .maxY)
        hoverPopover = pop
        return true
    }

    // MARK: Context menu

    override func menu(for event: NSEvent) -> NSMenu? {
        let menu = super.menu(for: event) ?? NSMenu()
        guard let c else { return menu }
        let idx = characterIndexForInsertion(at: convert(event.locationInWindow, from: nil))
        var extra: [NSMenuItem] = []
        if c.tableLocation(at: idx) != nil {
            let item = NSMenuItem(title: "Table", action: nil, keyEquivalent: "")
            item.submenu = c.tableMenu()
            extra.append(item)
        }
        if let problem = c.islandProblem(at: idx) {
            for fix in c.fixes(for: problem) {
                let item = ClosureMenuItem(title: fix.title, action: fix.run)
                item.toolTip = problem.message
                extra.append(item)
            }
        }
        if let island = c.island(at: idx), isEditable {
            let item = NSMenuItem(title: "Island “\(island.name)”", action: nil, keyEquivalent: "")
            let sub = NSMenu()
            if island.isUsable {
                sub.addItem(ClosureMenuItem(title: "Ask Assistant…") { [weak c] in c?.askAboutIsland(island) })
                sub.items.last?.isHidden = !AppState.shared.assistantAvailable
                sub.addItem(ClosureMenuItem(title: "Copy Reference") { [weak c] in c?.copyIslandReference(island) })
                sub.addItem(.separator())
            }
            sub.addItem(ClosureMenuItem(title: "Rename Island…") { [weak c] in c?.renameIsland(island) })
            sub.addItem(ClosureMenuItem(title: "Unwrap Island") { [weak c] in c?.unwrapIsland(island) })
            sub.addItem(ClosureMenuItem(title: "Delete Island") { [weak c] in c?.deleteIsland(island) })
            item.submenu = sub
            extra.append(item)
        } else if selectedRange().length > 0, isEditable {
            extra.append(ClosureMenuItem(title: "Wrap in Island") { [weak c] in c?.insertIsland() })
        }
        if let img = c.imageReference(at: idx) {
            func item(_ title: String, _ action: Selector) {
                let i = NSMenuItem(title: title, action: action, keyEquivalent: "")
                i.representedObject = idx
                extra.append(i)
            }
            item("Set Width…", #selector(setImageWidth(_:)))
            item("Remove Image", #selector(removeImage(_:)))
        }
        if selectedRange().length > 0 {
            extra.append(NSMenuItem(title: "Convert CSV/TSV to Table", action: #selector(convertToTable(_:)), keyEquivalent: ""))
        }
        if AppState.shared.assistantAvailable, selectedRange().length > 0 {
            extra.append(ClosureMenuItem(title: "Ask Assistant About Selection…") { [weak c] in c?.askAboutSelection() })
        }
        if !extra.isEmpty {
            for (i, item) in extra.enumerated() { menu.insertItem(item, at: i) }
            menu.insertItem(.separator(), at: extra.count)
        }
        return menu
    }

    @objc func tableMenuAction(_ sender: NSMenuItem) {
        guard let raw = sender.representedObject as? String, let op = EditorController.TableOp(rawValue: raw) else { return }
        c?.performTableOp(op)
    }

    @objc func setImageWidth(_ sender: NSMenuItem) {
        guard let idx = sender.representedObject as? Int else { return }
        c?.setImageWidth(at: idx)
    }

    @objc func removeImage(_ sender: NSMenuItem) {
        guard let idx = sender.representedObject as? Int else { return }
        c?.removeImage(at: idx)
    }

    @objc func convertToTable(_ sender: Any?) {
        c?.convertSelectionToTable()
    }
}
