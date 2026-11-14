import Foundation

/// Column alignment as written in a GFM table's delimiter row.
public enum TableAlignment: String, Codable, Sendable, CaseIterable {
    case none, left, center, right

    var delimiter: (Int) -> String {
        { width in
            let w = max(width, 3)
            switch self {
            case .none: return String(repeating: "-", count: w)
            case .left: return ":" + String(repeating: "-", count: w - 1)
            case .right: return String(repeating: "-", count: w - 1) + ":"
            case .center: return ":" + String(repeating: "-", count: max(w - 2, 1)) + ":"
            }
        }
    }
}

/// Display width helpers. Wide (CJK, fullwidth) characters and emoji take two columns.
public enum DisplayWidth {
    public static func of(_ s: String) -> Int {
        s.reduce(0) { $0 + of($1) }
    }

    public static func of(_ c: Character) -> Int {
        guard let first = c.unicodeScalars.first else { return 0 }
        let scalars = c.unicodeScalars
        // Emoji presentation (including ZWJ sequences, flags, keycaps with VS16).
        if scalars.contains(where: { $0.value == 0xFE0F }) { return 2 }
        if first.properties.isEmojiPresentation { return 2 }
        if scalars.count > 1 && scalars.contains(where: { $0.value == 0x200D }) { return 2 }
        if first.properties.generalCategory == .nonspacingMark || first.properties.generalCategory == .enclosingMark { return 0 }
        if first.value == 0x200B || first.value == 0x200D { return 0 }
        return isWide(first.value) ? 2 : 1
    }

    static func isWide(_ v: UInt32) -> Bool {
        switch v {
        case 0x1100...0x115F, 0x2E80...0x303E, 0x3041...0x33FF, 0x3400...0x4DBF,
             0x4E00...0x9FFF, 0xA000...0xA4CF, 0xAC00...0xD7A3, 0xF900...0xFAFF,
             0xFE30...0xFE4F, 0xFF00...0xFF60, 0xFFE0...0xFFE6,
             0x1F300...0x1F64F, 0x1F900...0x1F9FF, 0x20000...0x2FFFD, 0x30000...0x3FFFD:
            return true
        default:
            return false
        }
    }
}

/// An in-memory GFM table: one header row, alignments and body rows.
public struct MarkdownTable: Equatable, Sendable {
    public var header: [String]
    public var alignments: [TableAlignment]
    public var rows: [[String]]
    /// Leading whitespace of the header line, repeated on every formatted line.
    public var indent = ""
    /// The editor's width in columns, when lines wrap. A table whose aligned form is wider is written compactly
    /// (single spaces, short `---` dividers) so each row wraps cleanly. Nil: always align. Not part of equality.
    public var layoutWidth: Int?
    /// Columns keep at least these widths, and the table isn't written compactly (a table just inserted with
    /// `@t:` keeps its room to type until the caret leaves it). Not part of equality.
    public var minimumWidths: [Int]?

    public static func == (a: MarkdownTable, b: MarkdownTable) -> Bool {
        a.header == b.header && a.alignments == b.alignments && a.rows == b.rows && a.indent == b.indent
    }

    public init(header: [String], alignments: [TableAlignment]? = nil, rows: [[String]], indent: String = "") {
        self.header = header
        self.indent = indent
        self.alignments = alignments ?? Array(repeating: .none, count: header.count)
        self.rows = rows
        normalize()
    }

    public var columnCount: Int { header.count }

    /// The header followed by the body rows (row 0 is the header).
    public var allRows: [[String]] {
        get { [header] + rows }
        set {
            header = newValue.first ?? []
            rows = Array(newValue.dropFirst())
            normalize()
        }
    }

    public static func empty(rows: Int, columns: Int) -> MarkdownTable {
        let cols = max(columns, 1)
        return MarkdownTable(header: (1...cols).map { "Column \($0)" },
                             rows: Array(repeating: Array(repeating: "", count: cols), count: max(rows, 0)))
    }

    mutating func normalize() {
        let n = max(header.count, alignments.count, rows.map(\.count).max() ?? 0, 1)
        while header.count < n { header.append("") }
        while alignments.count < n { alignments.append(.none) }
        if alignments.count > n { alignments.removeLast(alignments.count - n) }
        rows = rows.map { r in
            var r = r
            while r.count < n { r.append("") }
            if r.count > n { r = Array(r.prefix(n)) }
            return r
        }
    }

    // MARK: Parsing

    /// Splits a table line into trimmed cells on unescaped pipes. Cell text (including `\|`) is kept as written.
    public static func splitRow(_ line: String) -> [String] {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        var body = Substring(trimmed)
        if body.hasPrefix("|") { body = body.dropFirst() }
        if trimmed.count > 1, trimmed.hasSuffix("|"), !trimmed.hasSuffix("\\|"), !body.isEmpty { body = body.dropLast() }
        var cells: [String] = []
        var current = ""
        var escaped = false
        for c in body {
            if escaped { current.append(c); escaped = false }
            else if c == "\\" { current.append(c); escaped = true }
            else if c == "|" { cells.append(current.trimmingCharacters(in: .whitespaces)); current = "" }
            else { current.append(c) }
        }
        cells.append(current.trimmingCharacters(in: .whitespaces))
        return cells
    }

    /// Whether the line contains a `|` that isn't escaped with a backslash.
    public static func hasUnescapedPipe(_ line: String) -> Bool {
        var escaped = false
        for c in line {
            if escaped { escaped = false } else if c == "\\" { escaped = true } else if c == "|" { return true }
        }
        return false
    }

    static func isFence(_ line: String) -> Bool {
        let t = line.trimmingCharacters(in: .whitespaces)
        return t.hasPrefix("```") || t.hasPrefix("~~~")
    }

    /// Alignments of a delimiter row such as `| --- | :-: |`, or nil if the line isn't one.
    public static func delimiterAlignments(_ line: String) -> [TableAlignment]? {
        guard hasUnescapedPipe(line) else { return nil }
        let cells = splitRow(line)
        guard cells.allSatisfy({ $0.range(of: "^:?-+:?$", options: .regularExpression) != nil }) else { return nil }
        return cells.map(alignment(of:))
    }

    public static func isDelimiterRow(_ line: String) -> Bool { delimiterAlignments(line) != nil }

    /// A line that can be part of a table: it has an unescaped pipe and isn't a code fence.
    public static func looksLikeRow(_ line: String) -> Bool {
        hasUnescapedPipe(line) && !isFence(line)
    }

    static func alignment(of cell: String) -> TableAlignment {
        let c = cell.trimmingCharacters(in: .whitespaces)
        switch (c.hasPrefix(":"), c.hasSuffix(":")) {
        case (true, true): return .center
        case (true, false): return .left
        case (false, true): return .right
        default: return .none
        }
    }

    /// Parses the lines of a table (header, delimiter, body). Returns nil if not a table.
    public static func parse(lines: [String]) -> MarkdownTable? {
        guard lines.count >= 2, looksLikeRow(lines[0]), isDelimiterRow(lines[1]) else { return nil }
        let header = splitRow(lines[0])
        let aligns = splitRow(lines[1]).map(alignment(of:))
        let rows = lines.dropFirst(2).filter { looksLikeRow($0) }.map(splitRow)
        let indent = String(lines[0].prefix { $0 == " " || $0 == "\t" })
        return MarkdownTable(header: header, alignments: aligns, rows: rows, indent: indent)
    }

    public static func parse(_ text: String) -> MarkdownTable? {
        parse(lines: text.components(separatedBy: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty })
    }

    // MARK: Formatting

    public func columnWidths() -> [Int] {
        (0..<columnCount).map { c in
            let content = max(3, ([header[c]] + rows.map { $0[c] }).map(DisplayWidth.of).max() ?? 3)
            return max(content, minimumWidths.flatMap { c < $0.count ? $0[c] : nil } ?? 0)
        }
    }

    static func pad(_ s: String, to width: Int, _ align: TableAlignment) -> String {
        let gap = max(0, width - DisplayWidth.of(s))
        switch align {
        case .right: return String(repeating: " ", count: gap) + s
        case .center:
            let l = gap / 2
            return String(repeating: " ", count: l) + s + String(repeating: " ", count: gap - l)
        default: return s + String(repeating: " ", count: gap)
        }
    }

    /// The width of the aligned table's rows, in columns.
    public var alignedWidth: Int {
        DisplayWidth.of(indent) + columnWidths().reduce(0, +) + 3 * columnCount + 1
    }

    /// True when the aligned table wouldn't fit `layoutWidth`, so it is written compactly.
    public var isCompact: Bool { minimumWidths == nil && (layoutWidth.map { alignedWidth > $0 } ?? false) }

    /// Widths cells are padded to: the column widths, or 0 (no padding) for a compact table.
    func layoutWidths() -> [Int] { isCompact ? Array(repeating: 0, count: columnCount) : columnWidths() }

    /// Renders aligned Markdown with padded cells (or compact Markdown for a table too wide for `layoutWidth`).
    public func formatted() -> String {
        let widths = layoutWidths()
        func line(_ cells: [String]) -> String {
            indent + "| " + cells.enumerated().map { Self.pad($0.element, to: widths[$0.offset], alignments[$0.offset]) }.joined(separator: " | ") + " |"
        }
        var out = [line(header)]
        out.append(indent + "| " + alignments.enumerated().map { $0.element.delimiter(max(3, widths[$0.offset])) }.joined(separator: " | ") + " |")
        out += rows.map(line)
        return out.joined(separator: "\n")
    }

    // MARK: Editing operations (row indices are body rows; -1 means header)

    public mutating func insertRow(at index: Int) {
        rows.insert(Array(repeating: "", count: columnCount), at: min(max(index, 0), rows.count))
    }

    public mutating func deleteRow(at index: Int) {
        guard rows.indices.contains(index) else { return }
        rows.remove(at: index)
    }

    public mutating func insertColumn(at index: Int) {
        let i = min(max(index, 0), columnCount)
        header.insert("", at: i)
        alignments.insert(.none, at: i)
        for r in rows.indices { rows[r].insert("", at: i) }
    }

    public mutating func deleteColumn(at index: Int) {
        guard columnCount > 1, header.indices.contains(index) else { return }
        header.remove(at: index)
        alignments.remove(at: index)
        for r in rows.indices { rows[r].remove(at: index) }
    }

    public mutating func moveRow(from: Int, to: Int) {
        guard rows.indices.contains(from), rows.indices.contains(to) else { return }
        rows.swapAt(from, to)
    }

    public mutating func moveColumn(from: Int, to: Int) {
        guard header.indices.contains(from), header.indices.contains(to) else { return }
        header.swapAt(from, to)
        alignments.swapAt(from, to)
        for r in rows.indices { rows[r].swapAt(from, to) }
    }

    public mutating func sort(byColumn c: Int, ascending: Bool) {
        guard header.indices.contains(c) else { return }
        rows.sort { a, b in
            let x = a[c], y = b[c]
            let result: ComparisonResult
            if let dx = Double(x.replacingOccurrences(of: ",", with: "")), let dy = Double(y.replacingOccurrences(of: ",", with: "")) {
                result = dx < dy ? .orderedAscending : (dx > dy ? .orderedDescending : .orderedSame)
            } else {
                result = x.localizedStandardCompare(y)
            }
            return ascending ? result == .orderedAscending : result == .orderedDescending
        }
    }

    // MARK: Conversions

    public func csv(separator: Character = ",") -> String {
        func esc(_ s: String) -> String {
            let raw = s.replacingOccurrences(of: "\\|", with: "|")
            if raw.contains(separator) || raw.contains("\"") || raw.contains("\n") {
                return "\"" + raw.replacingOccurrences(of: "\"", with: "\"\"") + "\""
            }
            return raw
        }
        return ([header] + rows).map { $0.map(esc).joined(separator: String(separator)) }.joined(separator: "\n")
    }

    /// Parses CSV (RFC 4180-ish) or TSV into rows.
    public static func parseDelimited(_ text: String, separator: Character) -> [[String]] {
        var rows: [[String]] = []
        var row: [String] = []
        var field = ""
        var inQuotes = false
        var it = Array(text.replacingOccurrences(of: "\r\n", with: "\n"))
        var i = 0
        while i < it.count {
            let c = it[i]
            if inQuotes {
                if c == "\"" {
                    if i + 1 < it.count && it[i + 1] == "\"" { field.append("\""); i += 1 } else { inQuotes = false }
                } else { field.append(c) }
            } else if c == "\"" && field.isEmpty {
                inQuotes = true
            } else if c == separator {
                row.append(field); field = ""
            } else if c == "\n" {
                row.append(field); rows.append(row); row = []; field = ""
            } else {
                field.append(c)
            }
            i += 1
        }
        it.removeAll()
        if !field.isEmpty || !row.isEmpty { row.append(field); rows.append(row) }
        return rows.filter { !($0.count == 1 && $0[0].isEmpty) }
    }

    /// Guesses whether text is TSV or CSV data with a consistent column count (>= 2 columns, >= 2 rows).
    public static func detectDelimiter(_ text: String) -> Character? {
        let lines = text.trimmingCharacters(in: .newlines).components(separatedBy: "\n")
        guard lines.count >= 2 else { return nil }
        for sep: Character in ["\t", ","] {
            let rows = parseDelimited(text.trimmingCharacters(in: .newlines), separator: sep)
            guard rows.count >= 2, let n = rows.first?.count, n >= 2 else { continue }
            if rows.allSatisfy({ $0.count == n }) { return sep }
        }
        return nil
    }

    public static func fromDelimited(_ text: String, separator: Character) -> MarkdownTable? {
        let rows = parseDelimited(text.trimmingCharacters(in: .newlines), separator: separator)
        guard let header = rows.first, header.count >= 1 else { return nil }
        let clean: ([String]) -> [String] = { $0.map { $0.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "|", with: "\\|").replacingOccurrences(of: "\n", with: " ") } }
        return MarkdownTable(header: clean(header), rows: rows.dropFirst().map(clean))
    }
}
