import Foundation

/// Text helpers shared by titles, tags and search: code detection, prose extraction and word normalization.
public enum NoteText {
    /// Folds case and diacritics the way the search index does ("Ré" → "re").
    public static func fold(_ s: String) -> String {
        s.folding(options: [.caseInsensitive, .diacriticInsensitive, .widthInsensitive], locale: nil).lowercased()
    }

    /// Splits text into folded words (runs of letters and digits), as the unicode61 tokenizer does.
    public static func words(_ s: String) -> [String] {
        var out: [String] = []
        var cur = ""
        for scalar in fold(s).unicodeScalars {
            if CharacterSet.alphanumerics.contains(scalar) || scalar.properties.generalCategory == .nonspacingMark {
                cur.unicodeScalars.append(scalar)
            } else if !cur.isEmpty {
                out.append(cur); cur = ""
            }
        }
        if !cur.isEmpty { out.append(cur) }
        return out
    }

    /// True when the string contains Chinese, Japanese or Korean characters, which aren't split into words.
    public static func containsCJK(_ s: String) -> Bool {
        s.unicodeScalars.contains { isCJK($0) }
    }

    static func isCJK(_ u: Unicode.Scalar) -> Bool {
        switch u.value {
        case 0x3040...0x30FF, 0x3400...0x4DBF, 0x4E00...0x9FFF, 0xF900...0xFAFF, 0xAC00...0xD7AF, 0x1100...0x11FF, 0x20000...0x2FA1F: return true
        default: return false
        }
    }

    /// Line ranges and flags for a note body.
    public struct Lines {
        public var lines: [String]
        public var inCode: [Bool]
        public var inFrontMatter: [Bool]
    }

    public static func lines(_ body: String) -> Lines {
        let scan = BlockScanner.scan(body)
        let ns = body as NSString
        var lines: [String] = []
        var code: [Bool] = []
        var fm: [Bool] = []
        for (i, start) in scan.lineStarts.enumerated() {
            let end = i + 1 < scan.lineStarts.count ? scan.lineStarts[i + 1] : ns.length
            var line = ns.substring(with: NSRange(location: start, length: end - start))
            while line.hasSuffix("\n") || line.hasSuffix("\r") { line.removeLast() }
            lines.append(line)
            code.append(scan.codeBlockIndex(containing: i) != nil)
            fm.append(scan.frontMatter?.contains(i) ?? false)
        }
        return Lines(lines: lines, inCode: code, inFrontMatter: fm)
    }

    /// Removes inline code spans (`` `x` ``, ``` ``x`` ```) from one line, replacing them with a space.
    public static func removingCodeSpans(_ line: String) -> String {
        guard line.contains("`") else { return line }
        let chars = Array(line)
        var out = ""
        var i = 0
        while i < chars.count {
            if chars[i] == "`" {
                var run = 0
                while i + run < chars.count && chars[i + run] == "`" { run += 1 }
                // Find a closing run of the same length.
                var j = i + run
                var found = -1
                while j < chars.count {
                    if chars[j] == "`" {
                        var r = 0
                        while j + r < chars.count && chars[j + r] == "`" { r += 1 }
                        if r == run { found = j; break }
                        j += r
                    } else { j += 1 }
                }
                if found >= 0 {
                    out.append(" ")
                    i = found + run
                    continue
                }
                out.append(contentsOf: String(repeating: "`", count: run))
                i += run
                continue
            }
            out.append(chars[i])
            i += 1
        }
        return out
    }

    private static let commentRegex = try! NSRegularExpression(pattern: "<!--[\\s\\S]*?-->")
    private static let urlRegex = try! NSRegularExpression(pattern: "(?:[a-zA-Z][a-zA-Z0-9+.-]*://|www\\.|mailto:)[^\\s<>()]*")

    /// The note's text without front matter, fenced code, code spans, HTML comments and URLs.
    /// Implied tags match against this, so words in code never count.
    public static func prose(_ body: String) -> String {
        let l = lines(body)
        var kept: [String] = []
        for (i, line) in l.lines.enumerated() where !l.inCode[i] && !l.inFrontMatter[i] {
            kept.append(removingCodeSpans(line))
        }
        var s = kept.joined(separator: "\n")
        s = commentRegex.stringByReplacingMatches(in: s, range: NSRange(location: 0, length: (s as NSString).length), withTemplate: " ")
        s = urlRegex.stringByReplacingMatches(in: s, range: NSRange(location: 0, length: (s as NSString).length), withTemplate: " ")
        return s
    }
}
