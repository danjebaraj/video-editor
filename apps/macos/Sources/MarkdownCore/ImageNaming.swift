import Foundation

public enum ImagePathStyle: String, Codable, Sendable, CaseIterable {
    case angleBrackets, urlEncoded
}

/// Naming and path rules for images copied into the assets folder.
public enum ImageNaming {
    public static let supportedExtensions: Set<String> = ["png", "jpg", "jpeg", "gif", "webp", "svg", "heic", "heif", "bmp", "tiff", "tif"]

    public static func isImage(_ url: URL) -> Bool {
        supportedExtensions.contains(url.pathExtension.lowercased())
    }

    /// Lowercases and replaces whitespace runs with `-` when `normalize` is on.
    public static func sanitize(_ filename: String, normalize: Bool) -> String {
        var name = filename.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
        guard normalize else { return name }
        name = name.lowercased()
        name = name.replacingOccurrences(of: "\\s+", with: "-", options: .regularExpression)
        return name
    }

    /// Name for a pasted image with no filename: `image-YYYYMMDD-HHMMSS.png`.
    public static func pastedName(date: Date = Date(), ext: String = "png", timeZone: TimeZone = .current) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = timeZone
        f.dateFormat = "yyyyMMdd-HHmmss"
        return "image-\(f.string(from: date)).\(ext)"
    }

    public enum Placement: Equatable {
        /// Write the data to this filename.
        case write(String)
        /// An identical file already exists with this name; reference it.
        case reuse(String)
    }

    /// Chooses a free filename in the assets folder, adding `-1`, `-2`… suffixes, or reuses a byte-identical file.
    /// - Parameters:
    ///   - exists: whether a file with the name already exists in the assets folder.
    ///   - identical: whether the existing file with that name is byte-identical to the new data.
    public static func placement(for name: String, exists: (String) -> Bool, identical: (String) -> Bool) -> Placement {
        let ext = (name as NSString).pathExtension
        let base = (name as NSString).deletingPathExtension
        var candidate = name
        var n = 0
        while exists(candidate) {
            if identical(candidate) { return .reuse(candidate) }
            n += 1
            candidate = ext.isEmpty ? "\(base)-\(n)" : "\(base)-\(n).\(ext)"
        }
        return .write(candidate)
    }

    /// Relative path from a directory to a file, using `/` separators and `..` where needed.
    public static func relativePath(from directory: URL, to file: URL) -> String {
        let from = directory.standardizedFileURL.resolvingSymlinksInPath().pathComponents
        let to = file.standardizedFileURL.resolvingSymlinksInPath().pathComponents
        var i = 0
        while i < from.count && i < to.count && from[i] == to[i] { i += 1 }
        let ups = Array(repeating: "..", count: from.count - i)
        return (ups + to[i...]).joined(separator: "/")
    }

    /// Formats a path for Markdown link syntax. Paths with spaces are wrapped or encoded.
    public static func markdownPath(_ path: String, style: ImagePathStyle) -> String {
        guard path.contains(" ") || path.contains("(") || path.contains(")") else { return path }
        switch style {
        case .angleBrackets:
            return "<\(path)>"
        case .urlEncoded:
            var allowed = CharacterSet.urlPathAllowed
            allowed.remove(charactersIn: "()")
            return path.addingPercentEncoding(withAllowedCharacters: allowed) ?? path
        }
    }

    /// Inverse of `markdownPath`: strips angle brackets and percent-decoding.
    public static func decodeMarkdownPath(_ raw: String) -> String {
        var s = raw.trimmingCharacters(in: .whitespaces)
        // <path with spaces>, optionally followed by a title.
        if s.hasPrefix("<"), let close = s.firstIndex(of: ">") {
            s = String(s[s.index(after: s.startIndex)..<close])
            return s.removingPercentEncoding ?? s
        }
        // Drop an optional title: path "title"
        if let r = s.range(of: " \"") { s = String(s[..<r.lowerBound]) }
        return s.removingPercentEncoding ?? s
    }
}
