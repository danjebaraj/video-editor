import AppKit
import Combine
import MarkdownCore

/// Live editor statistics shown in the status bar.
@MainActor
final class EditorStatus: ObservableObject {
    @Published var words = 0
    @Published var characters = 0
    @Published var readingMinutes = 0
    @Published var line = 1
    @Published var column = 1
    @Published var isSelection = false
    /// A short non-modal message ("Image copied."), cleared after a few seconds.
    @Published private(set) var message: String?
    /// Images being processed in the background.
    @Published var busyCount = 0
    private var messageID = 0

    func show(_ text: String, seconds: Double = 5) {
        message = text
        messageID += 1
        let id = messageID
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [weak self] in
            if self?.messageID == id { self?.message = nil }
        }
    }
}

/// Owns the NSTextView for one document: highlighting, smart editing, commands, find and popups.
@MainActor
final class EditorController: NSObject, NSTextViewDelegate, NSTextStorageDelegate {
    unowned let document: ProjectDocument
    let scrollView: NSScrollView
    let textView: MarkdownTextView
    let highlighter = SyntaxHighlighter()
    let status = EditorStatus()
    let completion = CompletionPopup()
    private(set) var scan = BlockScanner()
    private var highlightedLines = IndexSet()
    private var pendingEditRange: NSRange?
    private var pendingDelta = 0
    private var statsWork: DispatchWorkItem?
    private var cancellables = Set<AnyCancellable>()
    private var lastTableRange: ClosedRange<Int>?
    var lineNumberRuler: LineNumberRulerView?
    /// Islands in this document, refreshed shortly after each edit (see `scheduleIslandScan`).
    var islandScan = IslandScan()
    var islandWork: DispatchWorkItem?
    /// Images, diagrams and math drawn under their source (View › Show Images and Diagrams Inline).
    lazy var inlineDisplay = InlineDisplay(editor: self)
    lazy var tableToolbar: TableToolbar = {
        let bar = TableToolbar()
        bar.isHidden = true
        bar.onCommand = { [weak self] in self?.performTable($0); self?.updateTableToolbar() }
        bar.onOperation = { [weak self] in self?.performTableOp($0); self?.updateTableToolbar() }
        return bar
    }()

    // Find state
    /// The line the caret was on, for converting `@td:` lines when the caret leaves them.
    var lastCaretLine: Int?
    /// Starting column widths of a table inserted with `@t:`, kept until the caret first leaves it.
    var tableMinimums: (headerLine: Int, widths: [Int])?
    /// Bumped when the whole text is replaced, so a pending `@td:` check for the old text is dropped.
    var todoTextGeneration = 0
    var findMatches: [NSRange] = []
    var findIndex = 0

    init(document: ProjectDocument, text: String) {
        self.document = document
        let tv = MarkdownTextView(usingTextLayoutManager: true)
        textView = tv
        scrollView = NSScrollView()
        super.init()
        tv.controller = self
        configureTextView()
        tv.string = text
        tv.setSelectedRange(NSRange(location: 0, length: 0))
        textView.textStorage?.delegate = self
        rescan()
        scheduleIslandScan(now: true)
        tv.typingAttributes = highlighter.baseAttributes
        textView.textStorage?.setAttributes(highlighter.baseAttributes, range: NSRange(location: 0, length: (text as NSString).length))
        observeSettings()
        DispatchQueue.main.async { [weak self] in
            self?.highlightVisible()
            self?.applyPendingPosition()
            if self?.inlineDisplay.isEnabled == true { self?.inlineDisplay.refresh() }
            self?.updateStatus()
        }
    }

    private func configureTextView() {
        let tv = textView
        tv.delegate = self
        tv.isRichText = false
        tv.importsGraphics = false
        tv.allowsUndo = true
        tv.usesFindBar = false
        tv.isAutomaticQuoteSubstitutionEnabled = false
        tv.isAutomaticDashSubstitutionEnabled = false
        tv.isAutomaticTextReplacementEnabled = false
        tv.isAutomaticSpellingCorrectionEnabled = false
        tv.isAutomaticLinkDetectionEnabled = false
        tv.smartInsertDeleteEnabled = false
        tv.drawsBackground = true
        tv.backgroundColor = .textBackgroundColor
        tv.textContainerInset = NSSize(width: 24, height: 18)
        tv.isVerticallyResizable = true
        tv.minSize = NSSize(width: 0, height: 0)
        tv.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        tv.autoresizingMask = [.width]
        tv.registerForDraggedTypes([.fileURL, .png, .tiff, .URL])

        scrollView.documentView = tv
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = true
        scrollView.backgroundColor = .textBackgroundColor
        scrollView.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.publisher(for: NSView.boundsDidChangeNotification, object: scrollView.contentView)
            .debounce(for: .milliseconds(40), scheduler: RunLoop.main)
            .sink { [weak self] _ in self?.highlightVisible() }
            .store(in: &cancellables)
        NotificationCenter.default.publisher(for: NSView.boundsDidChangeNotification, object: scrollView.contentView)
            .merge(with: NotificationCenter.default.publisher(for: NSView.frameDidChangeNotification, object: scrollView))
            .sink { [weak self] _ in self?.updateTableToolbar() }
            .store(in: &cancellables)
        scrollView.postsFrameChangedNotifications = true
        AppSettings.shared.$showTableToolbar.dropFirst()
            .sink { [weak self] _ in DispatchQueue.main.async { self?.updateTableToolbar() } }
            .store(in: &cancellables)
        AppSettings.shared.$highlightIslands.merge(with: AppSettings.shared.$dimIslandMarkers).dropFirst(2)
            .sink { [weak self] _ in
                DispatchQueue.main.async {
                    // Rescan first: an editor in the background may not have scanned its latest text yet.
                    guard let self else { return }
                    self.rescan()
                    self.highlightedLines = []
                    self.highlightVisible()
                    self.textView.needsDisplay = true
                }
            }
            .store(in: &cancellables)
        applySettings()
    }

    private func observeSettings() {
        let s = AppSettings.shared
        Publishers.MergeMany(
            s.$fontName.map { _ in () }.eraseToAnyPublisher(),
            s.$fontSize.map { _ in () }.eraseToAnyPublisher(),
            s.$lineHeight.map { _ in () }.eraseToAnyPublisher(),
            s.$softWrap.map { _ in () }.eraseToAnyPublisher(),
            s.$lineNumbers.map { _ in () }.eraseToAnyPublisher(),
            s.$spellCheck.map { _ in () }.eraseToAnyPublisher(),
            s.$grammarCheck.map { _ in () }.eraseToAnyPublisher()
        )
        .dropFirst(7)
        .debounce(for: .milliseconds(50), scheduler: RunLoop.main)
        .sink { [weak self] in self?.applySettings(rehighlight: true) }
        .store(in: &cancellables)
        s.$inlineRendering.dropFirst().removeDuplicates().receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.inlineDisplay.refresh() }.store(in: &cancellables)
        textView.postsFrameChangedNotifications = true
        NotificationCenter.default.publisher(for: NSView.frameDidChangeNotification, object: textView)
            .sink { [weak self] _ in self?.inlineDisplay.widthChanged() }.store(in: &cancellables)
    }

    func applySettings(rehighlight: Bool = false) {
        let s = AppSettings.shared
        highlighter.baseFont = s.editorFont
        highlighter.updateFonts()
        let ps = NSMutableParagraphStyle()
        ps.lineHeightMultiple = CGFloat(s.lineHeight)
        highlighter.paragraphStyle = ps
        highlighter.imageExists = { [weak self] raw in self?.imageExists(raw) ?? true }
        highlighter.spacingAfterLine = { [weak self] line in self?.inlineDisplay.spacing[line] }
        // Setting NSTextView.font resets attributes on the whole text, so only do it when it changes (and re-highlight).
        var fontChanged = false
        if textView.font != s.editorFont {
            textView.font = s.editorFont
            fontChanged = true
        }
        textView.typingAttributes = highlighter.baseAttributes
        textView.isContinuousSpellCheckingEnabled = s.spellCheck
        textView.isGrammarCheckingEnabled = s.grammarCheck

        if s.softWrap {
            scrollView.hasHorizontalScroller = false
            textView.isHorizontallyResizable = false
            textView.textContainer?.widthTracksTextView = true
            textView.textContainer?.containerSize = NSSize(width: scrollView.contentSize.width, height: CGFloat.greatestFiniteMagnitude)
            textView.frame.size.width = scrollView.contentSize.width
        } else {
            scrollView.hasHorizontalScroller = true
            textView.isHorizontallyResizable = true
            textView.textContainer?.widthTracksTextView = false
            textView.textContainer?.containerSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        }

        if s.lineNumbers {
            if lineNumberRuler == nil {
                let ruler = LineNumberRulerView(controller: self)
                scrollView.verticalRulerView = ruler
                lineNumberRuler = ruler
            }
            scrollView.hasVerticalRuler = true
            scrollView.rulersVisible = true
        } else {
            scrollView.rulersVisible = false
        }

        if rehighlight || fontChanged {
            let full = NSRange(location: 0, length: (textView.string as NSString).length)
            textView.textStorage?.setAttributes(highlighter.baseAttributes, range: full)
            highlightedLines = []
            highlightVisible()
        }
    }

    /// Keeps the wrapping width in sync after the scroll view is (re)attached.
    func fitToContainer() {
        guard AppSettings.shared.softWrap else { return }
        let w = scrollView.contentSize.width
        if abs(textView.frame.width - w) > 0.5 { textView.frame.size.width = w }
    }

    // MARK: Text access

    var text: String { textView.string }
    var nsText: NSString { textView.string as NSString }

    /// Replaces the whole text (external reload or review resolution).
    func replaceAllText(_ newText: String, undoable: Bool) {
        let sel = textView.selectedRange()
        let full = NSRange(location: 0, length: nsText.length)
        if undoable {
            apply(TextEdit(range: full, replacement: newText, selection: NSRange(location: min(sel.location, (newText as NSString).length), length: 0)))
            return
        }
        todoTextGeneration += 1
        lastCaretLine = nil
        textView.textStorage?.replaceCharacters(in: full, with: NSAttributedString(string: newText, attributes: highlighter.baseAttributes))
        textView.undoManager?.removeAllActions(withTarget: textView.textStorage as Any)
        textView.setSelectedRange(NSRange(location: min(sel.location, (newText as NSString).length), length: 0))
        // This edit is fully rescanned and highlighted here; don't leave it pending for the next keystroke,
        // which would otherwise re-highlight the whole note.
        pendingEditRange = nil
        pendingDelta = 0
        editNeedsRescan = true
        rescan()
        highlightedLines = []
        highlightVisible()
        updateStatus()
        scheduleIslandScan(now: true)
    }

    /// Applies an edit as one undoable user action.
    func apply(_ e: TextEdit, actionName: String? = nil) {
        guard textView.isEditable else { NSSound.beep(); return }
        guard textView.shouldChangeText(in: e.range, replacementString: e.replacement) else { return }
        textView.textStorage?.replaceCharacters(in: e.range, with: NSAttributedString(string: e.replacement, attributes: highlighter.baseAttributes))
        textView.didChangeText()
        let len = nsText.length
        let sel = NSRange(location: min(e.selection.location, len), length: min(e.selection.length, max(0, len - min(e.selection.location, len))))
        textView.setSelectedRange(sel)
        textView.scrollRangeToVisible(sel)
        if let actionName { textView.undoManager?.setActionName(actionName) }
    }

    func insert(_ s: String, selecting: NSRange? = nil) {
        let sel = textView.selectedRange()
        let loc = sel.location
        apply(TextEdit(range: sel, replacement: s, selection: selecting.map { NSRange(location: loc + $0.location, length: $0.length) } ?? NSRange(location: loc + (s as NSString).length, length: 0)))
    }

    // MARK: Highlighting

    func rescan() {
        scan = BlockScanner.scan(nsString: nsText)
    }

    /// Set from shouldChangeText: whether the pending edit may change line structure or code fences.
    private var editNeedsRescan = true
    private var pendingEditLocation = 0

    func textView(_ textView: NSTextView, shouldChangeTextIn affectedCharRange: NSRange, replacementString: String?) -> Bool {
        let replaced = affectedCharRange.length > 0 && NSMaxRange(affectedCharRange) <= nsText.length ? nsText.substring(with: affectedCharRange) : ""
        let rep = replacementString ?? ""
        let special = CharacterSet(charactersIn: "\n\r`~-.")
        let touchesStructure = replaced.rangeOfCharacter(from: special) != nil || rep.rangeOfCharacter(from: special) != nil || replacementString == nil
        // Also rescan if the edited line itself is a fence/front-matter line.
        let line = scan.line(at: min(affectedCharRange.location, max(nsText.length - 1, 0)))
        let lineStart = scan.lineStarts.isEmpty ? 0 : scan.lineStarts[min(line, scan.lineStarts.count - 1)]
        let first = lineStart < nsText.length ? nsText.character(at: lineStart) : 0
        let markerLine = first == 96 || first == 126 || first == 45 || first == 32
        editNeedsRescan = editNeedsRescan || touchesStructure || markerLine
        pendingEditLocation = affectedCharRange.location
        return true
    }

    nonisolated func textStorage(_ textStorage: NSTextStorage, didProcessEditing editedMask: NSTextStorageEditActions, range editedRange: NSRange, changeInLength delta: Int) {
        guard editedMask.contains(.editedCharacters) else { return }
        MainActor.assumeIsolated {
            if let p = pendingEditRange {
                // Merge with a pending range (adjusting for the new edit's length change).
                let start = min(p.location, editedRange.location)
                let end = max(NSMaxRange(p) + (p.location >= editedRange.location ? delta : 0), NSMaxRange(editedRange))
                pendingEditRange = NSRange(location: start, length: max(0, end - start))
            } else {
                pendingEditRange = editedRange
            }
            pendingDelta += delta
        }
    }

    func textDidChange(_ notification: Notification) {
        let old = scan
        if editNeedsRescan || scan.lineStarts.isEmpty {
            rescan()
        } else {
            scan.applyInLineEdit(at: pendingEditLocation, delta: pendingDelta)
        }
        editNeedsRescan = false
        let edited = pendingEditRange ?? NSRange(location: 0, length: 0)
        pendingEditRange = nil
        pendingDelta = 0
        let firstLine = scan.line(at: min(edited.location, max(nsText.length - 1, 0)))
        let lastLine = scan.line(at: min(NSMaxRange(edited), max(nsText.length, 0)))
        let lineDelta = scan.lineStarts.count - old.lineStarts.count
        if structureChanged(old: old, new: scan, editLine: firstLine, lineDelta: lineDelta) {
            highlightedLines = []
            highlightVisible()
        } else if lineDelta != 0 {
            highlightedLines.shift(startingAt: firstLine + 1, by: lineDelta)
        }
        inlineDisplay.shiftLines(after: firstLine, by: lineDelta)
        let lo = max(0, firstLine - 1), hi = min(scan.lineStarts.count - 1, lastLine + 1)
        if lo <= hi {
            highlighter.highlight(storage: textView.textStorage!, scan: scan, lines: lo...hi)
            highlightedLines.insert(integersIn: lo...hi)
        }
        document.textDidChange()
        completion.textChanged(in: self)
        updateTableToolbar()
        scheduleIslandScan()
        lineNumberRuler?.needsDisplay = true
        scheduleStatusUpdate()
        if document.showFindBar { document.windowController?.findBar.refreshMatches(keepIndex: true) }
        if inlineDisplay.isEnabled { inlineDisplay.scheduleRefresh() }
    }

    private func structureChanged(old: BlockScanner, new: BlockScanner, editLine: Int, lineDelta: Int) -> Bool {
        if old.codeBlocks.count != new.codeBlocks.count || (old.frontMatter == nil) != (new.frontMatter == nil) { return true }
        if let a = old.frontMatter, let b = new.frontMatter, a.upperBound + (a.upperBound > editLine ? lineDelta : 0) != b.upperBound { return true }
        for (a, b) in zip(old.codeBlocks, new.codeBlocks) {
            let shiftLo = a.lowerBound > editLine ? lineDelta : 0
            let shiftHi = a.upperBound > editLine ? lineDelta : 0
            if a.lowerBound + shiftLo != b.lowerBound || a.upperBound + shiftHi != b.upperBound { return true }
        }
        return false
    }

    func visibleLineRange() -> ClosedRange<Int>? {
        guard !scan.lineStarts.isEmpty else { return nil }
        let last = scan.lineStarts.count - 1
        guard let tlm = textView.textLayoutManager, let tcm = tlm.textContentManager,
              let vr = tlm.textViewportLayoutController.viewportRange else {
            return 0...min(last, 80)
        }
        let start = tcm.offset(from: tcm.documentRange.location, to: vr.location)
        let end = tcm.offset(from: tcm.documentRange.location, to: vr.endLocation)
        guard start != NSNotFound, end != NSNotFound else { return 0...min(last, 80) }
        let a = scan.line(at: max(0, start))
        let b = max(a, scan.line(at: max(0, end)))
        return max(0, a - 20)...min(last, b + 40)
    }

    /// Character index at a point in the visible area, clamped to the text.
    private func characterIndex(atVisibleY offset: CGFloat) -> Int {
        let visible = textView.visibleRect
        let y = max(textView.textContainerOrigin.y + 1, visible.minY + offset)
        let idx = textView.characterIndexForInsertion(at: NSPoint(x: textView.textContainerOrigin.x + 4, y: y))
        return idx == NSNotFound ? 0 : min(idx, nsText.length)
    }

    func highlightVisible() {
        guard let range = visibleLineRange(), let storage = textView.textStorage else { return }
        // Highlight only lines not yet coloured, in contiguous runs.
        var missing = IndexSet(integersIn: range.lowerBound...range.upperBound)
        missing.subtract(highlightedLines)
        guard !missing.isEmpty else { return }
        for r in missing.rangeView {
            highlighter.highlight(storage: storage, scan: scan, lines: r.lowerBound...(r.upperBound - 1))
            applyFindAttributes(in: lineRangeChars(r.lowerBound...(r.upperBound - 1)))
        }
        highlightedLines.formUnion(missing)
        lineNumberRuler?.needsDisplay = true
    }

    // MARK: Find highlights (drawn as storage background attributes)

    /// Re-applies syntax colours to the lines covering `range` (removing old find backgrounds).
    func rehighlight(charRange range: NSRange) {
        guard let storage = textView.textStorage, !scan.lineStarts.isEmpty, range.location <= nsText.length else { return }
        let a = scan.line(at: range.location), b = scan.line(at: min(NSMaxRange(range), nsText.length))
        highlighter.highlight(storage: storage, scan: scan, lines: a...b)
        applyFindAttributes(in: lineRangeChars(a...b))
    }

    func applyFindAttributes(in range: NSRange) {
        guard !findMatches.isEmpty, let storage = textView.textStorage else { return }
        storage.beginEditing()
        for (i, m) in findMatches.enumerated() where NSIntersectionRange(m, range).length > 0 && NSMaxRange(m) <= storage.length {
            let color = i == findIndex ? NSColor.systemYellow.withAlphaComponent(0.65) : NSColor.systemYellow.withAlphaComponent(0.28)
            storage.addAttribute(.backgroundColor, value: color, range: m)
        }
        storage.endEditing()
    }

    /// Replaces the set of find matches, redrawing old and new ones.
    func setFindMatches(_ matches: [NSRange], current: Int) {
        let old = findMatches
        findMatches = matches
        findIndex = current
        var lines = IndexSet()
        for r in old where NSMaxRange(r) <= nsText.length {
            lines.insert(integersIn: scan.line(at: r.location)...scan.line(at: NSMaxRange(r)))
        }
        if let storage = textView.textStorage {
            for run in lines.rangeView where run.upperBound - 1 < scan.lineStarts.count {
                highlighter.highlight(storage: storage, scan: scan, lines: run.lowerBound...(run.upperBound - 1))
            }
        }
        if let first = matches.first, let last = matches.last {
            applyFindAttributes(in: NSUnionRange(first, last))
        }
    }

    // MARK: Image existence (broken-path underline)

    private var imageExistsCache: [String: Bool] = [:]

    /// Whether an image link's target can be shown: `media:<id>` must be in the project and found
    /// on disk; web images always count. Anything else is underlined as broken.
    func imageExists(_ raw: String) -> Bool {
        let path = ImageNaming.decodeMarkdownPath(raw)
        if path.hasPrefix("http://") || path.hasPrefix("https://") || path.hasPrefix("data:") || path.isEmpty { return true }
        guard path.hasPrefix(MediaLink.prefix) else { return false }
        let status = document.mediaStatus(String(path.dropFirst(MediaLink.prefix.count)))
        return status == "ok" || status == "moved" || status == "changed"
    }

    /// Media was added, relinked or went missing: redraw links with their new state.
    func mediaChanged() {
        invalidateImageCache()
        if inlineDisplay.isEnabled { inlineDisplay.refresh() }
    }

    /// Moves the caret to a 1-based line and scrolls it into view.
    func goToLine(_ line: Int) {
        scrollToLine(max(0, line - 1), alignTop: true, select: true)
    }

    func invalidateImageCache() {
        imageExistsCache = [:]
        highlightedLines = []
        highlightVisible()
    }

    // MARK: Selection & status

    func textViewDidChangeSelection(_ notification: Notification) {
        scheduleStatusUpdate(quick: true)
        checkLeftTable()
        completion.selectionChanged(in: self)
        updateTableToolbar()
        inlineDisplay.selectionChanged()
        todoSelectionChanged()
    }

    /// The character range of line `line` (including its newline).
    func lineRange(_ line: Int) -> NSRange {
        guard line >= 0, line < scan.lineStarts.count else { return NSRange(location: nsText.length, length: 0) }
        let s = scan.lineStarts[line]
        let e = line + 1 < scan.lineStarts.count ? scan.lineStarts[line + 1] : nsText.length
        return NSRange(location: s, length: e - s)
    }

    /// The bottom of a line's text (without its paragraph spacing), in text view coordinates.
    func textBottom(ofLine line: Int) -> CGFloat? {
        guard let tlm = textView.textLayoutManager, let tcm = tlm.textContentManager, line < scan.lineStarts.count, scan.lineStarts[line] <= nsText.length,
              let tl = tcm.location(tcm.documentRange.location, offsetBy: scan.lineStarts[line]),
              let frag = tlm.textLayoutFragment(for: tl) else { return nil }
        // Only the fragments holding the line's own text: at the end of the document the empty last line's
        // fragment shares this layout fragment and sits below any room made after the line.
        let own = frag.textLineFragments.filter { $0.characterRange.length > 0 }
        let lines = own.isEmpty ? Array(frag.textLineFragments.prefix(1)) : own
        var bottom = lines.map { $0.typographicBounds.maxY }.max() ?? frag.layoutFragmentFrame.height
        // Room made after the line (inline renderings) isn't part of its text. TextKit sometimes counts that
        // paragraph spacing in the last line's bounds, which would put a rendering below the room instead of in it.
        let start = scan.lineStarts[line]
        if start < nsText.length, let ps = textView.textStorage?.attribute(.paragraphStyle, at: start, effectiveRange: nil) as? NSParagraphStyle,
           ps.paragraphSpacing > 0 {
            let space = ps.paragraphSpacing
            let lastHeight = lines.last?.typographicBounds.height ?? 0
            if lastHeight >= space + highlighter.baseFont.pointSize * 0.8 { bottom -= space }
            // The next line starts right after the room, so the text ends no lower than that minus the room.
            if line + 1 < scan.lineStarts.count, let next = textTop(ofLine: line + 1) {
                let candidate = next - textView.textContainerOrigin.y - frag.layoutFragmentFrame.minY - space
                let lastTop = lines.last?.typographicBounds.minY ?? 0
                if candidate >= lastTop + highlighter.baseFont.pointSize * 0.8 { bottom = min(bottom, candidate) }
            }
        }
        return textView.textContainerOrigin.y + frag.layoutFragmentFrame.minY + bottom
    }

    /// The top of a line in text view coordinates.
    func textTop(ofLine line: Int) -> CGFloat? {
        guard let tlm = textView.textLayoutManager, let tcm = tlm.textContentManager, line < scan.lineStarts.count, scan.lineStarts[line] < nsText.length,
              let tl = tcm.location(tcm.documentRange.location, offsetBy: scan.lineStarts[line]),
              let frag = tlm.textLayoutFragment(for: tl) else { return nil }
        return textView.textContainerOrigin.y + frag.layoutFragmentFrame.minY
    }

    private func scheduleStatusUpdate(quick: Bool = false) {
        statsWork?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.updateStatus() }
        statsWork = work
        let delay = nsText.length > 200_000 ? 0.4 : (quick ? 0.03 : 0.12)
        DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: work)
    }

    func updateStatus() {
        let sel = textView.selectedRange()
        let caret = sel.location
        let line = scan.line(at: caret)
        let lineStart = scan.lineStarts.isEmpty ? 0 : scan.lineStarts[line]
        status.line = line + 1
        status.column = caret - lineStart + 1
        status.isSelection = sel.length > 0
        let target = sel.length > 0 ? nsText.substring(with: sel) : text
        if target.utf16.count > 300_000 {
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                let stats = TextStats.compute(target)
                DispatchQueue.main.async { self?.setStats(stats) }
            }
        } else {
            setStats(TextStats.compute(target))
        }
    }

    private func setStats(_ s: TextStats) {
        status.words = s.words
        status.characters = s.characters
        status.readingMinutes = s.readingMinutes
    }

    // MARK: Positions

    func positionState() -> (selection: Int, scrollLine: Int) {
        (textView.selectedRange().location, topVisibleLine())
    }

    func topVisibleLine() -> Int {
        return scan.line(at: characterIndex(atVisibleY: 4))
    }

    func caretLine() -> Int { scan.line(at: textView.selectedRange().location) }

    private func applyPendingPosition() {
        if let sel = document.pendingSelection {
            textView.setSelectedRange(NSRange(location: min(sel, nsText.length), length: 0))
            document.pendingSelection = nil
        }
        if let line = document.pendingScrollLine {
            scrollToLine(line, alignTop: true)
            document.pendingScrollLine = nil
        }
    }

    func scrollToLine(_ line: Int, alignTop: Bool = true, select: Bool = false) {
        guard !scan.lineStarts.isEmpty else { return }
        let l = min(max(line, 0), scan.lineStarts.count - 1)
        let loc = scan.lineStarts[l]
        if select { textView.setSelectedRange(NSRange(location: loc, length: 0)) }
        guard let tlm = textView.textLayoutManager, let tcm = tlm.textContentManager,
              let tl = tcm.location(tcm.documentRange.location, offsetBy: loc) else {
            textView.scrollRangeToVisible(NSRange(location: loc, length: 0)); return
        }
        tlm.ensureLayout(for: NSTextRange(location: tcm.documentRange.location, end: tl) ?? tcm.documentRange)
        if alignTop, let frag = tlm.textLayoutFragment(for: tl) {
            let y = frag.layoutFragmentFrame.minY + textView.textContainerOrigin.y
            let maxY = max(0, textView.frame.height - scrollView.contentView.bounds.height)
            scrollView.contentView.scroll(to: NSPoint(x: 0, y: min(max(0, y - 8), maxY)))
            scrollView.reflectScrolledClipView(scrollView.contentView)
        } else {
            textView.scrollRangeToVisible(NSRange(location: loc, length: 0))
        }
        highlightVisible()
    }

    func focus() {
        textView.window?.makeFirstResponder(textView)
    }

    func setReadOnly(_ readOnly: Bool) {
        textView.isEditable = !readOnly
    }

    // MARK: Spell check outside code

    func textView(_ textView: NSTextView, shouldSetSpellingState value: Int, range affectedCharRange: NSRange) -> Int {
        guard value != 0 else { return value }
        let line = scan.line(at: affectedCharRange.location)
        if scan.isInCodeOrFrontMatter(line: line) { return 0 }
        if isInInlineCode(affectedCharRange.location) { return 0 }
        return value
    }

    /// Whether a location sits inside an inline code span on its line.
    func isInInlineCode(_ loc: Int) -> Bool {
        let line = scan.line(at: loc)
        let start = scan.lineStarts.isEmpty ? 0 : scan.lineStarts[line]
        guard loc > start else { return false }
        let prefix = nsText.substring(with: NSRange(location: start, length: loc - start))
        return prefix.filter { $0 == "`" }.count % 2 == 1
    }

    func isInCodeBlock(_ loc: Int) -> Bool {
        scan.isInCodeOrFrontMatter(line: scan.line(at: loc))
    }

    // MARK: Table auto-format on leaving

    private func checkLeftTable() {
        // (Only once an edit has been processed: mid-edit the line index can lag behind the text.)
        if let keep = tableMinimums, pendingEditRange == nil, tableLineRange(at: caretLine())?.lowerBound != keep.headerLine {
            // Left the `@t:` table: from now on it's formatted normally.
            tableMinimums = nil
        }
        guard AppSettings.shared.autoFormatTables, textView.isEditable else { lastTableRange = nil; return }
        let line = caretLine()
        let current = tableLineRange(at: line)
        if let last = lastTableRange, current == nil || current!.lowerBound != last.lowerBound {
            lastTableRange = current
            formatTable(lines: last, keepCaret: true)
        } else {
            lastTableRange = current
        }
    }

    func tableLineRange(at line: Int) -> ClosedRange<Int>? {
        guard line < scan.lineStarts.count, let loc = tableLocation(at: scan.lineStarts[line]) else { return nil }
        return loc.headerLine...loc.lastLine
    }

    /// Returns lines near `line` (to avoid splitting the whole document).
    func linesAround(_ line: Int, radius: Int) -> (lines: [String], offset: Int) {
        let lo = max(0, line - radius), hi = min(scan.lineStarts.count - 1, line + radius)
        guard lo <= hi else { return ([], 0) }
        let start = scan.lineStarts[lo]
        let end = hi + 1 < scan.lineStarts.count ? scan.lineStarts[hi + 1] : nsText.length
        var s = nsText.substring(with: NSRange(location: start, length: end - start))
        if s.hasSuffix("\n") { s.removeLast() }
        return (s.components(separatedBy: "\n"), lo)
    }

    func lineRangeChars(_ lines: ClosedRange<Int>) -> NSRange {
        let start = scan.lineStarts[lines.lowerBound]
        var end = lines.upperBound + 1 < scan.lineStarts.count ? scan.lineStarts[lines.upperBound + 1] : nsText.length
        if end > start, end <= nsText.length, nsText.character(at: end - 1) == 10 { end -= 1 }
        return NSRange(location: start, length: end - start)
    }

    func lineText(_ line: Int) -> String {
        guard line >= 0, line < scan.lineStarts.count else { return "" }
        return nsText.substring(with: lineRangeChars(line...line))
    }
}
