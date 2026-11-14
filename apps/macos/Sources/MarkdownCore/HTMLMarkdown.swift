import Foundation

/// Converts clipboard HTML (Teams, browsers, Outlook) to Markdown for rich paste. Images are left as
/// placeholders so the app can store each one in Assets and put its link in their place.
public enum HTMLMarkdown {
    public struct Image: Equatable, Sendable {
        public var src: String
        public var alt: String
    }

    public struct Result: Equatable, Sendable {
        /// Markdown with `placeholder(i)` where image `i` goes.
        public var markdown: String
        public var images: [Image]
    }

    /// The marker for image `i` in `Result.markdown` (private-use characters never typed by people).
    public static func placeholder(_ i: Int) -> String { "\u{E000}\(i)\u{E001}" }

    /// Replaces the image placeholders with the given Markdown (an image link, or "" to drop the image).
    public static func fill(_ markdown: String, with replacements: [String]) -> String {
        var out = markdown
        for (i, r) in replacements.enumerated() { out = out.replacingOccurrences(of: placeholder(i), with: r) }
        return out
    }

    /// True when the HTML has at least one `<img>` with a source (the case plain-text paste would lose).
    public static func hasImages(_ html: String) -> Bool {
        html.range(of: "<img\\b[^>]*\\bsrc\\s*=", options: [.regularExpression, .caseInsensitive]) != nil
    }

    /// True when the HTML has list items, whose markers and nesting plain text loses.
    public static func hasLists(_ html: String) -> Bool {
        html.range(of: "<li\\b", options: [.regularExpression, .caseInsensitive]) != nil
    }

    /// `tableWidth`: the editor's width in columns; wider tables are written compactly (see `MarkdownTable.layoutWidth`).
    public static func convert(_ html: String, tableWidth: Int? = nil) -> Result {
        var r = Renderer()
        r.tableWidth = tableWidth
        let root = Parser.parse(fragment(html))
        let blocks = r.blocks(root.children)
        var md = blocks.joined(separator: "\n\n")
        md = md.replacingOccurrences(of: "\n{3,}", with: "\n\n", options: .regularExpression)
        return Result(markdown: md.trimmingCharacters(in: .whitespacesAndNewlines), images: r.images)
    }

    /// The copied part of a full clipboard document (between StartFragment and EndFragment, else the body).
    static func fragment(_ html: String) -> String {
        if let s = html.range(of: "<!--StartFragment-->"), let e = html.range(of: "<!--EndFragment-->", range: s.upperBound..<html.endIndex) {
            return String(html[s.upperBound..<e.lowerBound])
        }
        if let b = html.range(of: "<body[^>]*>", options: [.regularExpression, .caseInsensitive]) {
            let end = html.range(of: "</body>", options: .caseInsensitive, range: b.upperBound..<html.endIndex)?.lowerBound ?? html.endIndex
            return String(html[b.upperBound..<end])
        }
        return html
    }

    // MARK: Tree

    final class Node {
        let name: String          // "" for text
        var attrs: [String: String]
        var text: String
        var children: [Node] = []
        init(name: String, attrs: [String: String] = [:], text: String = "") { self.name = name; self.attrs = attrs; self.text = text }
    }

    enum Parser {
        static let void: Set<String> = ["img", "br", "hr", "meta", "link", "input", "col", "wbr", "source", "area", "base"]
        static let skipped: Set<String> = ["head", "style", "script", "title", "template", "noscript", "svg", "button", "select"]
        private static let tokenRx = try! NSRegularExpression(pattern: "<!--[\\s\\S]*?-->|<(/?)([a-zA-Z][a-zA-Z0-9:-]*)((?:[^>\"']|\"[^\"]*\"|'[^']*')*)>|<![^>]*>")
        private static let attrRx = try! NSRegularExpression(pattern: "([a-zA-Z_:][-a-zA-Z0-9_:.]*)(?:\\s*=\\s*(?:\"([^\"]*)\"|'([^']*)'|([^\\s\"'>]+)))?")

        static func parse(_ html: String) -> Node {
            let root = Node(name: "#root")
            var stack = [root]
            let ns = html as NSString
            var pos = 0
            var skipping: (name: String, depth: Int)?
            func addText(_ s: String) {
                guard skipping == nil, !s.isEmpty else { return }
                stack.last!.children.append(Node(name: "", text: decodeEntities(s)))
            }
            for m in tokenRx.matches(in: html, range: NSRange(location: 0, length: ns.length)) {
                if m.range.location > pos { addText(ns.substring(with: NSRange(location: pos, length: m.range.location - pos))) }
                pos = NSMaxRange(m.range)
                guard m.range(at: 2).location != NSNotFound else { continue }   // comment or doctype
                let closing = m.range(at: 1).length > 0
                let name = ns.substring(with: m.range(at: 2)).lowercased()
                let rest = m.range(at: 3).location != NSNotFound ? ns.substring(with: m.range(at: 3)) : ""
                if let s = skipping {
                    if name == s.name { skipping = closing ? (s.depth <= 1 ? nil : (s.name, s.depth - 1)) : (s.name, s.depth + 1) }
                    continue
                }
                if !closing && skipped.contains(name) {
                    if !rest.hasSuffix("/") { skipping = (name, 1) }
                    continue
                }
                if closing {
                    // Close the nearest open element with this name; ignore stray closers.
                    if let i = stack.lastIndex(where: { $0.name == name }), i > 0 { stack.removeSubrange(i...) }
                    continue
                }
                // An <li> or <p> closes an unclosed sibling of the same kind.
                if name == "li", let i = stack.lastIndex(where: { $0.name == "li" }),
                   !stack[(i + 1)...].contains(where: { $0.name == "ul" || $0.name == "ol" }) { stack.removeSubrange(i...) }
                if name == "p", stack.last?.name == "p" { stack.removeLast() }
                let node = Node(name: name, attrs: attributes(rest))
                stack.last!.children.append(node)
                if !void.contains(name) && !rest.hasSuffix("/") { stack.append(node) }
            }
            if pos < ns.length { addText(ns.substring(from: pos)) }
            return root
        }

        static func attributes(_ s: String) -> [String: String] {
            var out: [String: String] = [:]
            let ns = s as NSString
            for m in attrRx.matches(in: s, range: NSRange(location: 0, length: ns.length)) {
                let key = ns.substring(with: m.range(at: 1)).lowercased()
                var value = ""
                for g in 2...4 where m.range(at: g).location != NSNotFound { value = ns.substring(with: m.range(at: g)) }
                if out[key] == nil { out[key] = decodeEntities(value) }
            }
            return out
        }
    }

    // MARK: Entities

    static let namedEntities: [String: String] = [
        "amp": "&", "lt": "<", "gt": ">", "quot": "\"", "apos": "'", "nbsp": "\u{A0}", "ndash": "–", "mdash": "—",
        "hellip": "…", "lsquo": "‘", "rsquo": "’", "ldquo": "“", "rdquo": "”", "bull": "•", "middot": "·",
        "copy": "©", "reg": "®", "trade": "™", "deg": "°", "times": "×", "divide": "÷", "euro": "€", "pound": "£",
        "laquo": "«", "raquo": "»", "rarr": "→", "larr": "←", "zwj": "\u{200D}", "zwnj": "\u{200C}", "shy": "",
    ]
    private static let entityRx = try! NSRegularExpression(pattern: "&(#[0-9]+|#[xX][0-9a-fA-F]+|[a-zA-Z][a-zA-Z0-9]*);")

    static func decodeEntities(_ s: String) -> String {
        guard s.contains("&") else { return s }
        let ns = s as NSString
        var out = ""
        var pos = 0
        for m in entityRx.matches(in: s, range: NSRange(location: 0, length: ns.length)) {
            out += ns.substring(with: NSRange(location: pos, length: m.range.location - pos))
            let body = ns.substring(with: m.range(at: 1))
            var rep: String?
            if body.hasPrefix("#x") || body.hasPrefix("#X") {
                rep = UInt32(body.dropFirst(2), radix: 16).flatMap(Unicode.Scalar.init).map { String(Character($0)) }
            } else if body.hasPrefix("#") {
                rep = UInt32(body.dropFirst()).flatMap(Unicode.Scalar.init).map { String(Character($0)) }
            } else {
                rep = namedEntities[body.lowercased()]
            }
            out += rep ?? ns.substring(with: m.range)
            pos = NSMaxRange(m.range)
        }
        out += ns.substring(from: pos)
        return out
    }

    // MARK: Rendering

    struct Renderer {
        var images: [Image] = []
        var tableWidth: Int?

        static let blockNames: Set<String> = ["p", "div", "section", "article", "header", "footer", "main", "aside", "nav",
                                               "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "blockquote", "pre",
                                               "table", "hr", "figure", "figcaption", "dl", "dt", "dd", "address", "details", "summary"]

        static func isBlock(_ n: Node) -> Bool { blockNames.contains(n.name) }

        /// Renders a list of nodes as Markdown blocks. Runs of inline content become paragraphs.
        mutating func blocks(_ nodes: [Node]) -> [String] {
            var out: [String] = []
            var inline: [Node] = []
            func flush(_ r: inout Renderer) {
                let text = r.inlineText(inline)
                if !text.isEmpty { out.append(text) }
                inline = []
            }
            for n in nodes {
                if Self.isBlock(n) || containsBlock(n) {
                    flush(&self)
                    out += block(n)
                } else {
                    inline.append(n)
                }
            }
            flush(&self)
            return out
        }

        /// An inline wrapper (`<span>`, `<b>`) that holds blocks is treated as a transparent container.
        func containsBlock(_ n: Node) -> Bool {
            guard !n.name.isEmpty, !Self.isBlock(n) else { return false }
            return n.children.contains { Self.isBlock($0) || containsBlock($0) }
        }

        mutating func block(_ n: Node) -> [String] {
            switch n.name {
            case "h1", "h2", "h3", "h4", "h5", "h6":
                let level = Int(String(n.name.last!))!
                let t = inlineText(n.children).replacingOccurrences(of: "\n", with: " ")
                return t.isEmpty ? [] : [String(repeating: "#", count: level) + " " + t]
            case "ul", "ol":
                let l = list(n, ordered: n.name == "ol")
                return l.isEmpty ? [] : [l]
            case "li":
                let l = listItem(n, marker: "- ")
                return l.isEmpty ? [] : [l]
            case "blockquote":
                let inner = blocks(n.children).joined(separator: "\n\n")
                guard !inner.isEmpty else { return [] }
                return [inner.components(separatedBy: "\n").map { $0.isEmpty ? ">" : "> " + $0 }.joined(separator: "\n")]
            case "pre":
                let code = rawText(n).trimmingCharacters(in: .newlines)
                guard !code.isEmpty else { return [] }
                let fence = code.contains("```") ? "~~~" : "```"
                return [fence + "\n" + code + "\n" + fence]
            case "hr":
                return ["---"]
            case "table":
                return table(n).map { [$0] } ?? []
            default:
                return blocks(n.children)
            }
        }

        mutating func list(_ n: Node, ordered: Bool) -> String {
            var lines: [String] = []
            var number = Int(n.attrs["start"] ?? "") ?? 1
            for c in n.children {
                if c.name == "li" {
                    let item = listItem(c, marker: ordered ? "\(number). " : "- ")
                    if !item.isEmpty { lines.append(item); number += 1 }
                } else if c.name == "ul" || c.name == "ol" {
                    // A list directly inside a list (some editors do this): indent it under the previous item.
                    let nested = list(c, ordered: c.name == "ol")
                    if !nested.isEmpty { lines.append(indent(nested)) }
                } else if c.name.isEmpty, c.text.allSatisfy(\.isWhitespace) {
                    continue
                } else {
                    let t = inlineText([c])
                    if !t.isEmpty { lines.append("- " + t) }
                }
            }
            return lines.joined(separator: "\n")
        }

        mutating func listItem(_ n: Node, marker: String) -> String {
            var parts = blocks(n.children)
            guard !parts.isEmpty else { return "" }
            let first = parts.removeFirst()
            var out = marker + first.replacingOccurrences(of: "\n", with: "\n" + String(repeating: " ", count: 4))
            for p in parts {
                let isList = p.hasPrefix("- ") || p.range(of: "^[0-9]+\\. ", options: .regularExpression) != nil
                out += (isList ? "\n" : "\n\n") + indent(p)
            }
            return out
        }

        func indent(_ s: String) -> String {
            s.components(separatedBy: "\n").map { $0.isEmpty ? $0 : "    " + $0 }.joined(separator: "\n")
        }

        mutating func table(_ n: Node) -> String? {
            var rows: [[String]] = []
            func collect(_ node: Node, _ r: inout Renderer) {
                for c in node.children {
                    if c.name == "tr" {
                        let cells = c.children.filter { $0.name == "td" || $0.name == "th" }
                            .map { cell in
                                // Paragraphs and line breaks in a cell stay apart with <br>, which GFM tables allow.
                                r.blocks(cell.children).map { $0.replacingOccurrences(of: "\n", with: "<br>") }
                                    .joined(separator: "<br>").replacingOccurrences(of: "|", with: "\\|")
                            }
                        if !cells.isEmpty { rows.append(cells) }
                    } else if !c.name.isEmpty {
                        collect(c, &r)
                    }
                }
            }
            collect(n, &self)
            guard let header = rows.first else { return nil }
            let width = rows.map(\.count).max() ?? header.count
            let pad = { (r: [String]) in r + Array(repeating: "", count: width - r.count) }
            var t = MarkdownTable(header: pad(header), rows: rows.dropFirst().map(pad))
            t.layoutWidth = tableWidth
            return t.formatted()
        }

        /// Text of a node with whitespace kept (for `<pre>`).
        func rawText(_ n: Node) -> String {
            if n.name.isEmpty { return n.text }
            if n.name == "br" { return "\n" }
            return n.children.map(rawText).joined()
        }

        /// Inline Markdown for a run of nodes, with whitespace collapsed and `<br>` as a line break.
        mutating func inlineText(_ nodes: [Node]) -> String {
            var s = nodes.map { inline($0) }.joined()
            s = s.replacingOccurrences(of: "\u{A0}", with: " ")
            s = s.replacingOccurrences(of: "[ \\t\\r\\n]+", with: " ", options: .regularExpression)
            s = s.replacingOccurrences(of: " ?\u{2028} ?", with: "\n", options: .regularExpression)
            return s.trimmingCharacters(in: CharacterSet(charactersIn: " \t"))
                .trimmingCharacters(in: .newlines)
        }

        mutating func inline(_ n: Node) -> String {
            if n.name.isEmpty { return n.text }
            switch n.name {
            case "br": return "\u{2028}"
            case "img":
                guard let src = n.attrs["src"], !src.isEmpty else { return "" }
                images.append(Image(src: src, alt: n.attrs["alt"] ?? ""))
                return HTMLMarkdown.placeholder(images.count - 1)
            case "strong", "b": return wrap(n, "**")
            case "em", "i", "cite", "dfn": return wrap(n, "*")
            case "s", "del", "strike": return wrap(n, "~~")
            case "code", "kbd", "samp", "tt":
                let t = n.children.map(rawText).joined().replacingOccurrences(of: "\n", with: " ")
                guard !t.isEmpty else { return "" }
                return t.contains("`") ? "`` " + t + " ``" : "`" + t + "`"
            case "a":
                let text = n.children.map { inline($0) }.joined()
                let label = text.trimmingCharacters(in: .whitespacesAndNewlines)
                guard let href = n.attrs["href"], !href.isEmpty, !href.hasPrefix("javascript:"), !href.hasPrefix("#") else { return text }
                let target = href.contains(" ") || href.contains(")") ? "<" + href + ">" : href
                if label.isEmpty { return "" }
                if label == href { return href }
                return "[" + label + "](" + target + ")"
            default:
                return n.children.map { inline($0) }.joined()
            }
        }

        /// Wraps inline content in a delimiter, keeping surrounding spaces outside it (`** x **` doesn't render).
        mutating func wrap(_ n: Node, _ d: String) -> String {
            let inner = n.children.map { inline($0) }.joined()
            let trimmed = inner.trimmingCharacters(in: .whitespaces)
            guard !trimmed.isEmpty, !trimmed.allSatisfy({ $0 == "\u{A0}" }) else { return inner }
            let lead = inner.prefix { $0 == " " || $0 == "\u{A0}" }, trail = String(inner.reversed().prefix { $0 == " " || $0 == "\u{A0}" })
            return lead + d + trimmed + d + trail
        }
    }

    // MARK: Data URLs

    /// The bytes and file extension of a `data:image/...` URL, or nil.
    public static func decodeDataURL(_ src: String) -> (data: Data, ext: String)? {
        guard src.lowercased().hasPrefix("data:"), let comma = src.firstIndex(of: ",") else { return nil }
        let meta = src[src.index(src.startIndex, offsetBy: 5)..<comma].lowercased()
        let payload = String(src[src.index(after: comma)...])
        let mime = meta.split(separator: ";").first.map(String.init) ?? ""
        guard mime.hasPrefix("image/") else { return nil }
        let data: Data?
        if meta.contains(";base64") {
            data = Data(base64Encoded: payload.filter { !$0.isWhitespace }, options: .ignoreUnknownCharacters)
        } else {
            data = payload.removingPercentEncoding.map { Data($0.utf8) }
        }
        guard let data, !data.isEmpty else { return nil }
        var ext = String(mime.dropFirst("image/".count))
        if ext == "jpeg" { ext = "jpg" } else if ext == "svg+xml" { ext = "svg" }
        return (data, ext)
    }
}
