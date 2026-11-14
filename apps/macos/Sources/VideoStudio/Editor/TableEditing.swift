import AppKit
import MarkdownCore

/// Table navigation, formatting and the Table menu operations.
/// Structural commands come from `TableEditor` in MarkdownCore; this file applies them to the text view.
extension EditorController {
    /// The table around the caret, with its range in document coordinates. Nil in code blocks and outside tables.
    func tableLocation(at caret: Int? = nil) -> TableLocation? {
        let caret = caret ?? textView.selectedRange().location
        // During undo the selection can change before textDidChange refreshes `scan`; skip until it is current.
        guard let lastStart = scan.lineStarts.last, lastStart <= nsText.length, caret <= nsText.length else { return nil }
        let line = scan.line(at: caret)
        guard MarkdownTable.looksLikeRow(lineText(line)), !scan.isInCodeOrFrontMatter(line: line) else { return nil }
        let window = linesAround(line, radius: 400)
        guard let loc = TableEditor.locate(lines: window.lines, caretLine: line - window.offset, column: caret - scan.lineStarts[line]) else { return nil }
        let shifted = loc.shifted(lines: window.offset, characters: scan.lineStarts[window.offset]).withLayoutWidth(tableLayoutWidth)
        // A table just inserted with `@t:` keeps its starting widths until the caret leaves it.
        guard let keep = tableMinimums, keep.headerLine == shifted.headerLine else { return shifted }
        return shifted.withMinimumWidths(keep.widths)
    }

    /// The editor's visible width in characters at the current font.
    var editorColumns: Int {
        let charWidth = ("M" as NSString).size(withAttributes: [.font: highlighter.baseFont]).width
        guard charWidth > 0 else { return 80 }
        let pad = textView.textContainer?.lineFragmentPadding ?? 5
        var width = textView.enclosingScrollView?.contentSize.width ?? textView.bounds.width
        if AppSettings.shared.softWrap, let c = textView.textContainer, c.size.width > 0, c.size.width < 100_000 { width = c.size.width }
        width -= 2 * pad + 2 * textView.textContainerInset.width
        return max(20, Int(width / charWidth))
    }

    /// `@t:rows[,columns]` alone on the caret's line, then Tab or Space: inserts an empty table sized to the editor,
    /// with the caret in the first header cell, as one undo step (spec section 13a). Returns true when it did.
    func expandTableShortcut() -> Bool {
        guard textView.isEditable, textView.selectedRanges.count == 1, !textView.hasMarkedText() else { return false }
        let sel = textView.selectedRange()
        guard sel.length == 0, let lastStart = scan.lineStarts.last, lastStart <= nsText.length else { return false }
        let line = scan.line(at: sel.location)
        guard !scan.isInCodeOrFrontMatter(line: line) else { return false }
        var r = lineRange(line)
        if r.length > 0, nsText.character(at: NSMaxRange(r) - 1) == 10 { r.length -= 1 }
        guard NSMaxRange(r) == sel.location, let spec = TableShortcut.parse(nsText.substring(with: r)) else { return false }
        // Not inside an existing table (a line next to one would join it).
        if line > 0, MarkdownTable.looksLikeRow(lineText(line - 1)) { return false }
        let table = TableShortcut.table(spec, editorWidth: editorColumns)
        let block = table.formatted()
        let edit = MarkdownEditing.insertBlock(nsText, selection: r, block: block, selectInBlock: TableEditor.cellRange(in: table, row: 0, column: 0))
        let before = (edit.replacement as NSString).range(of: block).location
        let headerLine = line + (edit.replacement as NSString).substring(to: max(0, before)).filter { $0 == "\n" }.count
        startOwnUndoStep()
        // Undo puts the caret back after the shortcut (this runs last when undoing).
        let caretAfter = NSRange(location: sel.location, length: 0)
        textView.undoManager?.registerUndo(withTarget: textView) { tv in tv.setSelectedRange(caretAfter) }
        tableMinimums = (headerLine, table.minimumWidths ?? [])
        apply(edit, actionName: "Insert Table")
        return true
    }

    /// How many columns fit across the editor when lines wrap; tables wider than this are written compactly
    /// so each row wraps cleanly. Nil when soft wrap is off (wide tables just scroll).
    var tableLayoutWidth: Int? {
        guard AppSettings.shared.softWrap, let container = textView.textContainer else { return nil }
        let charWidth = ("M" as NSString).size(withAttributes: [.font: highlighter.baseFont]).width
        guard charWidth > 0 else { return nil }
        let usable = container.size.width - 2 * container.lineFragmentPadding
        guard usable > 0, usable < 100_000 else { return nil }
        return max(20, Int(usable / charWidth) - 1)
    }

    /// Runs a table command at the caret. Returns false (and does nothing) when it doesn't apply.
    @discardableResult
    func performTable(_ command: TableCommand) -> Bool {
        guard textView.isEditable, let loc = tableLocation(), let edit = TableEditor.apply(command, at: loc) else { return false }
        applyTableEdit(edit, actionName: command.title)
        return true
    }

    /// Replaces a table as one undo step, or only moves the selection when the text is unchanged
    /// (so Tab through a tidy table leaves no undo entries).
    func applyTableEdit(_ edit: TextEdit, actionName: String) {
        if NSMaxRange(edit.range) <= nsText.length, nsText.substring(with: edit.range) == edit.replacement {
            textView.setSelectedRange(edit.selection)
            textView.scrollRangeToVisible(edit.selection)
            return
        }
        apply(edit, actionName: actionName)
    }

    /// Formats the table on `lines` without moving the caret out of place (used when the caret leaves a table).
    func formatTable(lines: ClosedRange<Int>, keepCaret: Bool) {
        guard lines.upperBound < scan.lineStarts.count, let loc = tableLocation(at: scan.lineStarts[lines.lowerBound]) else { return }
        let formatted = loc.table.formatted()
        guard formatted != nsText.substring(with: loc.range) else { return }
        let sel = textView.selectedRange()
        var newSel = sel
        if sel.location >= NSMaxRange(loc.range) { newSel.location += (formatted as NSString).length - loc.range.length }
        else if sel.location > loc.range.location { newSel = NSRange(location: loc.range.location, length: 0) }
        apply(TextEdit(range: loc.range, replacement: formatted, selection: newSel), actionName: "Format Table")
    }

    /// Tab / Shift-Tab inside a table. Returns false if the caret isn't in a table or there are several selections.
    func tableTab(backward: Bool) -> Bool {
        guard textView.selectedRanges.count == 1 else { return false }
        return performTable(backward ? .previousCell : .nextCell)
    }

    /// Enter inside a table: same column in the next row, adding a row if needed.
    /// Enter on an empty last body row removes it and ends the table.
    func tableEnter() -> Bool {
        guard let loc = tableLocation() else { return false }
        var table = loc.table
        var rows = table.allRows
        let row = loc.onDelimiter ? 0 : loc.row
        if row == rows.count - 1, row > 0, rows[row].allSatisfy(\.isEmpty) {
            rows.removeLast()
            table.allRows = rows
            let formatted = table.formatted()
            apply(TextEdit(range: loc.range, replacement: formatted + "\n",
                           selection: NSRange(location: loc.range.location + (formatted as NSString).length + 1, length: 0)), actionName: "Typing")
            return true
        }
        if row + 1 >= rows.count { rows.append(Array(repeating: "", count: table.columnCount)) }
        table.allRows = rows
        let r = TableEditor.render(table, row: row + 1, column: loc.column)
        applyTableEdit(TextEdit(range: loc.range, replacement: r.text,
                                selection: NSRange(location: loc.range.location + r.selection.location, length: 0)), actionName: "Insert Row Below")
        return true
    }

    enum TableOp: String {
        case rowAbove, rowBelow, colLeft, colRight, deleteRow, deleteColumn, deleteTable
        case moveRowUp, moveRowDown, moveColLeft, moveColRight
        case alignLeft, alignCenter, alignRight, alignNone
        case sortAsc, sortDesc, format, copyCSV, edit

        var command: TableCommand? {
            switch self {
            case .rowAbove: return .insertRowAbove
            case .rowBelow: return .insertRowBelow
            case .colLeft: return .insertColumnLeft
            case .colRight: return .insertColumnRight
            case .deleteRow: return .deleteRow
            case .deleteColumn: return .deleteColumn
            case .moveRowUp: return .moveRowUp
            case .moveRowDown: return .moveRowDown
            case .moveColLeft: return .moveColumnLeft
            case .moveColRight: return .moveColumnRight
            case .alignLeft: return .align(.left)
            case .alignCenter: return .align(.center)
            case .alignRight: return .align(.right)
            case .alignNone: return .align(.none)
            case .format: return .format
            case .deleteTable, .sortAsc, .sortDesc, .copyCSV, .edit: return nil
            }
        }

        var title: String {
            if let command { return command.title }
            switch self {
            case .deleteTable: return "Delete Table"
            case .sortAsc: return "Sort Ascending by Column"
            case .sortDesc: return "Sort Descending by Column"
            case .copyCSV: return "Copy Table as CSV"
            default: return "Edit Table…"
            }
        }
    }

    func performTableOp(_ op: TableOp) {
        if let command = op.command { performTable(command); return }
        guard textView.isEditable || op == .copyCSV, let loc = tableLocation() else { return }
        var t = loc.table
        switch op {
        case .deleteTable:
            apply(TextEdit(range: loc.range, replacement: "", selection: NSRange(location: loc.range.location, length: 0)), actionName: "Delete Table")
        case .sortAsc, .sortDesc:
            t.sort(byColumn: loc.column, ascending: op == .sortAsc)
            let r = TableEditor.render(t, row: 0, column: loc.column)
            applyTableEdit(TextEdit(range: loc.range, replacement: r.text,
                                    selection: NSRange(location: loc.range.location + r.selection.location, length: 0)), actionName: op.title)
        case .copyCSV:
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(t.csv(), forType: .string)
        case .edit:
            TableEditorSheet.present(table: t, from: textView.window) { [weak self] edited in
                guard let self, let fresh = self.tableLocation() else { return }
                var edited = edited
                edited.indent = fresh.table.indent
                let r = TableEditor.render(edited, row: 0, column: 0)
                self.apply(TextEdit(range: fresh.range, replacement: r.text,
                                    selection: NSRange(location: fresh.range.location + r.selection.location, length: 0)), actionName: "Edit Table")
            }
        default:
            break
        }
    }

    /// Inserts a formatted table as its own block with "Column 1" selected.
    func insertTable(rows: Int, columns: Int) {
        let t = MarkdownTable.empty(rows: rows, columns: columns)
        apply(MarkdownEditing.insertBlock(nsText, selection: textView.selectedRange(), block: t.formatted(),
                                          selectInBlock: TableEditor.cellRange(in: t, row: 0, column: 0)), actionName: "Insert Table")
    }

    /// Converts the selected tab- or comma-separated lines into a table and selects it.
    func convertSelectionToTable() {
        let sel = textView.selectedRange()
        guard sel.length > 0, textView.isEditable, var t = TableEditor.convert(nsText.substring(with: sel)) else { return }
        t.layoutWidth = tableLayoutWidth
        let formatted = t.formatted()
        apply(TextEdit(range: sel, replacement: formatted, selection: NSRange(location: sel.location, length: (formatted as NSString).length)),
              actionName: "Convert to Table")
    }

    /// The Table submenu for the editor's context menu.
    func tableMenu() -> NSMenu {
        let m = NSMenu(title: "Table")
        func add(_ op: TableOp) {
            let item = NSMenuItem(title: op.title, action: #selector(MarkdownTextView.tableMenuAction(_:)), keyEquivalent: "")
            item.representedObject = op.rawValue
            m.addItem(item)
        }
        Self.tableMenuLayout.forEach { group in
            if !m.items.isEmpty { m.addItem(.separator()) }
            group.forEach(add)
        }
        return m
    }

    /// Groups of table operations shared by the Format › Table menu and the context menu.
    static let tableMenuLayout: [[TableOp]] = [
        [.edit, .format],
        [.rowAbove, .rowBelow, .colLeft, .colRight],
        [.deleteRow, .deleteColumn, .deleteTable],
        [.moveRowUp, .moveRowDown, .moveColLeft, .moveColRight],
        [.alignLeft, .alignCenter, .alignRight, .alignNone],
        [.sortAsc, .sortDesc],
        [.copyCSV],
    ]
}
