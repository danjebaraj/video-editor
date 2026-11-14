import Foundation

/// Path helpers for Claude workspaces and folders Claude may read.
enum FileIndex {
    static func isMarkdown(_ url: URL) -> Bool {
        ["md", "markdown"].contains(url.pathExtension.lowercased())
    }

    static func markdownFiles(in root: URL, limit: Int = 5000) -> [URL] {
        var result: [URL] = []
        guard let e = FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.isDirectoryKey], options: [.skipsHiddenFiles, .skipsPackageDescendants]) else { return [] }
        for case let url as URL in e {
            if url.lastPathComponent == "assets" { e.skipDescendants(); continue }
            if isMarkdown(url) { result.append(url) }
            if result.count >= limit { break }
        }
        return result
    }

    static func relativePath(_ url: URL, root: URL) -> String {
        let r = root.standardizedFileURL.path
        let p = url.standardizedFileURL.path
        if p.hasPrefix(r + "/") { return String(p.dropFirst(r.count + 1)) }
        let cr = canonicalPath(root), cp = canonicalPath(url)
        if cp.hasPrefix(cr + "/") { return String(cp.dropFirst(cr.count + 1)) }
        return p
    }

    static func isInside(_ url: URL, root: URL) -> Bool {
        let r = canonicalPath(root)
        let p = canonicalPath(url)
        return p == r || p.hasPrefix(r.hasSuffix("/") ? r : r + "/")
    }

    /// Resolves symlinks even for paths that don't exist yet (resolves the deepest existing ancestor).
    static func canonicalPath(_ url: URL) -> String {
        var existing = url.standardizedFileURL
        var rest: [String] = []
        while !FileManager.default.fileExists(atPath: existing.path) && existing.path != "/" {
            rest.insert(existing.lastPathComponent, at: 0)
            existing.deleteLastPathComponent()
        }
        var resolved = existing.resolvingSymlinksInPath()
        for c in rest { resolved.appendPathComponent(c) }
        return resolved.standardizedFileURL.path
    }
}
