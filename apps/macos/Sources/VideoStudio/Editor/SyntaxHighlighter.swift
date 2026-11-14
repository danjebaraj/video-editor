import AppKit
import MarkdownCore

/// Markdown syntax colouring applied to a text storage, one line range at a time.
/// Block context (code fences, front matter) comes from `BlockScanner`.
@MainActor
final class SyntaxHighlighter {
    var baseFont: NSFont = AppSettings.shared.editorFont
    var paragraphStyle = NSParagraphStyle.default
    /// Returns true if the image at a raw Markdown path exists (for broken-path underlines).
    var imageExists: ((String) -> Bool)?

    private var boldFont: NSFont = .boldSystemFont(ofSize: 14)
    /// For a todo's date part.
    private var smallFont: NSFont = .systemFont(ofSize: 12)
    private var italicFont: NSFont = .systemFont(ofSize: 14)
    private var boldItalicFont: NSFont = .boldSystemFont(ofSize: 14)
    private var codeFont: NSFont = .monospacedSystemFont(ofSize: 14, weight: .regular)

    static let markerColor = NSColor.tertiaryLabelColor
    static let headingColor = NSColor(name: nil) { $0.bestMatch(from: [.darkAqua]) == .darkAqua ? NSColor(srgbRed: 0.55, green: 0.75, blue: 1.0, alpha: 1) : NSColor(srgbRed: 0.05, green: 0.30, blue: 0.70, alpha: 1) }
    static let codeColor = NSColor(name: nil) { $0.bestMatch(from: [.darkAqua]) == .darkAqua ? NSColor(srgbRed: 1.0, green: 0.55, blue: 0.55, alpha: 1) : NSColor(srgbRed: 0.75, green: 0.10, blue: 0.25, alpha: 1) }
    static let codeBackground = NSColor(name: nil) { $0.bestMatch(from: [.darkAqua]) == .darkAqua ? NSColor(white: 1, alpha: 0.06) : NSColor(white: 0, alpha: 0.045) }
    static let listColor = NSColor.systemOrange
    static let quoteColor = NSColor.secondaryLabelColor
    static let linkColor = NSColor.linkColor
    static let tagBackground = NSColor.controlAccentColor.withAlphaComponent(0.13)
    static let autoTagBackground = NSColor.systemIndigo.withAlphaComponent(0.14)
    /// A folded-away line: no height to speak of.
    static let hiddenFont = NSFont.systemFont(ofSize: 0.01)
    static let hiddenParagraph: NSParagraphStyle = {
        let p = NSMutableParagraphStyle()
        p.minimumLineHeight = 0.01; p.maximumLineHeight = 0.01; p.paragraphSpacing = 0; p.paragraphSpacingBefore = 0
        return p
    }()
    /// Todo categories look like tags, in a different tint.
    static let categoryBackground = NSColor.systemPurple.withAlphaComponent(0.14)
    static let categoryColor = NSColor(name: nil) { $0.bestMatch(from: [.darkAqua]) == .darkAqua ? NSColor(srgbRed: 0.85, green: 0.7, blue: 1.0, alpha: 1) : NSColor(srgbRed: 0.45, green: 0.2, blue: 0.65, alpha: 1) }
    static let urlColor = NSColor.secondaryLabelColor
    static let frontMatterColor = NSColor.systemPurple
    static let htmlColor = NSColor.systemTeal
    static let mathColor = NSColor.systemGreen
    /// Code fence content colours, chosen per language so different fences look different.
    static let fencePalette: [NSColor] = [.systemTeal, .systemIndigo, .systemBrown, .systemGreen, .systemPink, .systemCyan, .systemMint]

    func updateFonts() {
        let size = baseFont.pointSize
        let fm = NSFontManager.shared
        boldFont = fm.convert(baseFont, toHaveTrait: .boldFontMask)
        smallFont = NSFont(descriptor: baseFont.fontDescriptor, size: max(9, baseFont.pointSize - 2)) ?? baseFont
        italicFont = fm.convert(baseFont, toHaveTrait: .italicFontMask)
        if italicFont == baseFont || !fm.traits(of: italicFont).contains(.italicFontMask) {
            // Monospaced system fonts often lack italics; use an oblique transform.
            let desc = baseFont.fontDescriptor.withMatrix(AffineTransform(m11: 1, m12: 0, m21: 0.2, m22: 1, tX: 0, tY: 0))
            italicFont = NSFont(descriptor: desc, size: size) ?? baseFont
        }
        boldItalicFont = NSFont(descriptor: boldFont.fontDescriptor.withMatrix(AffineTransform(m11: 1, m12: 0, m21: 0.2, m22: 1, tX: 0, tY: 0)), size: size) ?? boldFont
        codeFont = baseFont.isFixedPitch ? baseFont : .monospacedSystemFont(ofSize: size * 0.95, weight: .regular)
    }

    /// Extra space after a line (inline images and diagrams), kept when lines are re-highlighted.
    var spacingAfterLine: ((Int) -> CGFloat?)?

    /// The base paragraph style, with room after the line when `spacingAfter` is set.
    func paragraphStyle(spacingAfter: CGFloat?) -> NSParagraphStyle {
        guard let s = spacingAfter, s > 0 else { return paragraphStyle }
        let ps = paragraphStyle.mutableCopy() as! NSMutableParagraphStyle
        ps.paragraphSpacing = s
        return ps
    }

    var baseAttributes: [NSAttributedString.Key: Any] {
        [.font: baseFont, .foregroundColor: NSColor.textColor, .paragraphStyle: paragraphStyle]
    }

    private static let rx: (String) -> NSRegularExpression = { try! NSRegularExpression(pattern: $0, options: [.anchorsMatchLines]) }
    private static let headingRx = rx("^(#{1,6})(\\s+.*)?$")
    private static let quoteRx = rx("^(\\s*>)+")
    private static let listRx = rx("^\\s*([-*+]|\\d+[.)])\\s+(\\[[ xX]\\]\\s)?")
    private static let hrRx = rx("^\\s{0,3}([-*_])(\\s*\\1){2,}\\s*$")
    private static let codeSpanRx = rx("(`+)(?!`)(.+?)(?<!`)\\1(?!`)")
    private static let boldRx = rx("(\\*\\*|__)(?=\\S)(.+?)(?<=\\S)\\1")
    private static let italicRx = rx("(?<![*_\\w\\\\])([*_])(?![\\s*_])(.+?)(?<![\\s*_\\\\])\\1(?![*_\\w])")
    private static let strikeRx = rx("~~(?=\\S)(.+?)(?<=\\S)~~")
    private static let linkRx = rx("(!?)\\[([^\\]\\n]*)\\]\\(([^)\\n]*)\\)")
    private static let refLinkRx = rx("^\\s{0,3}\\[[^\\]]+\\]:\\s+\\S+.*$")
    private static let autolinkRx = rx("<(https?://[^>\\s]+)>|(?<![(<\\w])(https?://[^\\s)>\\]]+)")
    /// Explicit `#tags` (same rules as NoteTags.extract: after start, whitespace or punctuation; starts with a letter).
    private static let tagRx = rx("(?<![\\p{L}\\p{N}_/&#\\\\:=\"'`])#\\p{L}[\\p{L}\\p{N}_/-]*")
    private static let htmlRx = rx("</?[a-zA-Z][a-zA-Z0-9-]*(\\s[^>\\n]*)?/?>")
    private static let footnoteRx = rx("\\[\\^[^\\]\\s]+\\]")
    private static let mathRx = rx("(?<![\\\\$])\\$(?!\\s)([^$\\n]+?)(?<!\\s)\\$(?!\\d)|\\$\\$")
    private static let tablePipeRx = rx("(?<!\\\\)\\|")
    private static let imgTagRx = rx("<img\\s[^>]*src=\"([^\"]+)\"")

    /// Re-applies attributes to whole lines in `lineRange` (line indices, inclusive).
    func highlight(storage: NSTextStorage, scan: BlockScanner, lines: ClosedRange<Int>) {
        let ns = storage.string as NSString
        guard ns.length > 0, !scan.lineStarts.isEmpty else { return }
        let first = max(0, lines.lowerBound)
        let last = min(scan.lineStarts.count - 1, lines.upperBound)
        guard first <= last else { return }
        let start = scan.lineStarts[first]
        let end = last + 1 < scan.lineStarts.count ? scan.lineStarts[last + 1] : ns.length
        let range = NSRange(location: start, length: end - start)
        guard range.length > 0 else { return }

        storage.beginEditing()
        storage.setAttributes(baseAttributes, range: range)

        var plainRuns: [NSRange] = []
        var runStart: Int? = nil
        var inTable = false
        for li in first...last {
            let ls = scan.lineStarts[li]
            let le = li + 1 < scan.lineStarts.count ? scan.lineStarts[li + 1] : ns.length
            let lr = NSRange(location: ls, length: le - ls)
            let isBlock = scan.isInCodeOrFrontMatter(line: li)
            if isBlock {
                if let s = runStart { plainRuns.append(NSRange(location: s, length: ls - s)); runStart = nil }
                if let fm = scan.frontMatter, fm.contains(li) {
                    storage.addAttribute(.foregroundColor, value: Self.frontMatterColor, range: lr)
                } else if let bi = scan.codeBlockIndex(containing: li) {
                    let block = scan.codeBlocks[bi]
                    storage.addAttribute(.font, value: codeFont, range: lr)
                    storage.addAttribute(.backgroundColor, value: Self.codeBackground, range: lr)
                    if li == block.lowerBound || li == block.upperBound {
                        storage.addAttribute(.foregroundColor, value: Self.markerColor, range: lr)
                    } else {
                        let lang = scan.codeLanguages[bi].lowercased()
                        let color = lang.isEmpty ? NSColor.secondaryLabelColor : Self.fencePalette[lang.unicodeScalars.reduce(0) { $0 &+ Int($1.value) } % Self.fencePalette.count]
                        storage.addAttribute(.foregroundColor, value: color, range: lr)
                    }
                }
                inTable = false
                continue
            }
            if runStart == nil { runStart = ls }
            let line = ns.substring(with: lr)
            highlightLine(storage, line: line, lineRange: lr, inTable: &inTable)
        }
        if let s = runStart { plainRuns.append(NSRange(location: s, length: end - s)) }
        for run in plainRuns { highlightInline(storage, ns: ns, range: run) }
        if let spacing = spacingAfterLine {
            for li in first...last {
                guard let sp = spacing(li), sp > 0 else { continue }
                let ls = scan.lineStarts[li]
                let le = li + 1 < scan.lineStarts.count ? scan.lineStarts[li + 1] : ns.length
                if le > ls { storage.addAttribute(.paragraphStyle, value: paragraphStyle(spacingAfter: sp), range: NSRange(location: ls, length: le - ls)) }
            }
        }
        storage.endEditing()
    }

    private func highlightLine(_ storage: NSTextStorage, line: String, lineRange lr: NSRange, inTable: inout Bool) {
        let lns = line as NSString
        let local = NSRange(location: 0, length: lns.length)
        func abs(_ r: NSRange) -> NSRange { NSRange(location: lr.location + r.location, length: r.length) }

        if line.hasPrefix("<!--") || line.hasPrefix(" "), AppSettings.shared.dimIslandMarkers,
           let m = Islands.marker(inLine: line.trimmingCharacters(in: .newlines)) {
            // Island marker: dimmed, with the name emphasised.
            storage.addAttribute(.foregroundColor, value: Self.markerColor, range: lr)
            let colon = lns.range(of: ":")
            let nameRange = lns.range(of: m.name, range: NSRange(location: NSMaxRange(colon), length: lns.length - NSMaxRange(colon)))
            if nameRange.location != NSNotFound {
                storage.addAttribute(.foregroundColor, value: NSColor.secondaryLabelColor, range: abs(nameRange))
                storage.addAttribute(.font, value: boldFont, range: abs(nameRange))
            }
            return
        }
        if let m = Self.headingRx.firstMatch(in: line, range: local) {
            storage.addAttribute(.font, value: boldFont, range: lr)
            storage.addAttribute(.foregroundColor, value: Self.headingColor, range: lr)
            storage.addAttribute(.foregroundColor, value: Self.markerColor, range: abs(m.range(at: 1)))
            return
        }
        if Self.hrRx.firstMatch(in: line, range: local) != nil {
            storage.addAttribute(.foregroundColor, value: Self.markerColor, range: lr)
            return
        }
        if Self.refLinkRx.firstMatch(in: line, range: local) != nil {
            storage.addAttribute(.foregroundColor, value: Self.urlColor, range: lr)
            return
        }
        if let m = Self.quoteRx.firstMatch(in: line, range: local) {
            storage.addAttribute(.foregroundColor, value: Self.quoteColor, range: lr)
            storage.addAttribute(.foregroundColor, value: Self.markerColor, range: abs(m.range))
            if line.range(of: "^\\s*>\\s*\\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\\]", options: [.regularExpression, .caseInsensitive]) != nil {
                storage.addAttribute(.font, value: boldFont, range: lr)
            }
        }
        if let m = Self.listRx.firstMatch(in: line, range: local) {
            storage.addAttribute(.foregroundColor, value: Self.listColor, range: abs(m.range(at: 1)))
            if m.range(at: 2).location != NSNotFound {
                storage.addAttribute(.foregroundColor, value: Self.listColor, range: abs(m.range(at: 2)))
                if line.range(of: "\\[[xX]\\]", options: .regularExpression) != nil {
                    let contentStart = m.range.length
                    if contentStart < lns.length {
                        storage.addAttribute(.foregroundColor, value: NSColor.secondaryLabelColor, range: NSRange(location: lr.location + contentStart, length: lns.length - contentStart))
                    }
                }
            }
        }
        let isTableRow = MarkdownTable.looksLikeRow(line)
        if isTableRow {
            if MarkdownTable.isDelimiterRow(line) {
                storage.addAttribute(.foregroundColor, value: Self.markerColor, range: lr)
                inTable = true
            } else {
                for m in Self.tablePipeRx.matches(in: line, range: local) {
                    storage.addAttribute(.foregroundColor, value: Self.markerColor, range: abs(m.range))
                }
            }
        } else {
            inTable = false
        }
    }

    private func highlightInline(_ storage: NSTextStorage, ns: NSString, range: NSRange) {
        let text = storage.string
        var codeRanges: [NSRange] = []
        for m in Self.codeSpanRx.matches(in: text, range: range) {
            storage.addAttribute(.font, value: codeFont, range: m.range)
            storage.addAttribute(.foregroundColor, value: Self.codeColor, range: m.range(at: 2))
            storage.addAttribute(.backgroundColor, value: Self.codeBackground, range: m.range)
            storage.addAttribute(.foregroundColor, value: Self.markerColor, range: m.range(at: 1))
            storage.addAttribute(.foregroundColor, value: Self.markerColor, range: NSRange(location: NSMaxRange(m.range) - m.range(at: 1).length, length: m.range(at: 1).length))
            codeRanges.append(m.range)
        }
        // Code spans come back in order and don't overlap, so a binary search finds the nearest one.
        func inCode(_ r: NSRange) -> Bool {
            var lo = 0, hi = codeRanges.count
            while lo < hi {
                let mid = (lo + hi) / 2
                if NSMaxRange(codeRanges[mid]) <= r.location { lo = mid + 1 } else { hi = mid }
            }
            return lo < codeRanges.count && NSIntersectionRange(codeRanges[lo], r).length > 0
        }

        for m in Self.boldRx.matches(in: text, range: range) where !inCode(m.range) {
            storage.addAttribute(.font, value: boldFont, range: m.range)
            markDelimiters(storage, m.range, m.range(at: 1).length)
        }
        for m in Self.italicRx.matches(in: text, range: range) where !inCode(m.range) {
            let current = storage.attribute(.font, at: m.range.location + 1, effectiveRange: nil) as? NSFont
            storage.addAttribute(.font, value: current == boldFont ? boldItalicFont : italicFont, range: m.range)
            markDelimiters(storage, m.range, 1)
        }
        for m in Self.strikeRx.matches(in: text, range: range) where !inCode(m.range) {
            storage.addAttribute(.strikethroughStyle, value: NSUnderlineStyle.single.rawValue, range: m.range)
            markDelimiters(storage, m.range, 2)
        }
        for m in Self.linkRx.matches(in: text, range: range) where !inCode(m.range) {
            storage.addAttribute(.foregroundColor, value: Self.markerColor, range: m.range)
            storage.addAttribute(.foregroundColor, value: Self.linkColor, range: m.range(at: 2))
            storage.addAttribute(.foregroundColor, value: Self.urlColor, range: m.range(at: 3))
            if m.range(at: 1).length == 1, let exists = imageExists {
                let raw = ns.substring(with: m.range(at: 3))
                if !exists(raw) {
                    storage.addAttributes([.underlineStyle: NSUnderlineStyle.single.rawValue | NSUnderlineStyle.patternDot.rawValue,
                                           .underlineColor: NSColor.systemRed], range: m.range(at: 3))
                }
            }
        }
        for m in Self.imgTagRx.matches(in: text, range: range) where !inCode(m.range) {
            if let exists = imageExists, !exists(ns.substring(with: m.range(at: 1))) {
                storage.addAttributes([.underlineStyle: NSUnderlineStyle.single.rawValue | NSUnderlineStyle.patternDot.rawValue,
                                       .underlineColor: NSColor.systemRed], range: m.range(at: 1))
            }
        }
        for m in Self.autolinkRx.matches(in: text, range: range) where !inCode(m.range) {
            storage.addAttribute(.foregroundColor, value: Self.linkColor, range: m.range)
        }
        for m in Self.footnoteRx.matches(in: text, range: range) where !inCode(m.range) {
            storage.addAttribute(.foregroundColor, value: Self.linkColor, range: m.range)
        }
        for m in Self.htmlRx.matches(in: text, range: range) where !inCode(m.range) {
            storage.addAttribute(.foregroundColor, value: Self.htmlColor, range: m.range)
        }
        for m in Self.mathRx.matches(in: text, range: range) where !inCode(m.range) {
            storage.addAttribute(.foregroundColor, value: Self.mathColor, range: m.range)
        }
    }

    private func markDelimiters(_ storage: NSTextStorage, _ r: NSRange, _ len: Int) {
        guard r.length >= 2 * len else { return }
        storage.addAttribute(.foregroundColor, value: Self.markerColor, range: NSRange(location: r.location, length: len))
        storage.addAttribute(.foregroundColor, value: Self.markerColor, range: NSRange(location: NSMaxRange(r) - len, length: len))
    }
}
