import Foundation

/// Image files stored once by content hash. In Video Studio the core stores pasted images in the
/// project's sidecar; this remains for the editor code shared with Just Notes.
public enum NoteAssets {
    public static let linkPrefix = "assets/"

    @discardableResult
    public static func store(data: Data, ext: String, in folder: URL, fileManager fm: FileManager = .default) throws -> String {
        try fm.createDirectory(at: folder, withIntermediateDirectories: true)
        let name = "\(ImageIngest.sha256(data)).\(ext.lowercased())"
        let url = folder.appendingPathComponent(name)
        if !fm.fileExists(atPath: url.path) { try data.write(to: url, options: .atomic) }
        return name
    }

    public static let imageExtensions: Set<String> = ["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "tif", "tiff", "bmp", "svg", "avif", "ico"]
}
