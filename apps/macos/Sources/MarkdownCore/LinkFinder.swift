import Foundation

/// Finds the link under a position in a line, for ⌘-click in the editor.
public enum LinkFinder {
    public struct Link: Equatable, Sendable {
        /// The link target as written (decoded for Markdown links).
        public var target: String
        /// The link's range in the line (UTF-16).
        public var range: NSRange
        public var isImage: Bool
    }

    private static let mdLinkRx = try! NSRegularExpression(pattern: "(!?)\\[(?:[^\\[\\]\\n]|\\[[^\\]\\n]*\\])*\\]\\(([^()\\n]*(?:\\([^()\\n]*\\)[^()\\n]*)*)\\)")
    private static let autolinkRx = try! NSRegularExpression(pattern: "<((?:https?|ftp)://[^>\\s]+|mailto:[^>\\s]+)>", options: .caseInsensitive)
    private static let bareRx = try! NSRegularExpression(pattern: "(?<![\\w/@.])(?:(?:https?|ftp)://|www\\.|mailto:)[^\\s<>\"'`]+", options: .caseInsensitive)

    /// The link at `offset` in `line`, or nil. Markdown links win over a URL inside them.
    public static func link(in line: String, at offset: Int) -> Link? {
        let ns = line as NSString
        let full = NSRange(location: 0, length: ns.length)
        func hit(_ r: NSRange) -> Bool { offset >= r.location && offset < NSMaxRange(r) }
        for m in mdLinkRx.matches(in: line, range: full) where hit(m.range) {
            let target = ImageNaming.decodeMarkdownPath(ns.substring(with: m.range(at: 2)))
            return Link(target: target, range: m.range, isImage: m.range(at: 1).length > 0)
        }
        for m in autolinkRx.matches(in: line, range: full) where hit(m.range) {
            return Link(target: ns.substring(with: m.range(at: 1)), range: m.range, isImage: false)
        }
        for m in bareRx.matches(in: line, range: full) {
            var r = m.range
            var s = ns.substring(with: r)
            // Trailing punctuation belongs to the sentence; a closing ) only when unbalanced.
            while let last = s.last, ".,;:!?*_~".contains(last) || (last == ")" && s.filter({ $0 == ")" }).count > s.filter({ $0 == "(" }).count) {
                s.removeLast(); r.length -= 1
            }
            guard hit(r), !s.isEmpty else { continue }
            if s.lowercased().hasPrefix("www.") { s = "https://" + s }
            return Link(target: s, range: r, isImage: false)
        }
        return nil
    }

    /// A URL the system should open (web, mail, ftp), or nil for a path inside the notes.
    public static func externalURL(_ target: String) -> URL? {
        let lower = target.lowercased()
        guard lower.hasPrefix("http://") || lower.hasPrefix("https://") || lower.hasPrefix("mailto:") || lower.hasPrefix("ftp://") else { return nil }
        return URL(string: target) ?? URL(string: target.addingPercentEncoding(withAllowedCharacters: .urlFragmentAllowed.union(.urlQueryAllowed)) ?? "")
    }
}
