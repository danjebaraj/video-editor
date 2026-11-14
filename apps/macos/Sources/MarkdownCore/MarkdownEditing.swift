import Foundation

/// A single text replacement plus the selection to apply afterwards (UTF-16 ranges).
public struct TextEdit: Equatable, Sendable {
    public var range: NSRange
    public var replacement: String
    public var selection: NSRange

    public init(range: NSRange, replacement: String, selection: NSRange) {
        self.range = range
        self.replacement = replacement
        self.selection = selection
    }
}

/// Pure text transformations behind formatting commands and smart editing.
public enum MarkdownEditing {
    // MARK: Inline formatting

    /// Wraps the selection in `marker…marker`, or unwraps it if already wrapped.
    /// With an empty selection, inserts a selected placeholder.
    public static func toggleInline(_ text: NSString, selection sel: NSRange, marker: String, placeholder: String) -> TextEdit {
        let m = marker as NSString
        let ml = m.length
        // Already wrapped outside the selection: **[sel]**
        if sel.location >= ml, NSMaxRange(sel) + ml <= text.length,
           text.substring(with: NSRange(location: sel.location - ml, length: ml)) == marker,
           text.substring(with: NSRange(location: NSMaxRange(sel), length: ml)) == marker, sel.length > 0 {
            let outer = NSRange(location: sel.location - ml, length: sel.length + 2 * ml)
            return TextEdit(range: outer, replacement: text.substring(with: sel), selection: NSRange(location: outer.location, length: sel.length))
        }
        let selected = text.substring(with: sel)
        // Selection includes markers: [**text**]
        if sel.length >= 2 * ml, selected.hasPrefix(marker), selected.hasSuffix(marker) {
            let inner = (selected as NSString).substring(with: NSRange(location: ml, length: sel.length - 2 * ml))
            return TextEdit(range: sel, replacement: inner, selection: NSRange(location: sel.location, length: (inner as NSString).length))
        }
        if sel.length == 0 {
            let rep = marker + placeholder + marker
            return TextEdit(range: sel, replacement: rep, selection: NSRange(location: sel.location + ml, length: (placeholder as NSString).length))
        }
        // Keep surrounding whitespace outside the markers.
        let trimmed = selected.trimmingCharacters(in: .whitespaces)
        let lead = (selected as NSString).range(of: trimmed).location
        let leading = (selected as NSString).substring(to: lead == NSNotFound ? 0 : lead)
        let trailing = (selected as NSString).substring(from: lead == NSNotFound ? 0 : lead + (trimmed as NSString).length)
        let rep = leading + marker + trimmed + marker + trailing
        return TextEdit(range: sel, replacement: rep, selection: NSRange(location: sel.location + (leading as NSString).length + ml, length: (trimmed as NSString).length))
    }

    /// Builds a `[text](url)` link from the selection.
    public static func link(_ text: NSString, selection sel: NSRange, url: String?) -> TextEdit {
        let label = sel.length > 0 ? text.substring(with: sel) : "link text"
        if let url, !url.isEmpty {
            let rep = "[\(label)](\(url))"
            if sel.length == 0 {
                return TextEdit(range: sel, replacement: rep, selection: NSRange(location: sel.location + 1, length: (label as NSString).length))
            }
            return TextEdit(range: sel, replacement: rep, selection: NSRange(location: sel.location + (rep as NSString).length, length: 0))
        }
        let rep = "[\(label)](url)"
        let urlLoc = sel.location + (label as NSString).length + 3
        if sel.length == 0 {
            return TextEdit(range: sel, replacement: rep, selection: NSRange(location: sel.location + 1, length: (label as NSString).length))
        }
        return TextEdit(range: sel, replacement: rep, selection: NSRange(location: urlLoc, length: 3))
    }

    // MARK: Line-based formatting

    /// The full-line range covering the selection (without the final newline).
    public static func lineRange(_ text: NSString, for sel: NSRange) -> NSRange {
        var r = text.lineRange(for: sel)
        if r.length > 0, NSMaxRange(r) <= text.length, text.character(at: NSMaxRange(r) - 1) == 10 { r.length -= 1 }
        // When the selection ends exactly at the start of a line, exclude that line.
        if sel.length > 0, NSMaxRange(sel) > 0, NSMaxRange(sel) <= text.length, text.character(at: NSMaxRange(sel) - 1) == 10, NSMaxRange(sel) - 1 < NSMaxRange(r) {
            r.length = NSMaxRange(sel) - 1 - r.location
        }
        return r
    }

    static let headingRegex = try! NSRegularExpression(pattern: "^(#{1,6})\\s+")
    static let listRegex = try! NSRegularExpression(pattern: "^(\\s*)([-*+]|\\d+[.)])(\\s+)(\\[[ xX]\\]\\s+)?")
    static let quoteRegex = try! NSRegularExpression(pattern: "^(\\s*)>\\s?")

    static func strippedHeading(_ line: String) -> String {
        let ns = line as NSString
        if let m = headingRegex.firstMatch(in: line, range: NSRange(location: 0, length: ns.length)) {
            return ns.substring(from: m.range.length)
        }
        return line
    }

    /// Sets heading level (0 removes the heading) on every line in the selection. Toggling the same level removes it.
    public static func setHeading(_ text: NSString, selection sel: NSRange, level: Int) -> TextEdit {
        let r = lineRange(text, for: sel)
        let lines = text.substring(with: r).components(separatedBy: "\n")
        let allSame = level > 0 && lines.allSatisfy { $0.hasPrefix(String(repeating: "#", count: level) + " ") && !$0.hasPrefix(String(repeating: "#", count: level + 1)) }
        let out = lines.map { line -> String in
            let body = strippedHeading(line)
            if level == 0 || allSame { return body }
            return String(repeating: "#", count: level) + " " + body
        }.joined(separator: "\n")
        if sel.length == 0 && lines.count == 1 {
            let delta = (out as NSString).length - r.length
            let caret = max(r.location, min(sel.location + delta, r.location + (out as NSString).length))
            return TextEdit(range: r, replacement: out, selection: NSRange(location: caret, length: 0))
        }
        return TextEdit(range: r, replacement: out, selection: NSRange(location: r.location, length: (out as NSString).length))
    }

    public enum LineStyle: Sendable { case bullet, numbered, task, quote }

    static func matchesStyle(_ line: String, _ style: LineStyle) -> Bool {
        let t = line.trimmingCharacters(in: .whitespaces)
        switch style {
        case .bullet: return (t.hasPrefix("- ") || t.hasPrefix("* ") || t.hasPrefix("+ ")) && !isTask(t)
        case .numbered: return t.range(of: "^\\d+[.)]\\s", options: .regularExpression) != nil
        case .task: return isTask(t)
        case .quote: return t.hasPrefix(">")
        }
    }

    static func isTask(_ t: String) -> Bool {
        t.range(of: "^[-*+]\\s+\\[[ xX]\\]\\s", options: .regularExpression) != nil
    }

    static func stripPrefix(_ line: String) -> (indent: String, body: String) {
        let ns = line as NSString
        let full = NSRange(location: 0, length: ns.length)
        if let m = listRegex.firstMatch(in: line, range: full) {
            return (ns.substring(with: m.range(at: 1)), ns.substring(from: m.range.length))
        }
        if let m = quoteRegex.firstMatch(in: line, range: full) {
            return (ns.substring(with: m.range(at: 1)), ns.substring(from: m.range.length))
        }
        let indent = String(line.prefix { $0 == " " || $0 == "\t" })
        return (indent, String(line.dropFirst(indent.count)))
    }

    /// Toggles a list/quote prefix on each selected line.
    public static func toggleLineStyle(_ text: NSString, selection sel: NSRange, style: LineStyle) -> TextEdit {
        let r = lineRange(text, for: sel)
        let lines = text.substring(with: r).components(separatedBy: "\n")
        let nonEmpty = lines.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
        let remove = !nonEmpty.isEmpty && nonEmpty.allSatisfy { matchesStyle($0, style) }
        var n = 0
        let out = lines.map { line -> String in
            if line.trimmingCharacters(in: .whitespaces).isEmpty && lines.count > 1 { return line }
            let (indent, body) = stripPrefix(line)
            if remove { return indent + body }
            switch style {
            case .bullet: return indent + "- " + body
            case .numbered: n += 1; return indent + "\(n). " + body
            case .task: return indent + "- [ ] " + body
            case .quote: return indent + "> " + body
            }
        }.joined(separator: "\n")
        let outLen = (out as NSString).length
        if sel.length == 0 && lines.count == 1 {
            let delta = outLen - r.length
            return TextEdit(range: r, replacement: out, selection: NSRange(location: max(r.location, min(sel.location + delta, r.location + outLen)), length: 0))
        }
        return TextEdit(range: r, replacement: out, selection: NSRange(location: r.location, length: outLen))
    }

    /// Toggles `[ ]` ↔ `[x]` on the caret's line; converts a plain line or bullet into a task.
    public static func toggleTask(_ text: NSString, caret: Int) -> TextEdit {
        let r = lineRange(text, for: NSRange(location: caret, length: 0))
        let line = text.substring(with: r)
        var out: String
        if let range = line.range(of: "^\\s*[-*+]\\s+\\[ \\]", options: .regularExpression) {
            out = line.replacingCharacters(in: range, with: line[range].replacingOccurrences(of: "[ ]", with: "[x]"))
        } else if let range = line.range(of: "^\\s*[-*+]\\s+\\[[xX]\\]", options: .regularExpression) {
            out = line.replacingCharacters(in: range, with: line[range].replacingOccurrences(of: "[x]", with: "[ ]").replacingOccurrences(of: "[X]", with: "[ ]"))
        } else {
            let (indent, body) = stripPrefix(line)
            out = indent + "- [ ] " + body
        }
        let delta = (out as NSString).length - r.length
        return TextEdit(range: r, replacement: out, selection: NSRange(location: min(max(caret + delta, r.location), r.location + (out as NSString).length), length: 0))
    }

    /// Wraps lines in a fenced code block.
    public static func codeBlock(_ text: NSString, selection sel: NSRange, language: String = "") -> TextEdit {
        if sel.length == 0 {
            let prefix = needsLeadingNewline(text, at: sel.location) ? "\n" : ""
            let rep = prefix + "```\(language)\n\n```\n"
            let caret = sel.location + (prefix as NSString).length + 4 + (language as NSString).length
            return TextEdit(range: sel, replacement: rep, selection: NSRange(location: caret, length: 0))
        }
        let r = lineRange(text, for: sel)
        let body = text.substring(with: r)
        let rep = "```\(language)\n\(body)\n```"
        return TextEdit(range: r, replacement: rep, selection: NSRange(location: r.location + 3, length: (language as NSString).length))
    }

    static func needsLeadingNewline(_ text: NSString, at loc: Int) -> Bool {
        loc > 0 && text.character(at: loc - 1) != 10
    }

    /// Inserts a block (e.g. a table or rule) on its own lines at the caret.
    public static func insertBlock(_ text: NSString, selection sel: NSRange, block: String, selectInBlock: NSRange? = nil) -> TextEdit {
        var prefix = ""
        if sel.location > 0 && text.character(at: sel.location - 1) != 10 { prefix = "\n\n" }
        else if sel.location > 1 && text.character(at: sel.location - 2) != 10 { prefix = "\n" }
        var suffix = "\n"
        if NSMaxRange(sel) < text.length && text.character(at: NSMaxRange(sel)) != 10 { suffix = "\n\n" }
        let rep = prefix + block + suffix
        let base = sel.location + (prefix as NSString).length
        let selection = selectInBlock.map { NSRange(location: base + $0.location, length: $0.length) } ?? NSRange(location: base + (block as NSString).length, length: 0)
        return TextEdit(range: sel, replacement: rep, selection: selection)
    }

    // MARK: Smart editing

    /// What Enter should do on the current line, if it is a list item.
    /// Returns nil when normal newline behaviour applies.
    public static func listContinuation(_ text: NSString, caret: Int) -> TextEdit? {
        let r = lineRange(text, for: NSRange(location: caret, length: 0))
        let line = text.substring(with: r)
        let ns = line as NSString
        guard let m = listRegex.firstMatch(in: line, range: NSRange(location: 0, length: ns.length)) else {
            // Blockquote continuation.
            if let q = quoteRegex.firstMatch(in: line, range: NSRange(location: 0, length: ns.length)) {
                if ns.length == q.range.length {
                    return TextEdit(range: r, replacement: "", selection: NSRange(location: r.location, length: 0))
                }
                let prefix = ns.substring(with: q.range)
                let rest = ns.substring(from: caret - r.location)
                let before = ns.substring(to: caret - r.location)
                let rep = before + "\n" + prefix + rest
                return TextEdit(range: r, replacement: rep, selection: NSRange(location: r.location + (before as NSString).length + 1 + (prefix as NSString).length, length: 0))
            }
            return nil
        }
        guard caret - r.location >= m.range.length else { return nil }
        let indent = ns.substring(with: m.range(at: 1))
        let marker = ns.substring(with: m.range(at: 2))
        let space = ns.substring(with: m.range(at: 3))
        let hasTask = m.range(at: 4).location != NSNotFound
        let content = ns.substring(from: m.range.length)
        if content.trimmingCharacters(in: .whitespaces).isEmpty {
            // Empty item: outdent if nested, otherwise end the list.
            if !indent.isEmpty {
                let newIndent = String(indent.dropFirst(min(indent.hasPrefix("\t") ? 1 : 4, indent.count)))
                let newLine = newIndent + ns.substring(from: m.range(at: 1).length)
                return TextEdit(range: r, replacement: newLine, selection: NSRange(location: r.location + (newLine as NSString).length, length: 0))
            }
            return TextEdit(range: r, replacement: "", selection: NSRange(location: r.location, length: 0))
        }
        var nextMarker = marker
        if let num = Int(marker.dropLast()) { nextMarker = "\(num + 1)\(marker.last!)" }
        let prefix = indent + nextMarker + space + (hasTask ? "[ ] " : "")
        let before = ns.substring(to: caret - r.location)
        let after = ns.substring(from: caret - r.location)
        var rep = before + "\n" + prefix + after
        // Renumber following siblings for ordered lists.
        var replaceRange = r
        if Int(marker.dropLast()) != nil {
            let (renumbered, consumed) = renumberFollowing(text, after: NSMaxRange(r), indent: indent, startingAt: (Int(nextMarker.dropLast()) ?? 0) + 1)
            if consumed > 0 {
                rep += renumbered
                replaceRange = NSRange(location: r.location, length: r.length + consumed)
            }
        }
        return TextEdit(range: replaceRange, replacement: rep, selection: NSRange(location: r.location + (before as NSString).length + 1 + (prefix as NSString).length, length: 0))
    }

    /// Renumbers consecutive ordered items at `indent` after a location. Returns replacement text and consumed length.
    static func renumberFollowing(_ text: NSString, after loc: Int, indent: String, startingAt start: Int) -> (String, Int) {
        var pos = loc
        var n = start
        var out = ""
        var changed = false
        while pos < text.length && text.character(at: pos) == 10 {
            let lineStart = pos + 1
            var lineEnd = lineStart
            while lineEnd < text.length && text.character(at: lineEnd) != 10 { lineEnd += 1 }
            let line = text.substring(with: NSRange(location: lineStart, length: lineEnd - lineStart))
            let ns = line as NSString
            if let m = listRegex.firstMatch(in: line, range: NSRange(location: 0, length: ns.length)),
               ns.substring(with: m.range(at: 1)) == indent,
               Int(ns.substring(with: m.range(at: 2)).dropLast()) != nil {
                let marker = ns.substring(with: m.range(at: 2))
                let newLine = indent + "\(n)\(marker.last!)" + ns.substring(from: NSMaxRange(m.range(at: 2)))
                if newLine != line { changed = true }
                out += "\n" + newLine
                n += 1
            } else if !line.trimmingCharacters(in: .whitespaces).isEmpty && (line.hasPrefix(indent + "  ") || line.hasPrefix(indent + "\t")) {
                out += "\n" + line
            } else {
                break
            }
            pos = lineEnd
        }
        return changed ? (out, pos - loc) : ("", 0)
    }

    /// Indents (or outdents) the selected list lines by one level (4 spaces or a tab).
    public static func shiftLines(_ text: NSString, selection sel: NSRange, outdent: Bool, unit: String = "    ") -> TextEdit {
        let r = lineRange(text, for: sel)
        let lines = text.substring(with: r).components(separatedBy: "\n")
        var firstDelta = 0
        let out = lines.enumerated().map { (i, line) -> String in
            var new: String
            if outdent {
                if line.hasPrefix("\t") { new = String(line.dropFirst()) }
                else {
                    let spaces = line.prefix { $0 == " " }.count
                    new = String(line.dropFirst(min(spaces, unit.count)))
                }
            } else {
                new = line.isEmpty && lines.count > 1 ? line : unit + line
            }
            if i == 0 { firstDelta = (new as NSString).length - (line as NSString).length }
            return new
        }.joined(separator: "\n")
        let outLen = (out as NSString).length
        if sel.length == 0 {
            return TextEdit(range: r, replacement: out, selection: NSRange(location: max(r.location, sel.location + firstDelta), length: 0))
        }
        return TextEdit(range: r, replacement: out, selection: NSRange(location: r.location, length: outLen))
    }

    public static func isListLine(_ line: String) -> Bool {
        listRegex.firstMatch(in: line, range: NSRange(location: 0, length: (line as NSString).length)) != nil
    }

    /// Moves the selected lines up or down by one line.
    public static func moveLines(_ text: NSString, selection sel: NSRange, up: Bool) -> TextEdit? {
        let r = text.lineRange(for: sel)
        if up {
            guard r.location > 0 else { return nil }
            let prev = text.lineRange(for: NSRange(location: r.location - 1, length: 0))
            var block = text.substring(with: r)
            var prevText = text.substring(with: prev)
            if !block.hasSuffix("\n") { block += "\n"; prevText = String(prevText.dropLast()) }
            let total = NSRange(location: prev.location, length: prev.length + r.length)
            return TextEdit(range: total, replacement: block + prevText, selection: NSRange(location: sel.location - prev.length, length: sel.length))
        } else {
            guard NSMaxRange(r) < text.length else { return nil }
            let next = text.lineRange(for: NSRange(location: NSMaxRange(r), length: 0))
            var block = text.substring(with: r)
            var nextText = text.substring(with: next)
            if !nextText.hasSuffix("\n") { nextText += "\n"; block = String(block.dropLast()) }
            let total = NSRange(location: r.location, length: r.length + next.length)
            return TextEdit(range: total, replacement: nextText + block, selection: NSRange(location: sel.location + (nextText as NSString).length, length: sel.length))
        }
    }

    // MARK: Generated content

    public struct Heading: Equatable, Sendable {
        public var level: Int
        public var text: String
        public var line: Int
        public var location: Int

        public init(level: Int, text: String, line: Int, location: Int) {
            self.level = level; self.text = text; self.line = line; self.location = location
        }
    }

    /// Fast heading scan (ATX headings outside code fences) for the outline and TOC.
    public static func headings(in text: String) -> [Heading] {
        var result: [Heading] = []
        let scan = BlockScanner.scan(text)
        let ns = text as NSString
        for (i, start) in scan.lineStarts.enumerated() {
            if scan.isInCodeOrFrontMatter(line: i) { continue }
            guard start < ns.length, ns.character(at: start) == 35 else { continue }
            let end = i + 1 < scan.lineStarts.count ? scan.lineStarts[i + 1] : ns.length
            let line = ns.substring(with: NSRange(location: start, length: end - start)).trimmingCharacters(in: .newlines)
            let hashes = line.prefix { $0 == "#" }.count
            guard hashes <= 6, line.count == hashes || line.dropFirst(hashes).first == " " else { continue }
            var title = line.dropFirst(hashes).trimmingCharacters(in: .whitespaces)
            while title.hasSuffix("#") { title = String(title.dropLast()).trimmingCharacters(in: .whitespaces) }
            if title.isEmpty { continue }
            result.append(Heading(level: hashes, text: title, line: i, location: start))
        }
        return result
    }

    public static func tableOfContents(for text: String) -> String {
        let hs = headings(in: text)
        guard let minLevel = hs.map(\.level).min() else { return "" }
        var seen: [String: Int] = [:]
        return hs.map { h in
            var slug = WikiLinkResolver.slug(plainHeading(h.text))
            if let n = seen[slug] { seen[slug] = n + 1; slug += "-\(n + 1)" } else { seen[slug] = 0 }
            return String(repeating: "  ", count: h.level - minLevel) + "- [\(plainHeading(h.text))](#\(slug))"
        }.joined(separator: "\n")
    }

    /// Strips simple inline Markdown from a heading.
    public static func plainHeading(_ s: String) -> String {
        s.replacingOccurrences(of: "[*_`~]", with: "", options: .regularExpression)
            .replacingOccurrences(of: "\\[([^\\]]*)\\]\\([^)]*\\)", with: "$1", options: .regularExpression)
    }

    /// Adds a footnote reference at the caret and a definition at the end of the text.
    public static func footnote(_ text: NSString, selection sel: NSRange) -> (reference: TextEdit, definitionAppend: String, label: String) {
        let existing = (try? NSRegularExpression(pattern: "\\[\\^(\\d+)\\]"))?.matches(in: text as String, range: NSRange(location: 0, length: text.length))
            .compactMap { Int(text.substring(with: $0.range(at: 1))) } ?? []
        let n = (existing.max() ?? 0) + 1
        let ref = "[^\(n)]"
        let def = (text.hasSuffix("\n") ? "" : "\n") + "\n[^\(n)]: "
        return (TextEdit(range: NSRange(location: NSMaxRange(sel), length: 0), replacement: ref, selection: NSRange(location: NSMaxRange(sel) + (ref as NSString).length, length: 0)), def, "\(n)")
    }
}

/// Scans code fences, front matter and line starts. Fast enough to run on every edit of a 1 MB file.
public struct BlockScanner: Sendable {
    public var lineStarts: [Int] = []
    /// Line index ranges (inclusive) for fenced code blocks, including fence lines.
    public var codeBlocks: [ClosedRange<Int>] = []
    /// Language for each code block (parallel to `codeBlocks`).
    public var codeLanguages: [String] = []
    public var frontMatter: ClosedRange<Int>?

    public init() {}

    public func isInCodeOrFrontMatter(line: Int) -> Bool {
        if let fm = frontMatter, fm.contains(line) { return true }
        return codeBlockIndex(containing: line) != nil
    }

    public func codeBlockIndex(containing line: Int) -> Int? {
        var lo = 0, hi = codeBlocks.count - 1
        while lo <= hi {
            let mid = (lo + hi) / 2
            let r = codeBlocks[mid]
            if line < r.lowerBound { hi = mid - 1 } else if line > r.upperBound { lo = mid + 1 } else { return mid }
        }
        return nil
    }

    public func line(at location: Int) -> Int {
        var lo = 0, hi = lineStarts.count - 1
        while lo < hi {
            let mid = (lo + hi + 1) / 2
            if lineStarts[mid] <= location { lo = mid } else { hi = mid - 1 }
        }
        return max(lo, 0)
    }

    public static func scan(_ text: String) -> BlockScanner {
        scan(units: Array(text.utf16))
    }

    /// Fast path for NSString-backed text (copies UTF-16 units in one call).
    public static func scan(nsString ns: NSString) -> BlockScanner {
        var buf = [unichar](repeating: 0, count: ns.length)
        if ns.length > 0 { ns.getCharacters(&buf, range: NSRange(location: 0, length: ns.length)) }
        return scan(units: buf)
    }

    /// Shifts line starts after an edit that didn't add or remove line breaks or touch block markers.
    public mutating func applyInLineEdit(at location: Int, delta: Int) {
        guard delta != 0, !lineStarts.isEmpty else { return }
        let line = self.line(at: location)
        var i = line + 1
        while i < lineStarts.count { lineStarts[i] += delta; i += 1 }
    }

    static func scan(units utf16: [unichar]) -> BlockScanner {
        var s = BlockScanner()
        let n = utf16.count
        s.lineStarts.reserveCapacity(n / 40 + 1)
        var lineStart = 0
        var lineIndex = 0
        var fenceStartLine = -1
        var fenceChar: UInt16 = 0
        var fenceLen = 0
        var fenceLang = ""
        var inFrontMatter = false
        var i = 0
        func processLine(_ start: Int, _ end: Int) {
            // Skip up to 3 spaces of indentation.
            var p = start
            var spaces = 0
            while p < end && utf16[p] == 32 && spaces < 4 { p += 1; spaces += 1 }
            if lineIndex == 0 && end - start >= 3 && utf16[start] == 45 && utf16[start + 1] == 45 && utf16[start + 2] == 45 && (end - start == 3 || (start + 3..<end).allSatisfy({ utf16[$0] == 32 })) {
                inFrontMatter = true
                return
            }
            if inFrontMatter {
                let len = end - start
                if len >= 3 && ((utf16[start] == 45 && utf16[start + 1] == 45 && utf16[start + 2] == 45) || (utf16[start] == 46 && utf16[start + 1] == 46 && utf16[start + 2] == 46)) {
                    s.frontMatter = 0...lineIndex
                    inFrontMatter = false
                }
                return
            }
            guard spaces < 4, p < end else { return }
            let c = utf16[p]
            guard c == 96 || c == 126 else { return }
            var q = p
            while q < end && utf16[q] == c { q += 1 }
            let run = q - p
            guard run >= 3 else { return }
            if fenceStartLine < 0 {
                // Backtick fences cannot have backticks in the info string.
                if c == 96 && (q..<end).contains(where: { utf16[$0] == 96 }) { return }
                fenceStartLine = lineIndex; fenceChar = c; fenceLen = run
                let info = String(utf16CodeUnits: Array(utf16[q..<end]), count: end - q).trimmingCharacters(in: .whitespaces)
                fenceLang = String(info.split(separator: " ").first ?? "")
            } else if c == fenceChar && run >= fenceLen && (q..<end).allSatisfy({ utf16[$0] == 32 || utf16[$0] == 9 }) {
                s.codeBlocks.append(fenceStartLine...lineIndex)
                s.codeLanguages.append(fenceLang)
                fenceStartLine = -1
            }
        }
        while i <= n {
            if i == n || utf16[i] == 10 {
                s.lineStarts.append(lineStart)
                var end = i
                if end > lineStart && utf16[end - 1] == 13 { end -= 1 }
                processLine(lineStart, end)
                lineIndex += 1
                lineStart = i + 1
            }
            i += 1
        }
        if fenceStartLine >= 0 {
            s.codeBlocks.append(fenceStartLine...max(lineIndex - 1, fenceStartLine))
            s.codeLanguages.append(fenceLang)
        }
        return s
    }
}

public struct TextStats: Equatable, Sendable {
    public var words: Int
    public var characters: Int
    public var readingMinutes: Int

    public static func compute(_ text: String) -> TextStats {
        var words = 0
        var inWord = false
        var chars = 0
        for scalar in text.unicodeScalars {
            if scalar.properties.isWhitespace {
                inWord = false
            } else {
                chars += 1
                if DisplayWidth.isWide(scalar.value) { words += 1; inWord = false; continue }
                if !inWord { words += 1; inWord = true }
            }
        }
        // Count characters as user-perceived characters (excluding newlines).
        let characters = text.count - text.filter { $0 == "\n" || $0 == "\r\n" }.count
        _ = chars
        return TextStats(words: words, characters: characters, readingMinutes: max(words > 0 ? 1 : 0, Int((Double(words) / 200.0).rounded(.up))))
    }
}

/// Converts an HTML table (e.g. copied from a browser) into rows of cell text.
public enum HTMLTableParser {
    public static func rows(from html: String) -> [[String]]? {
        guard let tableRange = html.range(of: "<table", options: .caseInsensitive) else { return nil }
        let body = String(html[tableRange.lowerBound...])
        let rowRegex = try! NSRegularExpression(pattern: "<tr[^>]*>(.*?)</tr>", options: [.caseInsensitive, .dotMatchesLineSeparators])
        let cellRegex = try! NSRegularExpression(pattern: "<t[hd][^>]*>(.*?)</t[hd]>", options: [.caseInsensitive, .dotMatchesLineSeparators])
        let ns = body as NSString
        let rows: [[String]] = rowRegex.matches(in: body, range: NSRange(location: 0, length: ns.length)).map { m in
            let rowHTML = ns.substring(with: m.range(at: 1))
            let rns = rowHTML as NSString
            return cellRegex.matches(in: rowHTML, range: NSRange(location: 0, length: rns.length)).map { cm in
                decode(rns.substring(with: cm.range(at: 1)))
            }
        }.filter { !$0.isEmpty }
        return rows.isEmpty ? nil : rows
    }

    /// Cell text: paragraphs, list items and line breaks inside a cell are kept apart with `<br>`
    /// (GFM tables allow it), other tags are dropped, entities decoded and `|` escaped.
    static func decode(_ s: String) -> String {
        let breakMark = "\u{1}"
        var t = s.replacingOccurrences(of: "<br\\s*/?>|</(p|div|li|h[1-6]|tr|blockquote|pre)\\s*>", with: breakMark, options: [.regularExpression, .caseInsensitive])
        t = t.replacingOccurrences(of: "<[^>]+>", with: "", options: .regularExpression)
        t = HTMLMarkdown.decodeEntities(t).replacingOccurrences(of: "\u{A0}", with: " ")
        let parts = t.components(separatedBy: breakMark)
            .map { $0.replacingOccurrences(of: "\\s+", with: " ", options: .regularExpression).trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        return parts.joined(separator: "<br>").replacingOccurrences(of: "|", with: "\\|")
    }
}
