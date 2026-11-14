import Foundation
import CryptoKit
import ImageIO
import UniformTypeIdentifiers

// Image embedding: validation, PNG conversion, storage next to the note, references in Markdown
// and the path rules the preview uses. See Image-Embedding-Spec.md.

public enum ImageIngestError: Error, Equatable, Sendable {
    /// Not decodable, not an image, or outside the pixel limits.
    case invalid
    /// Over the byte limits, or still over 20 MB after downscaling.
    case tooLarge

    public var message: String {
        switch self {
        case .invalid: return "Choose a supported image."
        case .tooLarge: return "Stored images must be smaller than 20 MB. With PNG conversion enabled, source images can be up to 100 MB."
        }
    }
}

public enum ImageLimits {
    public static let maxInputConverting = 100 * 1024 * 1024
    public static let maxStored = 20 * 1024 * 1024
    public static let maxSide = 16_000
    public static let maxPixels = 50_000_000
    /// Longest side of a converted image, halved while the PNG is too large.
    public static let convertSides = [4096, 2048, 1024]

    public static func maxInput(converting: Bool) -> Int { converting ? maxInputConverting : maxStored }
}

/// An image ready to store: the bytes to write, their type and the display filename.
public struct PreparedImage: Equatable, Sendable {
    public var data: Data
    /// Preferred lowercase extension of the stored type (`png`, `jpeg`).
    public var ext: String
    public var mime: String
    /// Display name and alt text: the source's base name plus `ext`.
    public var filename: String
    public var width: Int
    public var height: Int
    public var sha256: String { ImageIngest.sha256(data) }
}

public enum ImageIngest {
    public struct Header: Equatable, Sendable {
        public var type: UTType
        public var width: Int
        public var height: Int
    }

    public static func sha256(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    /// Reads only the image header and checks the type and dimension caps.
    public static func inspect(_ data: Data) throws -> Header {
        if isSVG(data) { return Header(type: .svg, width: 0, height: 0) }
        let noCache = [kCGImageSourceShouldCache: false] as CFDictionary
        guard let src = CGImageSourceCreateWithData(data as CFData, noCache),
              let id = CGImageSourceGetType(src) as String?, let type = UTType(id), type.conforms(to: .image),
              CGImageSourceGetCount(src) > 0 else { throw ImageIngestError.invalid }
        let index = CGImageSourceGetPrimaryImageIndex(src)
        guard let props = CGImageSourceCopyPropertiesAtIndex(src, index, noCache) as? [CFString: Any],
              let w = (props[kCGImagePropertyPixelWidth] as? NSNumber)?.intValue,
              let h = (props[kCGImagePropertyPixelHeight] as? NSNumber)?.intValue,
              (1...ImageLimits.maxSide).contains(w), (1...ImageLimits.maxSide).contains(h),
              w * h <= ImageLimits.maxPixels else { throw ImageIngestError.invalid }
        return Header(type: type, width: w, height: h)
    }

    /// The MIME type and extension sniffed from the bytes, or nil if they aren't an image.
    public static func sniff(_ data: Data) -> (mime: String, ext: String)? {
        if isSVG(data) { return ("image/svg+xml", "svg") }
        guard let src = CGImageSourceCreateWithData(data as CFData, [kCGImageSourceShouldCache: false] as CFDictionary),
              let id = CGImageSourceGetType(src) as String?, let type = UTType(id), type.conforms(to: .image) else { return nil }
        return (type.preferredMIMEType ?? "application/octet-stream", type.preferredFilenameExtension?.lowercased() ?? "img")
    }

    /// SVG can't be decoded by ImageIO; it is accepted as-is and never converted.
    static func isSVG(_ data: Data) -> Bool {
        guard let head = String(data: data.prefix(1024), encoding: .utf8)?.lowercased() else { return false }
        let t = head.drop { $0.isWhitespace || $0 == "\u{feff}" }
        return (t.hasPrefix("<?xml") || t.hasPrefix("<svg") || t.hasPrefix("<!doctype svg")) && head.contains("<svg")
    }

    /// Validates `data` and either converts it to PNG (primary frame, upright, longest side ≤ 4096,
    /// halved while over 20 MB) or keeps the original bytes.
    public static func prepare(_ data: Data, sourceName: String, convertToPNG: Bool) throws -> PreparedImage {
        let svg = isSVG(data)
        guard data.count <= ImageLimits.maxInput(converting: convertToPNG && !svg) else { throw ImageIngestError.tooLarge }
        let header = try inspect(data)
        if !convertToPNG || svg {
            guard data.count <= ImageLimits.maxStored else { throw ImageIngestError.tooLarge }
            let ext = svg ? "svg" : header.type.preferredFilenameExtension?.lowercased() ?? "img"
            let mime = svg ? "image/svg+xml" : header.type.preferredMIMEType ?? "application/octet-stream"
            return PreparedImage(data: data, ext: ext, mime: mime, filename: storedFilename(sourceName, ext: ext),
                                 width: header.width, height: header.height)
        }
        guard let src = CGImageSourceCreateWithData(data as CFData, nil) else { throw ImageIngestError.invalid }
        let index = CGImageSourceGetPrimaryImageIndex(src)
        for side in ImageLimits.convertSides {
            let options: [CFString: Any] = [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceShouldCacheImmediately: true,
                kCGImageSourceThumbnailMaxPixelSize: min(side, max(header.width, header.height)),
            ]
            guard let image = CGImageSourceCreateThumbnailAtIndex(src, index, options as CFDictionary) else { throw ImageIngestError.invalid }
            let out = NSMutableData()
            guard let dest = CGImageDestinationCreateWithData(out, UTType.png.identifier as CFString, 1, nil) else { throw ImageIngestError.invalid }
            CGImageDestinationAddImage(dest, image, nil)
            guard CGImageDestinationFinalize(dest) else { throw ImageIngestError.invalid }
            if out.length <= ImageLimits.maxStored {
                return PreparedImage(data: out as Data, ext: "png", mime: "image/png", filename: storedFilename(sourceName, ext: "png"),
                                     width: image.width, height: image.height)
            }
        }
        throw ImageIngestError.tooLarge
    }

    /// The source's base name plus the stored type's extension, with path separators and control
    /// characters removed. Empty, `.` and `..` become `attachment`; long names are shortened.
    public static func storedFilename(_ sourceName: String, ext: String) -> String {
        let last = sourceName.split(whereSeparator: { $0 == "/" || $0 == "\\" }).last.map(String.init) ?? ""
        var base = (last as NSString).deletingPathExtension
        base = String(base.unicodeScalars.filter { !CharacterSet.controlCharacters.contains($0) && $0 != ":" }.map(Character.init))
        base = base.trimmingCharacters(in: .whitespaces)
        if base.isEmpty || base == "." || base == ".." { base = "attachment" }
        var e = String(ext.lowercased().unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) }.map(Character.init))
        if base.count + 1 + e.count > 180 {
            base = String(base.prefix(160))
            e = String(e.prefix(16))
        }
        return e.isEmpty ? base : "\(base).\(e)"
    }

    /// `<hash>.<ext>` names, checked before a hash-named file is written.
    public static func isHashName(_ name: String) -> Bool {
        name.range(of: "^[0-9a-f]{64}\\.[a-z0-9]{1,16}$", options: .regularExpression) != nil
    }

    public static func hashName(for image: PreparedImage) -> String { "\(image.sha256).\(image.ext)" }
}

// MARK: Image folder

public enum ImageFolder {
    public static let defaultName = "assets"

    /// The folder for new images, relative to the note's folder, or nil if `raw` isn't allowed:
    /// empty, absolute, `.` or `..` components, or control characters.
    public static func validated(_ raw: String) -> String? {
        let t = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty, !t.hasPrefix("/"), !t.hasPrefix("~"), !t.contains("\\"),
              t.unicodeScalars.allSatisfy({ !CharacterSet.controlCharacters.contains($0) }) else { return nil }
        let parts = t.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
        guard !parts.isEmpty, parts.allSatisfy({ $0 != "." && $0 != ".." && !$0.trimmingCharacters(in: .whitespaces).isEmpty }) else { return nil }
        return parts.joined(separator: "/")
    }

    /// `validated(raw)`, falling back to `assets`.
    public static func resolve(_ raw: String) -> String { validated(raw) ?? defaultName }
}

// MARK: Storing

public enum ImageStore {
    public enum Naming: Sendable { case original(normalize: Bool), hash }

    /// Writes `image` into `<noteDirectory>/<folder>` (created if needed) and returns the file.
    /// Content already in the folder is reused, whatever its name, so the same picture is stored once.
    /// Readable names get `-1`, `-2`… on a clash; hash names are validated before writing.
    public static func store(_ image: PreparedImage, noteDirectory: URL, folder: String, naming: Naming,
                             fileManager fm: FileManager = .default) throws -> URL {
        let dir = noteDirectory.appendingPathComponent(ImageFolder.resolve(folder), isDirectory: true)
        try fm.createDirectory(at: dir, withIntermediateDirectories: true)
        let name: String
        switch naming {
        case .hash:
            name = ImageIngest.hashName(for: image)
            guard ImageIngest.isHashName(name) else { throw ImageIngestError.invalid }
            let dest = dir.appendingPathComponent(name)
            if fm.fileExists(atPath: dest.path) { return dest }
            try image.data.write(to: dest, options: .atomic)
            return dest
        case .original(let normalize):
            if let existing = identicalFile(to: image.data, in: dir, fileManager: fm) { return existing }
            name = ImageNaming.sanitize(image.filename, normalize: normalize)
        }
        let placement = ImageNaming.placement(for: name,
                                              exists: { fm.fileExists(atPath: dir.appendingPathComponent($0).path) },
                                              identical: { (try? Data(contentsOf: dir.appendingPathComponent($0))) == image.data })
        switch placement {
        case .reuse(let n):
            return dir.appendingPathComponent(n)
        case .write(let n):
            let dest = dir.appendingPathComponent(n)
            try image.data.write(to: dest, options: .atomic)
            return dest
        }
    }

    /// A file in `dir` (not its subfolders) with exactly these bytes. Only same-size files are read.
    public static func identicalFile(to data: Data, in dir: URL, fileManager fm: FileManager = .default) -> URL? {
        guard let items = try? fm.contentsOfDirectory(at: dir, includingPropertiesForKeys: [.fileSizeKey, .isRegularFileKey]) else { return nil }
        for url in items.sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) {
            guard let v = try? url.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey]), v.isRegularFile == true,
                  v.fileSize == data.count, (try? Data(contentsOf: url)) == data else { continue }
            return dir.appendingPathComponent(url.lastPathComponent)
        }
        return nil
    }
}

// MARK: References in Markdown

public enum ImageReference {
    /// `![alt](path)` as its own block: wrapped in newlines, alt without `]`.
    public static func block(filename: String, path: String, style: ImagePathStyle) -> String {
        "\n![\(alt(filename))](\(ImageNaming.markdownPath(path, style: style)))\n"
    }

    public static func alt(_ filename: String) -> String {
        filename.replacingOccurrences(of: "]", with: "").replacingOccurrences(of: "\n", with: " ")
    }

    /// `![](https://)` with the caret in the brackets, or `![selection](https://)` with `https://` selected.
    public static func linkTemplate(selection: String) -> (text: String, selection: NSRange) {
        let alt = selection.replacingOccurrences(of: "]", with: "").replacingOccurrences(of: "\n", with: " ")
        let text = "![\(alt)](https://)"
        if alt.isEmpty { return (text, NSRange(location: 2, length: 0)) }
        return (text, NSRange(location: (alt as NSString).length + 4, length: 8))
    }
}

/// Finds image references in Markdown source, skipping fenced and indented code blocks and inline code.
/// Ranges are UTF-16 (NSString) offsets into the source.
public enum ImageReferenceScanner {
    public struct Match: Equatable, Sendable {
        /// The whole `![…](…)` or `<img …>`.
        public var range: NSRange
        /// The destination as written (inside the parentheses or the `src` quotes, without a title).
        public var destinationRange: NSRange
        /// The destination with angle brackets removed and percent-decoding applied.
        public var path: String
        public var alt: String
        public var isHTML: Bool
    }

    public static func matches(in source: String) -> [Match] {
        let ns = source as NSString
        var out: [Match] = []
        var fence: (char: unichar, count: Int)?
        var pos = 0
        var paragraph: [NSRange] = []

        func flushParagraph() {
            if let first = paragraph.first, let last = paragraph.last {
                out += inlineMatches(ns, in: NSRange(location: first.location, length: NSMaxRange(last) - first.location))
            }
            paragraph = []
        }
        while pos < ns.length {
            var lineEnd = 0, contentsEnd = 0
            ns.getLineStart(nil, end: &lineEnd, contentsEnd: &contentsEnd, for: NSRange(location: pos, length: 0))
            let line = NSRange(location: pos, length: contentsEnd - pos)
            let text = ns.substring(with: line)
            let trimmed = text.drop { $0 == " " }
            let indent = text.count - trimmed.count
            if let f = fence {
                let run = trimmed.prefix { $0 == Character(UnicodeScalar(f.char)!) }.count
                if indent < 4, run >= f.count, trimmed.dropFirst(run).allSatisfy({ $0 == " " || $0 == "\t" }) { fence = nil }
            } else if indent < 4, let c = trimmed.first, c == "`" || c == "~" {
                let run = trimmed.prefix { $0 == c }.count
                if run >= 3, !(c == "`" && trimmed.dropFirst(run).contains("`")) {
                    flushParagraph()
                    fence = (c == "`" ? 96 : 126, run)
                } else {
                    paragraph.append(line)
                }
            } else if trimmed.trimmingCharacters(in: .whitespaces).isEmpty {
                flushParagraph()
            } else if (indent >= 4 || text.hasPrefix("\t")) && paragraph.isEmpty {
                // Indented code block.
            } else {
                paragraph.append(line)
            }
            pos = lineEnd
        }
        flushParagraph()
        return out
    }

    /// Image references in a paragraph (its lines are scanned together so code spans can wrap).
    static func inlineMatches(_ ns: NSString, in line: NSRange) -> [Match] {
        var out: [Match] = []
        var i = line.location
        let end = NSMaxRange(line)
        while i < end {
            let c = ns.character(at: i)
            if c == 92 { i += 2; continue } // backslash escape
            if c == 96 { // code span: skip to a closing run of the same length
                var run = 0
                while i + run < end && ns.character(at: i + run) == 96 { run += 1 }
                var j = i + run
                var closed = false
                while j < end {
                    if ns.character(at: j) == 96 {
                        var r = 0
                        while j + r < end && ns.character(at: j + r) == 96 { r += 1 }
                        if r == run { closed = true; j += r; break }
                        j += r
                    } else { j += 1 }
                }
                i = closed ? j : i + run
                continue
            }
            if c == 33, i + 1 < end, ns.character(at: i + 1) == 91, let m = markdownImage(ns, at: i, end: end) {
                out.append(m)
                i = NSMaxRange(m.range)
                continue
            }
            if c == 60, let m = htmlImage(ns, at: i, end: end) {
                out.append(m)
                i = NSMaxRange(m.range)
                continue
            }
            i += 1
        }
        return out
    }

    private static func markdownImage(_ ns: NSString, at start: Int, end: Int) -> Match? {
        // Alt text: balanced brackets, escapes allowed.
        var i = start + 2
        var depth = 1
        while i < end {
            let c = ns.character(at: i)
            if c == 92 { i += 2; continue }
            if c == 91 { depth += 1 }
            if c == 93 { depth -= 1; if depth == 0 { break } }
            i += 1
        }
        guard i < end, depth == 0, i + 1 < end, ns.character(at: i + 1) == 40 else { return nil }
        let alt = ns.substring(with: NSRange(location: start + 2, length: i - start - 2))
        var j = i + 2
        while j < end, ns.character(at: j) == 32 { j += 1 }
        let destStart = j
        var destEnd = j
        if j < end, ns.character(at: j) == 60 { // <destination>
            while j < end, ns.character(at: j) != 62, ns.character(at: j) != 10 { j += 1 }
            guard j < end, ns.character(at: j) == 62 else { return nil }
            j += 1
            destEnd = j
        } else {
            var parens = 0
            while j < end {
                let c = ns.character(at: j)
                if c == 92 { j += 2; continue }
                if c == 32 || c == 9 || c == 10 || c == 13 { break }
                if c == 40 { parens += 1 }
                if c == 41 { if parens == 0 { break }; parens -= 1 }
                j += 1
            }
            destEnd = min(j, end)
        }
        // Optional title, then the closing parenthesis.
        while j < end, ns.character(at: j) != 41 {
            let c = ns.character(at: j)
            if c == 34 || c == 39 {
                j += 1
                while j < end, ns.character(at: j) != c { j += ns.character(at: j) == 92 ? 2 : 1 }
            }
            j += 1
        }
        guard j < end else { return nil }
        let destRange = NSRange(location: destStart, length: destEnd - destStart)
        return Match(range: NSRange(location: start, length: j + 1 - start), destinationRange: destRange,
                     path: ImageNaming.decodeMarkdownPath(ns.substring(with: destRange)), alt: alt, isHTML: false)
    }

    private static let imgTag = try! NSRegularExpression(pattern: "\\G<img\\s[^>]*?\\bsrc\\s*=\\s*\"([^\"]*)\"[^>]*>", options: .caseInsensitive)
    private static let altAttr = try! NSRegularExpression(pattern: "\\balt\\s*=\\s*\"([^\"]*)\"", options: .caseInsensitive)

    private static func htmlImage(_ ns: NSString, at start: Int, end: Int) -> Match? {
        guard let m = imgTag.firstMatch(in: ns as String, options: .anchored, range: NSRange(location: start, length: end - start)) else { return nil }
        let tag = ns.substring(with: m.range)
        let alt = altAttr.firstMatch(in: tag, range: NSRange(location: 0, length: (tag as NSString).length)).map { (tag as NSString).substring(with: $0.range(at: 1)) } ?? ""
        let raw = ns.substring(with: m.range(at: 1))
        return Match(range: m.range, destinationRange: m.range(at: 1), path: raw.removingPercentEncoding ?? raw, alt: alt, isHTML: true)
    }

    /// Removes every image reference whose decoded path is `path`, working from the end.
    /// References in code and to other files are kept.
    public static func removingReferences(to path: String, in source: String) -> String {
        let ns = NSMutableString(string: source)
        for m in matches(in: source).reversed() where m.path == path {
            ns.replaceCharacters(in: m.range, with: "")
        }
        return ns as String
    }
}

// MARK: Paste and drop sources

public enum ImageSources {
    static let movieExtensions: Set<String> = ["mov", "mp4", "m4v"]

    /// The files to ingest from pasted or dropped file URLs, in order:
    /// a Live Photo package (`.pvt`, `.livephoto`) gives its first image by name, and a movie whose
    /// base name matches an image in the same set (an exported Live Photo pair) is skipped.
    public static func files(from urls: [URL], fileManager fm: FileManager = .default) -> [URL] {
        let imageBases = Set(urls.filter { ImageNaming.isImage($0) }.map { $0.deletingPathExtension().lastPathComponent.lowercased() })
        var out: [URL] = []
        for url in urls {
            let ext = url.pathExtension.lowercased()
            if ext == "pvt" || ext == "livephoto" {
                let items = (try? fm.contentsOfDirectory(at: url, includingPropertiesForKeys: nil)) ?? []
                if let still = items.filter({ ImageNaming.isImage($0) }).sorted(by: { $0.lastPathComponent < $1.lastPathComponent }).first {
                    out.append(still)
                }
                continue
            }
            if movieExtensions.contains(ext), imageBases.contains(url.deletingPathExtension().lastPathComponent.lowercased()) { continue }
            out.append(url)
        }
        return out
    }
}

// MARK: Preview paths

public enum PreviewImagePath {
    /// The path from the notes folder for an image link written in a note whose folder is `noteFolder`
    /// (relative to the notes folder, `""` for the notes folder itself). Nil for anything with a scheme,
    /// an absolute path, a query or fragment, or a path that climbs out of the notes folder.
    public static func relativeToNotes(_ source: String, noteFolder: String) -> String? {
        var s = source.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("<") && s.hasSuffix(">") { s = String(s.dropFirst().dropLast()) }
        guard !s.isEmpty, !s.hasPrefix("/"), !s.hasPrefix("\\"), !s.contains("?"), !s.contains("#"),
              s.range(of: "^[A-Za-z][A-Za-z0-9+.-]*:", options: .regularExpression) == nil,
              let decoded = s.removingPercentEncoding, !decoded.contains("\0") else { return nil }
        var parts = noteFolder.split(separator: "/").map(String.init)
        for p in decoded.split(separator: "/", omittingEmptySubsequences: true) {
            if p == "." { continue }
            if p == ".." {
                guard !parts.isEmpty else { return nil }
                parts.removeLast()
            } else {
                parts.append(String(p))
            }
        }
        return parts.isEmpty ? nil : parts.joined(separator: "/")
    }

    /// The file for a path from the notes folder, with symbolic links resolved, or nil if it isn't inside the notes folder.
    public static func resolve(_ relative: String, notesFolder: URL) -> URL? {
        guard !relative.isEmpty, !relative.hasPrefix("/") else { return nil }
        let root = notesFolder.standardizedFileURL.resolvingSymlinksInPath()
        let file = root.appendingPathComponent(relative).standardizedFileURL.resolvingSymlinksInPath()
        let rp = root.path.hasSuffix("/") ? root.path : root.path + "/"
        return file.path.hasPrefix(rp) ? file : nil
    }

    /// Percent-encodes each path component for use in a preview URL.
    public static func encoded(_ relative: String) -> String {
        var allowed = CharacterSet.urlPathAllowed
        allowed.remove(charactersIn: "/?#%;")
        return relative.split(separator: "/").map { $0.addingPercentEncoding(withAllowedCharacters: allowed) ?? String($0) }.joined(separator: "/")
    }
}
