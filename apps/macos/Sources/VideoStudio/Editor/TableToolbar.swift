import AppKit
import MarkdownCore

/// Compact bar that floats above the table at the caret: insert, delete, move, align, format and more.
/// The editor owns one per text view and shows it only while the caret is in a table (see `updateTableToolbar`).
final class TableToolbar: NSView {
    var onCommand: ((TableCommand) -> Void)?
    var onOperation: ((EditorController.TableOp) -> Void)?

    private let stack = NSStackView()
    private let alignButton = NSButton()
    private let moveButton = NSButton()
    private let moreButton = NSButton()
    private var alignment: TableAlignment = .none

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.borderWidth = 1
        shadow = {
            let s = NSShadow()
            s.shadowColor = NSColor.black.withAlphaComponent(0.18)
            s.shadowBlurRadius = 6
            s.shadowOffset = NSSize(width: 0, height: -2)
            return s
        }()
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel("Table")

        stack.orientation = .horizontal
        stack.spacing = 1
        stack.edgeInsets = NSEdgeInsets(top: 2, left: 4, bottom: 2, right: 4)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])

        for cmd in [TableCommand.insertRowAbove, .insertRowBelow, .insertColumnLeft, .insertColumnRight] {
            stack.addArrangedSubview(button(Self.gridIcon(for: cmd), cmd.sentenceTitle) { [weak self] in self?.onCommand?(cmd) })
        }
        addDivider()
        for cmd in [TableCommand.deleteRow, .deleteColumn] {
            stack.addArrangedSubview(button(Self.gridIcon(for: cmd), cmd.sentenceTitle) { [weak self] in self?.onCommand?(cmd) })
        }
        addDivider()
        configure(moveButton, Self.symbol("arrow.up.and.down.and.arrow.left.and.right"), "Move row or column") { [weak self] in self?.showMoveMenu() }
        stack.addArrangedSubview(moveButton)
        configure(alignButton, Self.symbol(Self.alignSymbol(.none)), "Column alignment") { [weak self] in self?.showAlignMenu() }
        stack.addArrangedSubview(alignButton)
        stack.addArrangedSubview(button(Self.symbol("wand.and.stars"), TableCommand.format.sentenceTitle) { [weak self] in self?.onCommand?(.format) })
        configure(moreButton, Self.symbol("ellipsis"), "More table actions") { [weak self] in self?.showMoreMenu() }
        stack.addArrangedSubview(moreButton)
        setFrameSize(fittingSize)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
        layer?.borderColor = NSColor.separatorColor.cgColor
    }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .arrow)
    }

    /// Shows the current column's alignment on the alignment button.
    func setAlignment(_ a: TableAlignment) {
        guard a != alignment else { return }
        alignment = a
        alignButton.image = Self.symbol(Self.alignSymbol(a))
    }

    // MARK: Buttons and menus

    private var actions: [ObjectIdentifier: () -> Void] = [:]

    private func button(_ image: NSImage?, _ tip: String, _ action: @escaping () -> Void) -> NSButton {
        let b = NSButton()
        configure(b, image, tip, action)
        return b
    }

    private func configure(_ b: NSButton, _ image: NSImage?, _ tip: String, _ action: @escaping () -> Void) {
        b.image = image
        b.imagePosition = .imageOnly
        b.bezelStyle = .accessoryBarAction
        b.isBordered = false
        b.showsBorderOnlyWhileMouseInside = true
        b.refusesFirstResponder = true
        b.toolTip = tip
        b.setAccessibilityLabel(tip)
        b.target = self
        b.action = #selector(buttonPressed(_:))
        b.translatesAutoresizingMaskIntoConstraints = false
        b.widthAnchor.constraint(equalToConstant: 24).isActive = true
        b.heightAnchor.constraint(equalToConstant: 20).isActive = true
        actions[ObjectIdentifier(b)] = action
    }

    @objc private func buttonPressed(_ sender: NSButton) {
        actions[ObjectIdentifier(sender)]?()
    }

    private func addDivider() {
        let d = NSBox()
        d.boxType = .separator
        d.translatesAutoresizingMaskIntoConstraints = false
        d.heightAnchor.constraint(equalToConstant: 14).isActive = true
        stack.addArrangedSubview(d)
    }

    private func menuItem(_ title: String, _ image: NSImage? = nil, _ action: @escaping () -> Void) -> NSMenuItem {
        let item = ClosureMenuItem(title: title, action: action)
        item.image = image
        return item
    }

    private func popUp(_ menu: NSMenu, from sender: NSView) {
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: sender.bounds.height + 3), in: sender)
    }

    private func showMoveMenu() {
        let menu = NSMenu()
        for cmd in [TableCommand.moveRowUp, .moveRowDown, .moveColumnLeft, .moveColumnRight] {
            if cmd == .moveColumnLeft { menu.addItem(.separator()) }
            menu.addItem(menuItem(cmd.title) { [weak self] in self?.onCommand?(cmd) })
        }
        popUp(menu, from: moveButton)
    }

    private func showAlignMenu() {
        let menu = NSMenu()
        for (a, title) in [(TableAlignment.left, "Left"), (.center, "Center"), (.right, "Right"), (.none, "Default")] {
            let item = menuItem(title, Self.symbol(Self.alignSymbol(a))) { [weak self] in self?.onCommand?(.align(a)) }
            item.state = a == alignment ? .on : .off
            menu.addItem(item)
        }
        popUp(menu, from: alignButton)
    }

    private func showMoreMenu() {
        let menu = NSMenu()
        for op in [EditorController.TableOp.edit, .sortAsc, .sortDesc, .copyCSV] {
            menu.addItem(menuItem(op.title) { [weak self] in self?.onOperation?(op) })
        }
        menu.addItem(.separator())
        menu.addItem(menuItem(EditorController.TableOp.deleteTable.title) { [weak self] in self?.onOperation?(.deleteTable) })
        popUp(menu, from: moreButton)
    }

    // MARK: Icons

    static func alignSymbol(_ a: TableAlignment) -> String {
        switch a {
        case .left: return "text.alignleft"
        case .center: return "text.aligncenter"
        case .right: return "text.alignright"
        case .none: return "text.justify.leading"
        }
    }

    static func symbol(_ name: String) -> NSImage? {
        NSImage(systemSymbolName: name, accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 12, weight: .regular))
    }

    /// A 3×3 grid with the affected row or column filled, plus a + or − badge.
    static func gridIcon(for cmd: TableCommand) -> NSImage {
        NSImage(size: NSSize(width: 18, height: 16), flipped: true) { _ in
            let grid = NSRect(x: 1.5, y: 1.5, width: 12, height: 12)
            let cell = grid.width / 3
            let insert = cmd != .deleteRow && cmd != .deleteColumn
            let color: NSColor = insert ? .controlAccentColor : .systemRed
            let strip: NSRect
            switch cmd {
            case .insertRowAbove: strip = NSRect(x: grid.minX, y: grid.minY, width: grid.width, height: cell)
            case .insertRowBelow: strip = NSRect(x: grid.minX, y: grid.maxY - cell, width: grid.width, height: cell)
            case .insertColumnLeft: strip = NSRect(x: grid.minX, y: grid.minY, width: cell, height: grid.height)
            case .insertColumnRight: strip = NSRect(x: grid.maxX - cell, y: grid.minY, width: cell, height: grid.height)
            case .deleteRow: strip = NSRect(x: grid.minX, y: grid.minY + cell, width: grid.width, height: cell)
            default: strip = NSRect(x: grid.minX + cell, y: grid.minY, width: cell, height: grid.height)
            }
            color.withAlphaComponent(0.75).setFill()
            strip.fill()
            NSColor.labelColor.withAlphaComponent(0.7).setStroke()
            let path = NSBezierPath(rect: grid)
            for i in 1...2 {
                let o = CGFloat(i) * cell
                path.move(to: NSPoint(x: grid.minX + o, y: grid.minY)); path.line(to: NSPoint(x: grid.minX + o, y: grid.maxY))
                path.move(to: NSPoint(x: grid.minX, y: grid.minY + o)); path.line(to: NSPoint(x: grid.maxX, y: grid.minY + o))
            }
            path.lineWidth = 1
            path.stroke()
            // Badge
            let badge = NSRect(x: 11, y: 9, width: 7, height: 7)
            color.setFill()
            NSBezierPath(ovalIn: badge).fill()
            NSColor.white.setStroke()
            let sign = NSBezierPath()
            sign.lineWidth = 1.3
            sign.move(to: NSPoint(x: badge.minX + 1.8, y: badge.midY)); sign.line(to: NSPoint(x: badge.maxX - 1.8, y: badge.midY))
            if insert { sign.move(to: NSPoint(x: badge.midX, y: badge.minY + 1.8)); sign.line(to: NSPoint(x: badge.midX, y: badge.maxY - 1.8)) }
            sign.stroke()
            return true
        }
    }
}

/// A menu item that runs a closure.
final class ClosureMenuItem: NSMenuItem {
    private let handler: () -> Void

    init(title: String, action handler: @escaping () -> Void) {
        self.handler = handler
        super.init(title: title, action: #selector(run), keyEquivalent: "")
        target = self
    }

    required init(coder: NSCoder) { fatalError("init(coder:) is not used") }

    @objc private func run() { handler() }
}

extension EditorController {
    /// Shows, hides and positions the floating table toolbar. Visible only while the editor is editable and
    /// focused, has a single selection without marked text, and the caret is in a table.
    func updateTableToolbar() {
        let bar = tableToolbar
        guard AppSettings.shared.showTableToolbar, textView.isEditable, textView.window?.firstResponder === textView,
              textView.selectedRanges.count == 1, !textView.hasMarkedText(), let loc = tableLocation() else {
            bar.isHidden = true
            return
        }
        if bar.superview !== scrollView { scrollView.addSubview(bar, positioned: .above, relativeTo: nil) }
        bar.setAlignment(loc.table.alignments[loc.column])
        let top = rect(forCharacterAt: loc.range.location)
        // The last character, not the end: at the end of the document there is no layout fragment after the table.
        let bottom = rect(forCharacterAt: max(loc.range.location, NSMaxRange(loc.range) - 1))
        let visible = textView.visibleRect
        let size = bar.fittingSize
        // In text view (flipped) coordinates: above the table, else below it, else pinned to the top.
        var y = top.minY - size.height + 1
        if y < visible.minY {
            y = bottom.maxY + size.height <= visible.maxY && bottom.maxY >= visible.minY ? bottom.maxY : visible.minY + 2
        }
        let x = min(max(top.minX, visible.minX + 4), visible.maxX - size.width - 4)
        let inText = NSRect(x: x, y: y, width: size.width, height: size.height)
        bar.frame = scrollView.convert(inText, from: textView)
        bar.isHidden = false
    }
}
