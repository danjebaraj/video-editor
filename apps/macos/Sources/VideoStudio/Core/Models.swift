import Foundation

// The data the app exchanges with the Rust core. Property names match the JSON keys of the
// `.cutproj` format and the core API one-to-one (snake_case), so nothing needs key mapping and
// the two sides can't drift apart silently.

// MARK: JSON

/// Free-form JSON (voice settings, unknown project keys, raw reports).
enum JSONValue: Codable, Equatable, Hashable {
    case string(String), number(Double), bool(Bool), null, array([JSONValue]), object([String: JSONValue])

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let b = try? c.decode(Bool.self) { self = .bool(b) }
        else if let n = try? c.decode(Double.self) { self = .number(n) }
        else if let s = try? c.decode(String.self) { self = .string(s) }
        else if let a = try? c.decode([JSONValue].self) { self = .array(a) }
        else { self = .object(try c.decode([String: JSONValue].self)) }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .string(let s): try c.encode(s)
        case .number(let n): try c.encode(n)
        case .bool(let b): try c.encode(b)
        case .null: try c.encodeNil()
        case .array(let a): try c.encode(a)
        case .object(let o): try c.encode(o)
        }
    }

    var stringValue: String? { if case .string(let s) = self { return s } else { return nil } }
    var numberValue: Double? { if case .number(let n) = self { return n } else { return nil } }
    var boolValue: Bool? { if case .bool(let b) = self { return b } else { return nil } }
    var arrayValue: [JSONValue]? { if case .array(let a) = self { return a } else { return nil } }
    var objectValue: [String: JSONValue]? { if case .object(let o) = self { return o } else { return nil } }
    subscript(key: String) -> JSONValue? { objectValue?[key] }
}

struct AnyKey: CodingKey {
    var stringValue: String
    var intValue: Int? { nil }
    init(_ s: String) { stringValue = s }
    init?(stringValue: String) { self.stringValue = stringValue }
    init?(intValue: Int) { nil }
}

// MARK: Project

struct Project: Codable, Equatable {
    var format = "video-studio-project"
    var version = 1
    var id = ""
    var name = ""
    var created = ""
    var modified = ""
    var intent = Intent()
    var spec = ""
    var media: [MediaItem] = []
    var media_roots: [String] = []
    var brand: BrandRef?
    var models = ProjectModels()
    var output_configs: [OutputConfig] = []
    var run_configs: [RunConfig] = []
    var runs: [RunRecord] = []
    var assistant_sessions: [AssistantSessionRef] = []
    /// Extra folders (absolute paths) the spec assistant may read.
    var assistant_folders: [String] = []
    /// Whether production runs may drive and record apps, and which ones.
    var app_capture = AppCapture()
    /// Top-level keys this version doesn't know, kept so a newer app's additions survive.
    var extra: [String: JSONValue] = [:]

    enum CodingKeys: String, CodingKey, CaseIterable {
        case format, version, id, name, created, modified, intent, spec, media, media_roots, brand, models, output_configs, run_configs, runs, assistant_sessions, assistant_folders, app_capture
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        format = try c.decodeIfPresent(String.self, forKey: .format) ?? "video-studio-project"
        version = try c.decodeIfPresent(Int.self, forKey: .version) ?? 1
        id = try c.decodeIfPresent(String.self, forKey: .id) ?? ""
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        created = try c.decodeIfPresent(String.self, forKey: .created) ?? ""
        modified = try c.decodeIfPresent(String.self, forKey: .modified) ?? ""
        intent = try c.decodeIfPresent(Intent.self, forKey: .intent) ?? Intent()
        spec = try c.decodeIfPresent(String.self, forKey: .spec) ?? ""
        media = try c.decodeIfPresent([MediaItem].self, forKey: .media) ?? []
        media_roots = try c.decodeIfPresent([String].self, forKey: .media_roots) ?? []
        brand = try c.decodeIfPresent(BrandRef.self, forKey: .brand)
        models = try c.decodeIfPresent(ProjectModels.self, forKey: .models) ?? ProjectModels()
        output_configs = try c.decodeIfPresent([OutputConfig].self, forKey: .output_configs) ?? []
        run_configs = try c.decodeIfPresent([RunConfig].self, forKey: .run_configs) ?? []
        runs = try c.decodeIfPresent([RunRecord].self, forKey: .runs) ?? []
        assistant_sessions = try c.decodeIfPresent([AssistantSessionRef].self, forKey: .assistant_sessions) ?? []
        assistant_folders = try c.decodeIfPresent([String].self, forKey: .assistant_folders) ?? []
        app_capture = try c.decodeIfPresent(AppCapture.self, forKey: .app_capture) ?? AppCapture()
        let known = Set(CodingKeys.allCases.map(\.rawValue))
        let all = try decoder.container(keyedBy: AnyKey.self)
        for key in all.allKeys where !known.contains(key.stringValue) {
            extra[key.stringValue] = try all.decode(JSONValue.self, forKey: key)
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(format, forKey: .format)
        try c.encode(version, forKey: .version)
        try c.encode(id, forKey: .id)
        try c.encode(name, forKey: .name)
        try c.encode(created, forKey: .created)
        try c.encode(modified, forKey: .modified)
        try c.encode(intent, forKey: .intent)
        try c.encode(spec, forKey: .spec)
        try c.encode(media, forKey: .media)
        try c.encode(media_roots, forKey: .media_roots)
        try c.encodeIfPresent(brand, forKey: .brand)
        try c.encode(models, forKey: .models)
        try c.encode(output_configs, forKey: .output_configs)
        try c.encode(run_configs, forKey: .run_configs)
        try c.encode(runs, forKey: .runs)
        try c.encode(assistant_sessions, forKey: .assistant_sessions)
        if !assistant_folders.isEmpty { try c.encode(assistant_folders, forKey: .assistant_folders) }
        if app_capture != AppCapture() { try c.encode(app_capture, forKey: .app_capture) }
        var all = encoder.container(keyedBy: AnyKey.self)
        for (k, v) in extra { try all.encode(v, forKey: AnyKey(k)) }
    }

    func runConfig(_ id: String?) -> RunConfig? { run_configs.first { $0.id == id } }
    func mediaItem(_ id: String) -> MediaItem? { media.first { $0.id == id } }
}

struct IntentKind: Hashable, Identifiable {
    var key: String
    var label: String
    var id: String { key }

    static let all: [IntentKind] = [
        .init(key: "marketing", label: "Marketing"), .init(key: "product_demo", label: "Product demo"), .init(key: "tutorial", label: "Tutorial or how-to"),
        .init(key: "explainer", label: "Explainer"), .init(key: "social_clip", label: "Social clip"), .init(key: "testimonial", label: "Testimonial"),
        .init(key: "announcement", label: "Announcement"), .init(key: "event_recap", label: "Event recap"), .init(key: "internal", label: "Internal communication"),
        .init(key: "app_store_preview", label: "App store preview"), .init(key: "other", label: "Other"),
    ]
    static func label(_ key: String) -> String { all.first { $0.key == key }?.label ?? "Not set" }
}

struct Intent: Codable, Equatable {
    var kind = "unset"
    var other: String?
    var audience: String?
    var tone: String?
    var target_length_s: Double?
    var cta: String?
    var locales: [String] = []
}

/// App recording: Claude Code drives a desktop or web app during a production run and records it.
struct AppCapture: Codable, Equatable {
    var enabled = true
    /// When not empty, the only apps a run may drive.
    var apps: [String] = []

    init() {}
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        apps = try c.decodeIfPresent([String].self, forKey: .apps) ?? []
    }
}

/// What macOS lets the app do for app recording.
struct CapturePermissions: Codable, Equatable {
    var screen_recording = false
    var accessibility = false
    var supported = false
}

struct ProjectModels: Codable, Equatable {
    var assistant = "sonnet"
    var dry_run = "haiku"
    var effort: String?
    var web_access = false

    init() {}
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        assistant = try c.decodeIfPresent(String.self, forKey: .assistant) ?? "sonnet"
        dry_run = try c.decodeIfPresent(String.self, forKey: .dry_run) ?? "haiku"
        effort = try c.decodeIfPresent(String.self, forKey: .effort)
        web_access = try c.decodeIfPresent(Bool.self, forKey: .web_access) ?? false
    }
}

struct MediaPaths: Codable, Equatable {
    var absolute: String?
    var relative: String?
    var bookmark_macos: String?
}

struct Fingerprint: Codable, Equatable {
    var size: UInt64
    var mtime: String
    var quick_hash: String
}

struct Probe: Codable, Equatable {
    var duration_s: Double?
    var width: Int?
    var height: Int?
    var fps: String?
    var video_codec: String?
    var audio_codec: String?
    var audio_channels: Int?
    var loudness_lufs: Double?
    var rotation: Int?
    var vfr: Bool?
    var for_hash: String?
    var error: String?
}

struct MediaItem: Codable, Equatable, Identifiable {
    var id: String
    var kind: String
    var role: String?
    var paths = MediaPaths()
    var fingerprint: Fingerprint?
    var probe: Probe?
    var notes: String?
    var tags: [String]?
    var pasted: Bool?

    var fileName: String {
        let p = paths.absolute ?? paths.relative ?? id
        return (p as NSString).lastPathComponent
    }

    static let kinds: [(key: String, label: String, symbol: String)] = [
        ("video", "Video", "film"), ("audio", "Audio", "waveform"), ("image", "Images", "photo"), ("font", "Fonts", "textformat"),
        ("transcript", "Transcripts", "captions.bubble"), ("tape", "Terminal tapes", "terminal"), ("demo_script", "Demo scripts", "cursorarrow.click.2"), ("other", "Other", "doc"),
    ]
    static let roles: [(key: String, label: String)] = [
        ("footage", "Footage"), ("b_roll", "B-roll"), ("screen_recording", "Screen recording"), ("logo", "Logo"), ("music", "Music"), ("sfx", "Sound effect"), ("voice", "Voice"), ("reference", "Reference"),
    ]
    static func symbol(_ kind: String) -> String { kinds.first { $0.key == kind }?.symbol ?? "doc" }
}

struct Resolution: Codable, Equatable, Identifiable {
    var id: String
    /// `ok`, `moved`, `changed` or `missing`
    var status: String
    var path: String?
    var found_by: String?
    var stored_path: String?
}

struct PrefixFix: Codable, Equatable {
    var old_prefix: String
    var new_prefix: String
}

// MARK: Brand

struct AssetRef: Codable, Equatable {
    var path: String
    var fingerprint: Fingerprint?
}

struct Logos: Codable, Equatable {
    var primary: AssetRef?
    var horizontal: AssetRef?
    var icon: AssetRef?
    var light: AssetRef?
    var dark: AssetRef?
}

struct Palette: Codable, Equatable {
    var primary = "#F26B21"
    var secondary = "#1F2A44"
    var accent = "#2D9CDB"
    var background = "#FFFFFF"
    var text = "#111827"
}

struct FontRef: Codable, Equatable {
    var family: String?
    var file: AssetRef?
}

struct Fonts: Codable, Equatable {
    var heading = FontRef()
    var body = FontRef()
}

struct Watermark: Codable, Equatable {
    var asset: AssetRef?
    var position = "bottom_right"
    var opacity = 0.6
}

struct Voice: Codable, Equatable {
    var voice_id = ""
    var name: String?
    var model: String?
    var settings: JSONValue?
}

struct BrandPreset: Codable, Equatable, Identifiable {
    var id: String
    var name: String
    var description = ""
    var logos = Logos()
    var colors = Palette()
    var fonts = Fonts()
    var intro: AssetRef?
    var outro: AssetRef?
    var watermark = Watermark()
    var lower_third_style = ""
    var caption_style = ""
    var voice = Voice()
    var music_mood = ""
    var rules = ""
    var modified = ""
}

struct BrandRef: Codable, Equatable {
    var preset_id: String
    var snapshot: BrandPreset
}

// MARK: Outputs and runs

struct OutputOverrides: Codable, Equatable {
    var target: String?
    var width: Int?
    var height: Int?
    var fps: Double?
    var max_duration_s: Double?
    var max_bytes: UInt64?
    var audio: Bool?
    var loop: Bool?
    var captions: String?
    var formats: [String]?
}

struct OutputConfig: Codable, Equatable, Identifiable {
    var id: String
    var preset: String
    var name: String?
    var overrides = OutputOverrides()
}

struct Limits: Codable, Equatable {
    var min_duration_s: Double?
    var max_duration_s: Double?
    var max_bytes: UInt64?
    var max_fps: Double?
}

struct NamedSize: Codable, Equatable, Hashable {
    var name: String
    var width: Int
    var height: Int
}

struct OutputPreset: Codable, Equatable, Identifiable {
    var id: String
    var name: String
    var platform: String
    var targets: [String]
    var width: Int?
    var height: Int?
    var fps: Double?
    var sizes: [NamedSize]?
    var captions = "burn"
    var audio = true
    var loop = false
    var max_bytes: UInt64?
    var formats = ["mp4"]
    var limits = Limits()
    var notes = ""
    var user: Bool?

    enum CodingKeys: String, CodingKey { case id, name, platform, targets, width, height, fps, sizes, captions, audio, loop, max_bytes, formats, limits, notes, user }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        platform = try c.decode(String.self, forKey: .platform)
        targets = try c.decode([String].self, forKey: .targets)
        width = try c.decodeIfPresent(Int.self, forKey: .width)
        height = try c.decodeIfPresent(Int.self, forKey: .height)
        fps = try c.decodeIfPresent(Double.self, forKey: .fps)
        sizes = try c.decodeIfPresent([NamedSize].self, forKey: .sizes)
        captions = try c.decodeIfPresent(String.self, forKey: .captions) ?? "burn"
        audio = try c.decodeIfPresent(Bool.self, forKey: .audio) ?? true
        loop = try c.decodeIfPresent(Bool.self, forKey: .loop) ?? false
        max_bytes = try c.decodeIfPresent(UInt64.self, forKey: .max_bytes)
        formats = try c.decodeIfPresent([String].self, forKey: .formats) ?? ["mp4"]
        limits = try c.decodeIfPresent(Limits.self, forKey: .limits) ?? Limits()
        notes = try c.decodeIfPresent(String.self, forKey: .notes) ?? ""
        user = try c.decodeIfPresent(Bool.self, forKey: .user)
    }
}

struct IntentDefaults: Codable, Equatable {
    var template: String
    var outputs: [String]
    var captions: String
    var pacing: String
    var music_level: String
}

struct Platforms: Codable, Equatable {
    var version = 1
    var updated: String?
    var presets: [OutputPreset] = []
    var intents: [String: IntentDefaults]?

    func preset(_ id: String) -> OutputPreset? { presets.first { $0.id == id } }
}

struct ExportSpec: Codable, Equatable {
    var name: String
    var format: String
    var out: String
}

struct ResolvedOutput: Codable, Equatable, Identifiable {
    var id: String
    var name: String?
    var slug: String?
    var preset: String?
    var platform: String?
    var target: String?
    var width: Int?
    var height: Int?
    var fps: Double?
    var audio: Bool?
    var loop: Bool?
    var captions: String?
    var max_bytes: UInt64?
    var limits: Limits?
    var exports: [ExportSpec]?
    var notes: String?
    /// Set instead of everything else when the config can't be resolved.
    var error: String?
}

struct RunConfig: Codable, Equatable, Identifiable {
    var id: String
    var name: String
    var outputs: [String] = []
    var production_model = "opus"
    var quality = "final"
    var locales: [String] = []
    var output_folder = ""
    var eleven_credit_cap: UInt64 = 20_000
    var dry_run_first = true
    var max_spend_usd: Double?
    var max_turns: Int?
    var web_access = false
    var question_timeout_s: Int?
    var effort: String?

    enum CodingKeys: String, CodingKey { case id, name, outputs, production_model, quality, locales, output_folder, eleven_credit_cap, dry_run_first, max_spend_usd, max_turns, web_access, question_timeout_s, effort }
    init(id: String, name: String) { self.id = id; self.name = name }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        outputs = try c.decodeIfPresent([String].self, forKey: .outputs) ?? []
        production_model = try c.decodeIfPresent(String.self, forKey: .production_model) ?? "opus"
        quality = try c.decodeIfPresent(String.self, forKey: .quality) ?? "final"
        locales = try c.decodeIfPresent([String].self, forKey: .locales) ?? []
        output_folder = try c.decodeIfPresent(String.self, forKey: .output_folder) ?? ""
        eleven_credit_cap = try c.decodeIfPresent(UInt64.self, forKey: .eleven_credit_cap) ?? 20_000
        dry_run_first = try c.decodeIfPresent(Bool.self, forKey: .dry_run_first) ?? true
        max_spend_usd = try c.decodeIfPresent(Double.self, forKey: .max_spend_usd)
        max_turns = try c.decodeIfPresent(Int.self, forKey: .max_turns)
        web_access = try c.decodeIfPresent(Bool.self, forKey: .web_access) ?? false
        question_timeout_s = try c.decodeIfPresent(Int.self, forKey: .question_timeout_s)
        effort = try c.decodeIfPresent(String.self, forKey: .effort)
    }
}

struct RunRecord: Codable, Equatable, Identifiable {
    var id: String
    var run_config: String
    var run_config_name: String?
    /// `dry_run` or `production`
    var kind: String
    var folder: String
    /// `running`, `succeeded`, `succeeded_with_warnings`, `failed` or `cancelled`
    var status: String
    var started: String
    var finished: String?
    var claude_session: String?
    var model: String?
    var previous_run: String?
    var cost_usd: Double?
    var eleven_credits: UInt64?
}

struct AssistantSessionRef: Codable, Equatable, Identifiable {
    var id: String
    var name: String
    var claude_session: String?
}

enum RunStatus {
    static func label(_ s: String) -> String {
        switch s {
        case "running": return "Running"
        case "succeeded": return "Succeeded"
        case "succeeded_with_warnings": return "Succeeded with warnings"
        case "failed": return "Failed"
        case "cancelled": return "Cancelled"
        default: return s.capitalized
        }
    }
    static func symbol(_ s: String) -> String {
        switch s {
        case "running": return "circle.dotted"
        case "succeeded": return "checkmark.circle.fill"
        case "succeeded_with_warnings": return "exclamationmark.triangle.fill"
        case "failed": return "xmark.octagon.fill"
        case "cancelled": return "stop.circle.fill"
        default: return "circle"
        }
    }
}

// MARK: Settings, models, environment

struct CoreSettings: Codable, Equatable {
    var default_output_folder = ""
    var parallel_run_limit = 1
    var media_roots: [String] = []
    var notifications = true
    var recent_output_folders: [String] = []
    var claude_path = ""
    var default_assistant_model = "sonnet"
    var default_dry_run_model = "haiku"
    var default_production_model = "opus"
    var effort = ""
    var web_access_default = false
    var headless_question_timeout_s = 0
    var cutline_path = ""
    var cutline_repo = ""
    var default_quality = "final"
    var eleven_default_voice = ""
    var eleven_default_credit_cap: UInt64 = 20_000
    var eleven_cache_limit_bytes: UInt64 = 2_000_000_000
    var words_per_minute = 150
    var cleanup_older_than_days = 30
    var onboarding_done = false
}

struct ModelPrice: Codable, Equatable {
    var input: Double
    var output: Double
}

struct ModelVersion: Codable, Equatable, Identifiable {
    var id: String
    var name: String
}

struct ModelFamily: Codable, Equatable, Identifiable {
    var id: String
    var name: String
    var alias: String
    var tier: String?
    var price: ModelPrice?
    var versions: [ModelVersion]
}

struct ModelDefaults: Codable, Equatable {
    var assistant: String
    var dry_run: String
    var production: String
}

struct ModelList: Codable, Equatable {
    var version = 1
    var updated: String?
    var families: [ModelFamily] = []
    var defaults = ModelDefaults(assistant: "sonnet", dry_run: "haiku", production: "opus")

    func family(of model: String) -> ModelFamily? {
        let m = model.lowercased()
        return families.first { $0.alias == m || $0.versions.contains { $0.id == m } } ?? families.first { m.contains($0.id) }
    }

    func displayName(_ model: String) -> String {
        for f in families {
            if f.alias == model { return "\(f.name) (latest)" }
            if let v = f.versions.first(where: { $0.id == model }) { return v.name }
        }
        return model.isEmpty ? "Default" : model
    }
}

struct ClaudeInfo: Codable, Equatable {
    var path: String
    var version: String
    var version_text: String
    var effort_levels: [String]
    var login_path: String?
    var supported: Bool
}

struct CutlineInfo: Codable, Equatable {
    var path: String
    var version: String
}

struct DoctorSummary: Codable, Equatable {
    var ok: Bool
    var missing_required: [String]
    var missing_optional: [String]
    var raw: JSONValue?
}

struct CoreEnvironment: Codable, Equatable {
    var claude: ClaudeInfo?
    var signed_in: Bool?
    var cutline: CutlineInfo?
    var doctor: DoctorSummary?
    var min_claude_version = ""
}

// MARK: Spec

struct OutlineItem: Codable, Equatable, Identifiable {
    var level: Int
    var title: String
    var line: Int
    var kind: String
    var id: String { "\(line):\(title)" }
}

struct LintIssue: Codable, Equatable, Identifiable {
    var rule: String
    var severity: String
    var message: String
    var line: Int
    var id: String { "\(rule):\(line):\(message)" }
}

struct SpecTemplate: Codable, Equatable, Identifiable {
    var id: String
    var title: String
}

struct SlashBlock: Codable, Equatable {
    var command: String
    var title: String
    var snippet: String
}

struct Hunk: Codable, Equatable {
    var old_start: Int
    var old_lines: [String]
    var new_lines: [String]
}

// MARK: Validation

struct FixLink: Codable, Equatable {
    var kind: String
    var target: String?
    var line: Int?
}

struct ValidationIssue: Codable, Equatable, Identifiable {
    var id: String
    var rule: String
    /// `blocker`, `warning` or `info`
    var severity: String
    var category: String
    var title: String
    var detail: String?
    var fix: FixLink?
}

struct Estimates: Codable, Equatable {
    var duration_s: Double?
    var voice_over_words = 0
    var eleven_characters: UInt64 = 0
    var eleven_credits: UInt64 = 0
    var eleven_cap: UInt64 = 0
    var eleven_remaining: UInt64?
    var claude_usd_low = 0.0
    var claude_usd_high = 0.0
    var disk_bytes: UInt64 = 0
    var free_bytes: UInt64?
}

struct ReadinessFinding: Codable, Equatable {
    var text: String
    var spec_line: Int?
    var detail: String?
}

struct ReadinessReport: Codable, Equatable {
    var ready = false
    var ambiguities: [ReadinessFinding] = []
    var assumptions: [ReadinessFinding] = []
    var missing_media: [ReadinessFinding] = []
    var risks: [ReadinessFinding] = []
    var estimated_durations: [String: Double] = [:]
    var notes: String?
}

struct ValidationReport: Codable, Equatable {
    var generated = ""
    var project_id = ""
    var run_config: String?
    var issues: [ValidationIssue] = []
    var estimates = Estimates()
    var blockers = 0
    var warnings = 0
    var infos = 0
    var dry_run: ReadinessReport?
}

// MARK: Run events and results

struct ClaudeQuestionOption: Codable, Equatable, Hashable {
    var label: String
    var description: String?
}

struct ClaudeQuestion: Codable, Equatable {
    var question: String
    var header: String?
    var options: [ClaudeQuestionOption] = []
    var multi_select: Bool?
}

/// One line of the run event stream (`events.ndjson`); `event` says which fields are set.
struct RunEvent: Codable, Equatable {
    var ts: String
    var event: String
    var run_id: String?
    var kind: String?
    var run_config: String?
    var folder: String?
    var model: String?
    var outputs: [String]?
    var stage: String?
    var label: String?
    var fraction: Double?
    var detail: String?
    var role: String?
    var text: String?
    var tool: String?
    var id: String?
    var questions: [ClaudeQuestion]?
    var timeout_s: Int?
    var by: String?
    var answers: [String: String]?
    var path: String?
    var output: String?
    var bytes: UInt64?
    var message: String?
    var claude_usd: Double?
    var claude_estimated: Bool?
    var input_tokens: UInt64?
    var output_tokens: UInt64?
    var eleven_credits: UInt64?
    var eleven_cap: UInt64?
    var ok: Bool?
    var seconds: Double?
    var status: String?
    var exit_code: Int?
    var cost_usd: Double?
}

struct QaFinding: Codable, Equatable, Hashable {
    var check: String
    var severity: String
    var message: String
}

struct OutputFile: Codable, Equatable, Identifiable {
    var output_id: String
    var output_name: String
    var platform: String
    var export: String
    var path: String
    var exists: Bool
    var bytes: UInt64
    var width: Int
    var height: Int
    var incomplete: Bool
    var qa_verdict: String
    var qa_errors: UInt64
    var qa_warnings: UInt64
    var qa_findings: [QaFinding]?
    var qa_report: String?
    var captions_srt: String?
    var captions_vtt: String?
    var poster: String?
    var render_report: String?
    var duration_s: Double?
    var id: String { path }
}

struct LogFile: Codable, Equatable, Identifiable {
    var name: String
    var path: String
    var bytes: UInt64
    var source: String
    var id: String { path }
}

struct RunResults: Codable, Equatable {
    var folder: String
    var exists: Bool
    var run: JSONValue?
    var outputs: [OutputFile] = []
    var summary: String?
    var readiness: ReadinessReport?
    var project_json5: String?
    var logs: [LogFile] = []
}

struct RunOutcome: Codable, Equatable {
    var run_id: String
    var folder: String
    var status: String
    var exit_code: Int
    var record: RunRecord
    var outputs: [OutputFile]
    var readiness: ReadinessReport?
    var warnings: [String]
    var message: String?
}

struct RunStorage: Codable, Equatable, Identifiable {
    var folder: String
    var exists: Bool
    var total_bytes: UInt64
    var logs_bytes: UInt64
    var started: String?
    var kind: String?
    var id: String { folder }
}

struct StorageInfo: Codable, Equatable {
    var eleven_cache_bytes: UInt64 = 0
    var dry_runs_bytes: UInt64 = 0
    var runs: [RunStorage] = []
}

// MARK: Assistant

/// One assistant event; `event` says which fields are set.
struct AssistantEvent: Codable, Equatable {
    var event: String
    var session_id: String?
    var model: String?
    var text: String?
    var id: String?
    var name: String?
    var summary: String?
    var detail: String?
    var is_error: Bool?
    var reason: String?
    var questions: [ClaudeQuestion]?
    var spec: String?
    var input_tokens: UInt64?
    var output_tokens: UInt64?
    var cost_usd: Double?
    var duration_ms: UInt64?
    var message: String?
    var ok: Bool?
    var stopped: Bool?
}

// MARK: Formatting

enum Fmt {
    static func bytes(_ n: UInt64) -> String {
        let f = ByteCountFormatter()
        f.countStyle = .file
        return f.string(fromByteCount: Int64(min(n, UInt64(Int64.max))))
    }

    static func duration(_ seconds: Double) -> String {
        if seconds < 60 { return String(format: "%.1f s", seconds) }
        let t = Int(seconds.rounded())
        return t >= 3600 ? String(format: "%d:%02d:%02d", t / 3600, (t % 3600) / 60, t % 60) : String(format: "%d:%02d", t / 60, t % 60)
    }

    static func elapsed(_ seconds: Double) -> String {
        let t = Int(seconds)
        return t >= 3600 ? String(format: "%d:%02d:%02d", t / 3600, (t % 3600) / 60, t % 60) : String(format: "%d:%02d", t / 60, t % 60)
    }

    static func money(_ usd: Double) -> String { usd < 0.01 && usd > 0 ? "under $0.01" : String(format: "$%.2f", usd) }

    private static let iso: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()

    static func date(_ rfc3339: String?) -> Date? { rfc3339.flatMap { iso.date(from: $0) } }

    static func when(_ rfc3339: String?) -> String {
        guard let d = date(rfc3339) else { return "" }
        let f = DateFormatter()
        f.doesRelativeDateFormatting = true
        f.dateStyle = .medium
        f.timeStyle = .short
        return f.string(from: d)
    }

    static func aspect(width: Int, height: Int) -> String {
        guard height > 0 else { return "" }
        let a = Double(width) / Double(height)
        for (label, v) in [("16:9", 16.0 / 9), ("9:16", 9.0 / 16), ("1:1", 1.0), ("4:5", 0.8), ("4:3", 4.0 / 3), ("3:4", 0.75)] where abs(a - v) < 0.02 { return label }
        return "\(width)×\(height)"
    }
}
