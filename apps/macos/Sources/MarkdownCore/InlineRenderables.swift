import Foundation

/// An item the editor can draw under its Markdown source when inline display is on:
/// an image, a Mermaid diagram or a block of math.
public struct InlineRenderable: Hashable, Sendable {
    public enum Kind: Hashable, Sendable {
        /// `path` is the decoded link target (`assets/…`, a web URL, …).
        case image(path: String, alt: String)
        case mermaid(source: String)
        case math(source: String)
    }

    public var kind: Kind
    /// The source lines (0-based, inclusive). The rendering goes under the last one.
    public var lines: ClosedRange<Int>
    public var anchorLine: Int { lines.upperBound }

    public init(kind: Kind, lines: ClosedRange<Int>) { self.kind = kind; self.lines = lines }
}

public enum InlineRenderables {
    private static let mdImageRx = try! NSRegularExpression(pattern: "!\\[([^\\]\\n]*)\\]\\(([^)\\n]+)\\)")
    private static let tagImageRx = try! NSRegularExpression(pattern: "<img\\s[^>]*?src\\s*=\\s*[\"']([^\"']+)[\"'][^>]*>", options: .caseInsensitive)
    private static let altRx = try! NSRegularExpression(pattern: "\\balt\\s*=\\s*[\"']([^\"']*)[\"']", options: .caseInsensitive)

    /// Finds images (outside code), ```` ```mermaid ```` blocks and `$$ … $$` math blocks, in document order.
    public static func find(in text: String, scan: BlockScanner) -> [InlineRenderable] {
        let ns = text as NSString
        let count = scan.lineStarts.count
        guard count > 0 else { return [] }
        func line(_ i: Int) -> String {
            let s = scan.lineStarts[i]
            let e = i + 1 < count ? scan.lineStarts[i + 1] : ns.length
            return ns.substring(with: NSRange(location: s, length: e - s)).trimmingCharacters(in: .newlines)
        }
        var out: [InlineRenderable] = []
        var i = 0
        while i < count {
            // Mermaid: a closed fenced block whose language is mermaid.
            if let bi = scan.codeBlockIndex(containing: i) {
                let block = scan.codeBlocks[bi]
                let closing = line(block.upperBound).trimmingCharacters(in: .whitespaces)
                if i == block.lowerBound, scan.codeLanguages[bi].lowercased() == "mermaid", block.upperBound > block.lowerBound,
                   closing.hasPrefix("```") || closing.hasPrefix("~~~") {
                    let body = block.lowerBound + 1 < block.upperBound ? ((block.lowerBound + 1)...(block.upperBound - 1)).map(line).joined(separator: "\n") : ""
                    if !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        out.append(InlineRenderable(kind: .mermaid(source: body), lines: block))
                    }
                }
                i = block.upperBound + 1
                continue
            }
            if let fm = scan.frontMatter, fm.contains(i) { i = fm.upperBound + 1; continue }
            let text = line(i)
            let trimmed = text.trimmingCharacters(in: .whitespaces)
            // Math: $$ on its own line(s), the same block rule as the preview.
            if trimmed.hasPrefix("$$") {
                let first = String(trimmed.dropFirst(2))
                if first.trimmingCharacters(in: .whitespaces).hasSuffix("$$"), first.trimmingCharacters(in: .whitespaces).count >= 2 {
                    let body = String(first.trimmingCharacters(in: .whitespaces).dropLast(2))
                    if !body.trimmingCharacters(in: .whitespaces).isEmpty {
                        out.append(InlineRenderable(kind: .math(source: body), lines: i...i))
                    }
                    i += 1
                    continue
                }
                var body = first.trimmingCharacters(in: .whitespaces).isEmpty ? [String]() : [first]
                var j = i + 1
                var closed = false
                while j < count, !scan.isInCodeOrFrontMatter(line: j) {
                    let t = line(j).trimmingCharacters(in: .whitespaces)
                    if t.hasSuffix("$$") { body.append(String(t.dropLast(2))); closed = true; break }
                    body.append(line(j))
                    j += 1
                }
                if closed {
                    let src = body.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
                    if !src.isEmpty { out.append(InlineRenderable(kind: .math(source: src), lines: i...j)) }
                    i = j + 1
                    continue
                }
            }
            // Images anywhere on the line, outside inline code.
            if text.contains("![") || text.range(of: "<img", options: .caseInsensitive) != nil {
                let plain = NoteText.removingCodeSpans(text)
                let pns = plain as NSString
                var found: [(Int, InlineRenderable)] = []
                for m in mdImageRx.matches(in: plain, range: NSRange(location: 0, length: pns.length)) {
                    let path = ImageNaming.decodeMarkdownPath(pns.substring(with: m.range(at: 2)))
                    if !path.isEmpty { found.append((m.range.location, InlineRenderable(kind: .image(path: path, alt: pns.substring(with: m.range(at: 1))), lines: i...i))) }
                }
                for m in tagImageRx.matches(in: plain, range: NSRange(location: 0, length: pns.length)) {
                    let tag = pns.substring(with: m.range) as NSString
                    let alt = altRx.firstMatch(in: tag as String, range: NSRange(location: 0, length: tag.length)).map { tag.substring(with: $0.range(at: 1)) } ?? ""
                    let path = pns.substring(with: m.range(at: 1))
                    found.append((m.range.location, InlineRenderable(kind: .image(path: path.removingPercentEncoding ?? path, alt: alt), lines: i...i)))
                }
                out += found.sorted { $0.0 < $1.0 }.map(\.1)
            }
            i += 1
        }
        return out
    }
}
