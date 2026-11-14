import AppKit
import MarkdownCore

/// The popup list used for slash commands and code-fence languages.
@MainActor
final class CompletionPopup: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    struct Item {
        var title: String
        var subtitle: String
        var symbol: String
        var payload: String
    }

    enum Kind { case slash, language }

    private var panel: NSPanel?
    private var table: NSTableView!
    private(set) var items: [Item] = []
    private(set) var kind: Kind = .slash
    /// Location of the trigger: the `/` for slash, insertion point for languages.
    private var anchor = 0
    private weak var editor: EditorController?

    var isVisible: Bool { panel?.isVisible == true }

    static let languages = ["swift", "python", "javascript", "typescript", "json", "yaml", "bash", "shell", "html", "css", "markdown", "sql",
                            "go", "rust", "java", "kotlin", "c", "cpp", "csharp", "ruby", "php", "objectivec", "xml", "diff", "plaintext", "mermaid"]

    // MARK: Showing

    func showSlash(in editor: EditorController, anchor: Int) {
        kind = .slash
        self.anchor = anchor
        self.editor = editor
        refresh()
    }

    func showLanguages(in editor: EditorController, anchor: Int) {
        kind = .language
        self.anchor = anchor
        self.editor = editor
        refresh()
    }

    private var query: String? {
        guard let editor else { return nil }
        let ns = editor.nsText
        let caret = editor.textView.selectedRange().location
        let start = kind == .slash ? anchor + 1 : anchor
        guard caret >= start, start <= ns.length else { return nil }
        if kind == .slash, anchor >= ns.length || ns.character(at: anchor) != 47 { return nil }
        let q = ns.substring(with: NSRange(location: start, length: caret - start))
        if q.contains("\n") { return nil }
        if kind == .slash && q.contains(" ") { return nil }
        if kind == .language && q.contains(" ") { return nil }
        return q
    }

    private func refresh() {
        guard let editor, let q = query else { close(); return }
        switch kind {
        case .slash:
            let firstLine = editor.scan.line(at: anchor) == 0
            items = SlashCommandCatalog.filter(q, onFirstLine: firstLine).map {
                Item(title: $0.title, subtitle: "/" + ([$0.name] + $0.aliases.prefix(2)).joined(separator: " /"), symbol: $0.symbol, payload: $0.id)
            }
        case .language:
            items = Self.languages.compactMap { l in SlashCommandCatalog.score(q, l).map { (l, $0) } }
                .sorted { $0.1 > $1.1 }.map { Item(title: $0.0, subtitle: "", symbol: "curlybraces", payload: $0.0) }
            items.insert(Item(title: "No language", subtitle: "", symbol: "text.alignleft", payload: ""), at: q.isEmpty ? 0 : items.count)
        }
        guard !items.isEmpty else { close(); return }
        present()
    }

    private func present() {
        guard let editor, let window = editor.textView.window else { return }
        if panel == nil { build() }
        table.reloadData()
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        table.scrollRowToVisible(0)
        let rowH: CGFloat = 26
        let height = min(CGFloat(items.count) * rowH + 8, 300)
        let local = editor.rect(forCharacterAt: anchor)
        let caretScreen = window.convertToScreen(editor.textView.convert(local, to: nil))
        var origin = NSPoint(x: caretScreen.minX - 8, y: caretScreen.minY - height - 4)
        if let screen = window.screen, origin.y < screen.visibleFrame.minY { origin.y = caretScreen.maxY + 4 }
        panel!.setFrame(NSRect(x: origin.x, y: origin.y, width: 300, height: height), display: true)
        if panel!.parent == nil { window.addChildWindow(panel!, ordered: .above) }
        panel!.orderFront(nil)
    }

    private func build() {
        let p = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 300, height: 200), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        p.isFloatingPanel = true
        p.hasShadow = true
        p.backgroundColor = .clear
        p.isOpaque = false
        let effect = NSVisualEffectView()
        effect.material = .menu
        effect.state = .active
        effect.wantsLayer = true
        effect.layer?.cornerRadius = 8
        effect.layer?.masksToBounds = true
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        table = NSTableView()
        table.headerView = nil
        table.backgroundColor = .clear
        table.rowHeight = 26
        table.intercellSpacing = NSSize(width: 0, height: 0)
        table.style = .plain
        table.addTableColumn(NSTableColumn(identifier: .init("c")))
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(clicked)
        scroll.documentView = table
        scroll.translatesAutoresizingMaskIntoConstraints = false
        effect.addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: effect.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: effect.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: effect.topAnchor, constant: 4),
            scroll.bottomAnchor.constraint(equalTo: effect.bottomAnchor, constant: -4),
        ])
        p.contentView = effect
        panel = p
    }

    func close() {
        if let p = panel, let parent = p.parent { parent.removeChildWindow(p) }
        panel?.orderOut(nil)
    }

    // MARK: Editor hooks

    func textChanged(in editor: EditorController) {
        guard isVisible, editor === self.editor else { return }
        refresh()
    }

    func selectionChanged(in editor: EditorController) {
        guard isVisible, editor === self.editor else { return }
        if query == nil { close() }
    }

    /// Handles navigation keys while visible. Returns true if consumed.
    func handle(_ selector: Selector) -> Bool {
        guard isVisible else { return false }
        switch selector {
        case #selector(NSResponder.moveDown(_:)):
            move(1); return true
        case #selector(NSResponder.moveUp(_:)):
            move(-1); return true
        case #selector(NSResponder.insertNewline(_:)), #selector(NSResponder.insertTab(_:)):
            confirm(); return true
        case #selector(NSResponder.cancelOperation(_:)):
            close(); return true
        default:
            return false
        }
    }

    private func move(_ d: Int) {
        let row = max(0, min(items.count - 1, table.selectedRow + d))
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        table.scrollRowToVisible(row)
    }

    @objc private func clicked() { confirm() }

    func confirm() {
        guard let editor, table.selectedRow >= 0, table.selectedRow < items.count else { close(); return }
        let item = items[table.selectedRow]
        let caret = editor.textView.selectedRange().location
        close()
        switch kind {
        case .slash:
            let typed = query ?? ""
            let range = NSRange(location: anchor, length: max(0, caret - anchor))
            editor.apply(TextEdit(range: range, replacement: "", selection: NSRange(location: anchor, length: 0)))
            if let cmd = SlashCommandCatalog.all.first(where: { $0.id == item.payload }) {
                editor.runSlashCommand(cmd, typed: typed)
            }
        case .language:
            let range = NSRange(location: anchor, length: max(0, caret - anchor))
            editor.apply(TextEdit(range: range, replacement: "", selection: NSRange(location: anchor, length: 0)))
            editor.insertCodeFence(language: item.payload)
        }
    }

    // MARK: Table

    func numberOfRows(in tableView: NSTableView) -> Int { items.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let item = items[row]
        let cell = NSTableCellView()
        let icon = NSImageView(image: NSImage(systemSymbolName: item.symbol, accessibilityDescription: nil) ?? NSImage(systemSymbolName: "textformat", accessibilityDescription: nil)!)
        icon.contentTintColor = .secondaryLabelColor
        let title = NSTextField(labelWithString: item.title)
        title.font = .systemFont(ofSize: 13)
        let sub = NSTextField(labelWithString: item.subtitle)
        sub.font = .systemFont(ofSize: 11)
        sub.textColor = .secondaryLabelColor
        sub.lineBreakMode = .byTruncatingMiddle
        let stack = NSStackView(views: [icon, title, sub])
        stack.spacing = 8
        stack.edgeInsets = NSEdgeInsets(top: 0, left: 10, bottom: 0, right: 10)
        sub.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        stack.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: cell.leadingAnchor),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: cell.trailingAnchor),
            stack.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 16),
        ])
        return cell
    }
}
