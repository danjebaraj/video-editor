import Foundation

/// A parsed `[[Page#Heading|Alias]]` link.
public struct WikiLink: Equatable, Sendable {
    public var page: String
    public var heading: String?
    public var alias: String?
    /// UTF-16 range of the whole `[[…]]` in the source.
    public var range: NSRange

    public var displayText: String { alias ?? (heading.map { "\(page)#\($0)" } ?? page) }

    public static let regex = try! NSRegularExpression(pattern: "\\[\\[([^\\[\\]\\n|#]*)(?:#([^\\[\\]\\n|]*))?(?:\\|([^\\[\\]\\n]*))?\\]\\]")

    public static func parseAll(in text: String) -> [WikiLink] {
        let ns = text as NSString
        return regex.matches(in: text, range: NSRange(location: 0, length: ns.length)).compactMap { m in
            func group(_ i: Int) -> String? {
                let r = m.range(at: i)
                guard r.location != NSNotFound else { return nil }
                let s = ns.substring(with: r).trimmingCharacters(in: .whitespaces)
                return s.isEmpty ? nil : s
            }
            guard let page = group(1) else { return nil }
            return WikiLink(page: page, heading: group(2), alias: group(3), range: m.range)
        }
    }
}

/// Resolves wiki-link page names to Markdown files in a folder.
public struct WikiLinkResolver: Sendable {
    /// Paths of Markdown files relative to the root folder (e.g. `notes/Page.md`).
    public let files: [String]

    public init(files: [String]) {
        self.files = files
    }

    static func stem(_ path: String) -> String {
        ((path as NSString).lastPathComponent as NSString).deletingPathExtension
    }

    /// Resolves `page` relative to the directory of the current file (also relative to root).
    /// Prefers a file in the same directory, then the shortest path. Page may include a subpath (`dir/Page`).
    public func resolve(_ page: String, fromDirectory currentDir: String) -> String? {
        let target = page.lowercased()
        let hasExt = target.hasSuffix(".md") || target.hasSuffix(".markdown")
        let candidates = files.filter { f in
            let lower = f.lowercased()
            if target.contains("/") {
                let noExt = (lower as NSString).deletingPathExtension
                return hasExt ? (lower == target || lower.hasSuffix("/" + target)) : (noExt == target || noExt.hasSuffix("/" + target))
            }
            return hasExt ? (lower as NSString).lastPathComponent == target : Self.stem(lower) == target
        }
        guard !candidates.isEmpty else { return nil }
        let dir = currentDir.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        if let same = candidates.first(where: { (($0 as NSString).deletingLastPathComponent) == dir }) {
            return same
        }
        return candidates.min { a, b in
            let ca = a.split(separator: "/").count, cb = b.split(separator: "/").count
            return ca != cb ? ca < cb : a.count != b.count ? a.count < b.count : a < b
        }
    }

    /// Converts a heading's text to a GitHub-style anchor slug.
    public static func slug(_ heading: String) -> String {
        let lowered = heading.lowercased()
        var out = ""
        for ch in lowered {
            if ch.isLetter || ch.isNumber || ch == "_" || ch == "-" { out.append(ch) } else if ch == " " { out.append("-") }
        }
        return out
    }
}
