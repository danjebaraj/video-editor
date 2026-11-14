import Foundation

/// `@t:rows[,columns]` then Tab or Space inserts an empty table (spec section 13a).
public enum TableShortcut {
    public struct Spec: Equatable, Sendable {
        /// Body rows (not counting the header) and columns.
        public var rows: Int
        public var columns: Int
        /// The line's leading whitespace, kept on every table line.
        public var indent: String
    }

    public static let rowLimit = 1...50
    public static let columnLimit = 1...20
    public static let minimumCellWidth = 8
    public static let maximumCellWidth = 24

    private static let rx = try! NSRegularExpression(pattern: "^([ \\t]*)@t:([0-9]{1,4})(?:[ \\t]*,[ \\t]*([0-9]{1,4}))?$")

    /// The shortcut on a line, when it's the only thing there (apart from indentation) and the size is in range.
    public static func parse(_ line: String) -> Spec? {
        guard line.contains("@t:"), let m = rx.firstMatch(in: line, range: NSRange(location: 0, length: (line as NSString).length)) else { return nil }
        let ns = line as NSString
        guard let rows = Int(ns.substring(with: m.range(at: 2))) else { return nil }
        let columns = m.range(at: 3).location == NSNotFound ? 1 : Int(ns.substring(with: m.range(at: 3))) ?? 0
        guard rowLimit.contains(rows), columnLimit.contains(columns) else { return nil }
        return Spec(rows: rows, columns: columns, indent: ns.substring(with: m.range(at: 1)))
    }

    /// Each cell's width for an editor `editorWidth` characters wide: the width shared evenly between the columns
    /// after the `|` separators and padding, kept within 8…24.
    public static func cellWidth(columns: Int, editorWidth: Int, indent: Int = 0) -> Int {
        let cols = max(columns, 1)
        // A row is `| a | b |`: 1 + columns × (width + 3), after the indentation.
        let share = (editorWidth - indent - 1) / cols - 3
        return min(max(share, minimumCellWidth), maximumCellWidth)
    }

    /// The empty table for `spec`: an empty header, plain `---` delimiters, every column `cellWidth` wide.
    public static func table(_ spec: Spec, editorWidth: Int) -> MarkdownTable {
        let width = cellWidth(columns: spec.columns, editorWidth: editorWidth, indent: DisplayWidth.of(spec.indent))
        var t = MarkdownTable(header: Array(repeating: "", count: spec.columns),
                              rows: Array(repeating: Array(repeating: "", count: spec.columns), count: spec.rows), indent: spec.indent)
        t.minimumWidths = Array(repeating: width, count: spec.columns)
        return t
    }
}
