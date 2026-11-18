import Foundation

/// The Rust core (`video-studio-core`), reached through the UniFFI bindings.
///
/// Every operation takes and returns JSON; this wrapper encodes arguments, decodes results and
/// turns core errors into `CoreFailure`. Calls block, so anything that touches the disk or runs
/// a tool goes through `Core.async`.
enum Core {
    struct CoreFailure: LocalizedError {
        var code: String
        var message: String
        var errorDescription: String? { message }
    }

    private static let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.outputFormatting = [.withoutEscapingSlashes]
        return e
    }()
    private static let decoder = JSONDecoder()

    /// Arguments for a call: plain JSON values, or anything Encodable wrapped with `Core.arg`.
    typealias Args = [String: Any?]

    /// Wraps an Encodable value so it can go in `Args`.
    static func arg<T: Encodable>(_ value: T) -> Any {
        guard let data = try? encoder.encode(value), let obj = try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed]) else { return NSNull() }
        return obj
    }

    static func encode<T: Encodable>(_ value: T) -> String {
        (try? encoder.encode(value)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
    }

    static func decode<T: Decodable>(_ type: T.Type, from json: String) -> T? {
        try? decoder.decode(T.self, from: Data(json.utf8))
    }

    private static func argsJSON(_ args: Args) -> String {
        var clean: [String: Any] = [:]
        for (k, v) in args { clean[k] = v ?? NSNull() }
        let data = (try? JSONSerialization.data(withJSONObject: clean, options: [.withoutEscapingSlashes])) ?? Data("{}".utf8)
        return String(data: data, encoding: .utf8) ?? "{}"
    }

    /// Runs a core operation and returns its raw JSON result.
    static func raw(_ method: String, _ args: Args = [:]) throws -> String {
        do {
            return try call(method: method, args: argsJSON(args))
        } catch let e as CoreError {
            switch e {
            case .Failed(let code, let message): throw CoreFailure(code: code, message: message)
            }
        }
    }

    /// Runs a core operation and decodes its result.
    static func run<T: Decodable>(_ method: String, _ args: Args = [:], as type: T.Type = T.self) throws -> T {
        let json = try raw(method, args)
        do {
            return try decoder.decode(T.self, from: Data(json.utf8))
        } catch {
            throw CoreFailure(code: "decode", message: "Unexpected result from \(method): \(error)")
        }
    }

    /// Runs a core operation off the main thread.
    static func async<T: Decodable & Sendable>(_ method: String, _ args: Args = [:], as type: T.Type = T.self) async throws -> T {
        let json = argsJSON(args)
        return try await Task.detached(priority: .userInitiated) {
            let out: String
            do {
                out = try call(method: method, args: json)
            } catch let e as CoreError {
                switch e {
                case .Failed(let code, let message): throw CoreFailure(code: code, message: message)
                }
            }
            do {
                return try decoder.decode(T.self, from: Data(out.utf8))
            } catch {
                throw CoreFailure(code: "decode", message: "Unexpected result from \(method): \(error)")
            }
        }.value
    }
}

extension Project: @unchecked Sendable {}

// Results of core operations that return more than one thing.

struct MediaAddResult: Decodable, Sendable {
    var project: Project
    var ids: [String]
    var errors: [String]
}

struct MediaResolveResult: Decodable, Sendable {
    var project: Project
    var resolutions: [Resolution]
    var dirty: Bool
}

struct ProbeResult: Decodable, Sendable {
    var project: Project
    var changed: Bool
}

struct RelinkCandidate: Decodable, Sendable, Equatable {
    var id: String
    var path: String
}

struct RelinkResult: Decodable, Sendable {
    var project: Project
    var prefix_fix: PrefixFix?
    var candidates: [RelinkCandidate]
}

struct RelinkedResult: Decodable, Sendable {
    var project: Project
    var relinked: [String]
}

struct ValidateResult: Decodable, Sendable {
    var report: ValidationReport
    var project: Project
    var resolutions: [Resolution]
}

struct ExternalChangeResult: Decodable, Sendable {
    /// `none`, `runs_only` or `other`
    var change: String
    var disk: Project
}

struct MergeRunsResult: Decodable, Sendable {
    var project: Project
    var changed: Bool
}

struct SummaryResult: Decodable, Sendable {
    var summary: String
    var read_paths: [String]
}

/// The bundled `video-studio` binary: the helper Claude uses during runs, and the one process
/// that reads or writes the ElevenLabs key (spec §14, §19). The app itself never sees the key.
enum Helper {
    static var url: URL {
        if let override = ProcessInfo.processInfo.environment["VIDEO_STUDIO_HELPER"], !override.isEmpty { return URL(fileURLWithPath: override) }
        return Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/video-studio")
    }
    static var path: String { url.path }
    static var exists: Bool { FileManager.default.isExecutableFile(atPath: path) }

    /// A bundled test double (`vs-fake-claude`, `vs-fake-cutline`), present in Debug builds.
    static func fake(_ name: String) -> String? {
        let p = url.deletingLastPathComponent().appendingPathComponent(name).path
        return FileManager.default.isExecutableFile(atPath: p) ? p : nil
    }

    struct Output {
        var status: Int32
        var stdout: String
        var stderr: String
    }

    /// Runs the helper to completion. Blocks: call it off the main thread.
    static func run(_ args: [String], stdin: String? = nil, cwd: URL? = nil, environment: [String: String] = [:]) -> Output {
        let p = Process()
        p.executableURL = url
        p.arguments = args
        if let cwd { p.currentDirectoryURL = cwd }
        var env = ProcessInfo.processInfo.environment
        for (k, v) in environment { env[k] = v }
        p.environment = env
        let out = Pipe(), err = Pipe(), input = Pipe()
        p.standardOutput = out
        p.standardError = err
        p.standardInput = stdin == nil ? FileHandle.nullDevice : input
        do { try p.run() } catch { return Output(status: -1, stdout: "", stderr: error.localizedDescription) }
        if let stdin {
            input.fileHandleForWriting.write(Data(stdin.utf8))
            try? input.fileHandleForWriting.close()
        }
        let o = out.fileHandleForReading.readDataToEndOfFile()
        let e = err.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return Output(status: p.terminationStatus, stdout: String(data: o, encoding: .utf8) ?? "", stderr: String(data: e, encoding: .utf8) ?? "")
    }

    struct KeyStatus: Equatable {
        var present = false
        var masked: String?
        /// Credits left on the account, known after Test.
        var remaining: UInt64?
        var limit: UInt64?
        var tier: String?
        /// A key limited to some permissions works but can't read credits: the permission it lacks.
        var creditsUnavailable: String?
        /// The key was accepted by the last Test (Cartesia reports no credits).
        var works = false
        var error: String?
    }

    private static func json(_ s: String) -> [String: Any] {
        (try? JSONSerialization.jsonObject(with: Data(s.utf8)) as? [String: Any]) ?? [:]
    }

    /// Whether an ElevenLabs key is stored, shown masked.
    static func keyStatus(provider: String = "elevenlabs") -> KeyStatus {
        let o = run(["secret", "status", "--provider", provider, "--json"])
        let j = json(o.stdout)
        return KeyStatus(present: j["present"] as? Bool ?? false, masked: j["masked"] as? String)
    }

    /// Checks the stored key against ElevenLabs and reads the remaining credits.
    static func testKey(provider: String = "elevenlabs") -> KeyStatus {
        var s = keyStatus(provider: provider)
        guard s.present else { s.error = "No key is stored."; return s }
        let o = run(["secret", "test", "--provider", provider, "--json"])
        let j = json(o.stdout)
        if j["ok"] as? Bool == true {
            s.works = true
            s.remaining = (j["remaining"] as? NSNumber)?.uint64Value
            s.limit = (j["limit"] as? NSNumber)?.uint64Value
            s.tier = j["tier"] as? String
            s.creditsUnavailable = j["credits_unavailable"] as? String
        } else {
            s.error = (j["error"] as? String) ?? o.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return s
    }

    static func setKey(_ key: String, provider: String = "elevenlabs") -> Bool {
        run(["secret", "set", "--provider", provider], stdin: key + "\n").status == 0
    }

    static func deleteKey(provider: String = "elevenlabs") -> Bool {
        run(["secret", "delete", "--provider", provider]).status == 0
    }
}

/// Security-scoped bookmarks: how a media file is found again after it is renamed or moved on
/// the same volume (spec §6.2 step 1). The core calls back into this for its `FileRefs` trait.
final class BookmarkRefs: FileRefsProvider, @unchecked Sendable {
    func create(path: String) -> String? {
        let url = URL(fileURLWithPath: path)
        guard let data = try? url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil) else { return nil }
        return data.base64EncodedString()
    }

    func resolve(reference: String) -> String? {
        guard let data = Data(base64Encoded: reference) else { return nil }
        var stale = false
        guard let url = try? URL(resolvingBookmarkData: data, options: [.withoutUI, .withoutMounting], relativeTo: nil, bookmarkDataIsStale: &stale) else { return nil }
        return FileManager.default.fileExists(atPath: url.path) ? url.path : nil
    }
}

/// Forwards core events to a closure on the main queue.
final class EventForwarder: EventListener, @unchecked Sendable {
    private let handler: (String) -> Void
    init(_ handler: @escaping (String) -> Void) { self.handler = handler }
    func onEvent(json: String) {
        DispatchQueue.main.async { [handler] in handler(json) }
    }
}
