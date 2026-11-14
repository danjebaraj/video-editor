import Foundation

/// A structural or navigation command on the table around the caret.
public enum TableCommand: Equatable, Sendable {
    case insertRowAbove, insertRowBelow, insertColumnLeft, insertColumnRight
    case deleteRow, deleteColumn
    case moveRowUp, moveRowDown, moveColumnLeft, moveColumnRight
    case align(TableAlignment), format
    case nextCell, previousCell

    /// Title-case name, used for menu items and undo.
    public var title: String {
        switch self {
        case .insertRowAbove: return "Insert Row Above"
        case .insertRowBelow: return "Insert Row Below"
        case .insertColumnLeft: return "Insert Column Left"
        case .insertColumnRight: return "Insert Column Right"
        case .deleteRow: return "Delete Row"
        case .deleteColumn: return "Delete Column"
        case .moveRowUp: return "Move Row Up"
        case .moveRowDown: return "Move Row Down"
        case .moveColumnLeft: return "Move Column Left"
        case .moveColumnRight: return "Move Column Right"
        case .align(.left): return "Align Column Left"
        case .align(.center): return "Align Column Center"
        case .align(.right): return "Align Column Right"
        case .align(.none): return "Remove Column Alignment"
        case .format: return "Format Table"
        case .nextCell: return "Next Cell"
        case .previousCell: return "Previous Cell"
        }
    }

    /// Sentence-case name, used for tooltips ("Insert row above").
    public var sentenceTitle: String {
        let t = title
        return t.prefix(1) + t.dropFirst().lowercased()
    }
}

/// A table found around the caret.
public struct TableLocation: Equatable, Sendable {
    /// From the start of the header line to the end of the last body line (no trailing newline), UTF-16.
    public let range: NSRange
    public let table: MarkdownTable
    /// 0 = header; the delimiter row is not counted.
    public let row: Int
    public let column: Int
    /// Caret distance into the trimmed cell text (UTF-16).
    public let offset: Int
    public let onDelimiter: Bool
    /// Line index of the header within the lines that were searched.
    public let headerLine: Int
    /// Line index of the last body line within the lines that were searched.
    public let lastLine: Int

    /// The same location with the table laid out for an editor `width` columns wide (nil: always align).
    public func withLayoutWidth(_ width: Int?) -> TableLocation {
        var t = table
        t.layoutWidth = width
        return TableLocation(range: range, table: t, row: row, column: column, offset: offset,
                             onDelimiter: onDelimiter, headerLine: headerLine, lastLine: lastLine)
    }

    /// The same location with columns kept at least `widths` wide (nil: none).
    public func withMinimumWidths(_ widths: [Int]?) -> TableLocation {
        var t = table
        t.minimumWidths = widths
        return TableLocation(range: range, table: t, row: row, column: column, offset: offset,
                             onDelimiter: onDelimiter, headerLine: headerLine, lastLine: lastLine)
    }

    /// The same location in a larger document whose searched lines started at `line` / UTF-16 `offset`.
    public func shifted(lines line: Int, characters offset: Int) -> TableLocation {
        TableLocation(range: NSRange(location: range.location + offset, length: range.length), table: table, row: row, column: column,
                      offset: self.offset, onDelimiter: onDelimiter, headerLine: headerLine + line, lastLine: lastLine + line)
    }
}

/// Pure table editing: locating the table at the caret and applying commands.
/// Every command returns the fully formatted table, or nil when it doesn't apply.
public enum TableEditor {
    /// Finds the table around `caret` (UTF-16) in `source`. Returns nil inside fenced code or outside a table.
    public static func locate(in source: String, caret: Int) -> TableLocation? {
        let lines = source.components(separatedBy: "\n")
        var start = 0, caretLine = 0, column = caret
        for (i, l) in lines.enumerated() {
            let len = (l as NSString).length
            caretLine = i
            column = caret - start
            if caret <= start + len { break }
            start += len + 1
        }
        guard lines[..<caretLine].filter(MarkdownTable.isFence).count % 2 == 0 else { return nil }
        return locate(lines: lines, caretLine: caretLine, column: column)
    }

    /// Finds the table around a caret given as a line index and UTF-16 column within `lines`.
    /// Ranges are relative to the start of `lines[0]`. Fenced code isn't checked here; callers
    /// that pass a window of a larger document check it themselves.
    public static func locate(lines: [String], caretLine: Int, column: Int) -> TableLocation? {
        guard lines.indices.contains(caretLine), MarkdownTable.looksLikeRow(lines[caretLine]) else { return nil }
        var first = caretLine, last = caretLine
        while first > 0 && MarkdownTable.looksLikeRow(lines[first - 1]) { first -= 1 }
        while last + 1 < lines.count && MarkdownTable.looksLikeRow(lines[last + 1]) { last += 1 }
        let upper = min(last, caretLine + 1)
        guard first + 1 <= upper,
              let delimiter = (first + 1...upper).last(where: { MarkdownTable.isDelimiterRow(lines[$0]) }) else { return nil }
        let header = delimiter - 1
        guard header <= caretLine, let aligns = MarkdownTable.delimiterAlignments(lines[delimiter]) else { return nil }
        let body = delimiter < last ? lines[(delimiter + 1)...last].map(MarkdownTable.splitRow) : []
        let indent = String(lines[header].prefix { $0 == " " || $0 == "\t" })
        let table = MarkdownTable(header: MarkdownTable.splitRow(lines[header]), alignments: aligns, rows: body, indent: indent)

        let line = lines[caretLine]
        let (cell, cellStart) = cellAt(line, column: column, columnCount: table.columnCount)
        let onDelimiter = caretLine == delimiter
        let row = caretLine <= delimiter ? 0 : caretLine - header - 1

        var start = 0
        for i in 0..<header { start += (lines[i] as NSString).length + 1 }
        var end = start
        for i in header...last { end += (lines[i] as NSString).length + (i < last ? 1 : 0) }
        return TableLocation(range: NSRange(location: start, length: end - start), table: table, row: row, column: cell,
                             offset: max(0, column - cellStart), onDelimiter: onDelimiter, headerLine: header, lastLine: last)
    }

    /// The cell index for a UTF-16 column in a row line, and where that cell's text starts (after leading spaces).
    static func cellAt(_ line: String, column: Int, columnCount: Int) -> (cell: Int, textStart: Int) {
        let ns = line as NSString
        var pipes: [Int] = []
        var escaped = false
        for i in 0..<ns.length {
            let ch = ns.character(at: i)
            if escaped { escaped = false } else if ch == 92 { escaped = true } else if ch == 124 { pipes.append(i) }
        }
        let leading = line.trimmingCharacters(in: .whitespaces).hasPrefix("|")
        let before = pipes.filter { $0 < column }.count
        let cell = min(max(before - (leading ? 1 : 0), 0), max(columnCount - 1, 0))
        // The cell starts after pipe number `cell` (counting the leading pipe), or at the line start.
        let boundary = cell + (leading ? 1 : 0)
        var start = boundary == 0 ? 0 : (boundary <= pipes.count ? pipes[boundary - 1] + 1 : ns.length)
        while start < ns.length, ns.character(at: start) == 32 || ns.character(at: start) == 9 { start += 1 }
        return (cell, start)
    }

    /// Applies a command to the table around the selection in `source`.
    public static func apply(_ command: TableCommand, to source: String, selection: NSRange) -> TextEdit? {
        guard let location = locate(in: source, caret: selection.location) else { return nil }
        return apply(command, at: location)
    }

    /// Applies a command to a located table. The edit's range is `location.range`.
    public static func apply(_ command: TableCommand, at location: TableLocation) -> TextEdit? {
        var table = location.table
        var rows = table.allRows
        let count = table.columnCount
        var row = location.row, column = location.column
        var offset = 0
        var select = false
        switch command {
        case .format:
            offset = location.offset
        case .insertRowAbove, .insertRowBelow:
            let at = location.onDelimiter ? 1 : (command == .insertRowAbove ? row : row + 1)
            rows.insert(Array(repeating: "", count: count), at: at)
            row = at
        case .deleteRow:
            guard !location.onDelimiter, rows.count > 1 else { return nil }
            rows.remove(at: row)
            row = min(row, rows.count - 1)
        case .moveRowUp:
            guard !location.onDelimiter, row > 0 else { return nil }
            rows.swapAt(row, row - 1)
            row -= 1
            offset = location.offset
        case .moveRowDown:
            guard !location.onDelimiter, row + 1 < rows.count else { return nil }
            rows.swapAt(row, row + 1)
            row += 1
            offset = location.offset
        case .insertColumnLeft, .insertColumnRight:
            if command == .insertColumnRight { column += 1 }
            for r in rows.indices { rows[r].insert("", at: column) }
            table.alignments.insert(.none, at: column)
        case .deleteColumn:
            guard count > 1 else { return nil }
            for r in rows.indices { rows[r].remove(at: column) }
            table.alignments.remove(at: column)
            column = min(column, count - 2)
        case .moveColumnLeft, .moveColumnRight:
            let to = command == .moveColumnLeft ? column - 1 : column + 1
            guard (0..<count).contains(to) else { return nil }
            for r in rows.indices { rows[r].swapAt(column, to) }
            table.alignments.swapAt(column, to)
            column = to
            offset = location.offset
        case .align(let a):
            table.alignments[column] = a
            offset = location.offset
        case .nextCell:
            select = true
            if location.onDelimiter {
                row = 1; column = 0
            } else if column + 1 < count {
                column += 1
            } else {
                row += 1; column = 0
            }
            if row >= rows.count { rows.append(Array(repeating: "", count: count)) }
        case .previousCell:
            select = true
            if location.onDelimiter {
                row = 0; column = count - 1
            } else if column > 0 {
                column -= 1
            } else if row > 0 {
                row -= 1; column = count - 1
            }
        }
        table.allRows = rows
        let (text, selection) = render(table, row: row, column: column, offset: offset, select: select)
        return TextEdit(range: location.range, replacement: text,
                        selection: NSRange(location: location.range.location + selection.location, length: selection.length))
    }

    /// Formats `table` and returns the caret (or cell selection) for a cell, relative to the formatted text.
    public static func render(_ table: MarkdownTable, row: Int, column: Int, offset: Int = 0, select: Bool = false) -> (text: String, selection: NSRange) {
        let text = table.formatted()
        let cell = cellRange(in: table, row: row, column: column)
        if select && cell.length > 0 { return (text, cell) }
        return (text, NSRange(location: cell.location + min(offset, cell.length), length: 0))
    }

    /// Where a cell's text sits in `table.formatted()` (UTF-16): start and length, without padding.
    /// An empty cell starts right after "| " whatever the alignment.
    public static func cellRange(in table: MarkdownTable, row: Int, column: Int) -> NSRange {
        let rows = table.allRows
        let widths = table.layoutWidths()
        let r = min(max(row, 0), rows.count - 1)
        let c = min(max(column, 0), table.columnCount - 1)
        let lineIndex = r == 0 ? 0 : r + 1
        var location = 0
        for (i, line) in table.formatted().components(separatedBy: "\n").enumerated() where i < lineIndex {
            location += (line as NSString).length + 1
        }
        location += (table.indent as NSString).length + 2
        for k in 0..<c {
            location += (MarkdownTable.pad(rows[r][k], to: widths[k], table.alignments[k]) as NSString).length + 3
        }
        let text = rows[r][c]
        if !text.isEmpty {
            let padded = MarkdownTable.pad(text, to: widths[c], table.alignments[c])
            location += (padded as NSString).length - (padded.drop { $0 == " " } as Substring).utf16.count
        }
        return NSRange(location: location, length: (text as NSString).length)
    }

    /// Splits CSV or TSV text into a formatted table: tab-separated if the text contains a tab, otherwise comma-separated.
    public static func convert(_ selection: String) -> MarkdownTable? {
        let lines = selection.components(separatedBy: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
        guard !lines.isEmpty else { return nil }
        let separator: Character = selection.contains("\t") ? "\t" : ","
        guard let table = MarkdownTable.fromDelimited(lines.joined(separator: "\n"), separator: separator) else { return nil }
        return table
    }
}
