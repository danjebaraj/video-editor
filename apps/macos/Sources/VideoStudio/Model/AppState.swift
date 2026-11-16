import AppKit
import Combine

/// App-wide state: settings, the detected tools, the library (brands, output presets, models)
/// and the ElevenLabs key status.
@MainActor
final class AppState: ObservableObject {
    static let shared = AppState()

    @Published var settings = CoreSettings()
    @Published private(set) var environment: CoreEnvironment?
    @Published private(set) var isDetecting = false
    @Published private(set) var models = ModelList()
    @Published private(set) var platforms = Platforms()
    @Published private(set) var brands: [BrandPreset] = []
    @Published private(set) var keyStatus = Helper.KeyStatus()
    /// The Cartesia key, which speaks the voice-over when Settings says so.
    @Published private(set) var cartesiaKeyStatus = Helper.KeyStatus()
    /// IDs of projects with a production or dry run in progress.
    @Published var activeRuns: Set<String> = []
    /// Production run folders seen in opened projects, for Settings › Storage.
    @Published private(set) var knownRunFolders: [String] = []

    private var lastDetect = Date.distantPast

    private init() {
        setFileRefsProvider(provider: BookmarkRefs())
        reloadLibrary()
        knownRunFolders = UserDefaults.standard.stringArray(forKey: "knownRunFolders") ?? []
    }

    var claudeAvailable: Bool { environment?.claude?.supported == true }
    var claudeMissing: Bool { environment != nil && environment?.claude == nil }
    var cutlineAvailable: Bool { environment?.cutline != nil }
    var effortLevels: [String] { environment?.claude?.effort_levels ?? [] }
    var libraryURL: URL { URL(fileURLWithPath: (try? Core.run("library.dir", as: String.self)) ?? NSTemporaryDirectory(), isDirectory: true) }

    func reloadLibrary() {
        settings = (try? Core.run("settings.load")) ?? CoreSettings()
        models = (try? Core.run("models.list")) ?? ModelList()
        platforms = (try? Core.run("platforms.list")) ?? Platforms()
        brands = (try? Core.run("brand.list")) ?? []
    }

    func saveSettings() {
        _ = try? Core.raw("settings.save", ["settings": Core.arg(settings)])
    }

    func reloadBrands() { brands = (try? Core.run("brand.list")) ?? [] }
    func reloadPlatforms() { platforms = (try? Core.run("platforms.list")) ?? Platforms() }
    func reloadModels() { models = (try? Core.run("models.list")) ?? ModelList() }

    /// Finds Claude Code and cutline again (launch, app activation, Settings › Re-check).
    func detect(force: Bool = false) {
        guard !isDetecting, force || Date().timeIntervalSince(lastDetect) > 20 else { return }
        isDetecting = true
        lastDetect = Date()
        Task {
            let env: CoreEnvironment? = try? await Core.async("environment.detect")
            let key = await Task.detached { Helper.exists ? Helper.keyStatus() : Helper.KeyStatus() }.value
            self.environment = env ?? CoreEnvironment()
            if self.keyStatus.masked != key.masked || self.keyStatus.present != key.present { self.keyStatus = key }
            let cartesia = await Task.detached { Helper.exists ? Helper.keyStatus(provider: "cartesia") : Helper.KeyStatus() }.value
            if self.cartesiaKeyStatus.masked != cartesia.masked || self.cartesiaKeyStatus.present != cartesia.present { self.cartesiaKeyStatus = cartesia }
            self.isDetecting = false
        }
    }

    func refreshKeyStatus(test: Bool = false) {
        Task {
            let key = await Task.detached { test ? Helper.testKey() : Helper.keyStatus() }.value
            self.keyStatus = key
        }
    }

    func refreshCartesiaKeyStatus(test: Bool = false) {
        Task {
            let key = await Task.detached { test ? Helper.testKey(provider: "cartesia") : Helper.keyStatus(provider: "cartesia") }.value
            self.cartesiaKeyStatus = key
        }
    }

    func rememberRunFolders(_ folders: [String]) {
        var all = knownRunFolders
        for f in folders where !all.contains(f) { all.append(f) }
        if all != knownRunFolders {
            knownRunFolders = Array(all.suffix(500))
            UserDefaults.standard.set(knownRunFolders, forKey: "knownRunFolders")
        }
    }

    /// Whether another project may start a run now (Settings › General › parallel run limit).
    func canStartRun(projectID: String) -> Bool {
        !activeRuns.contains(projectID) && activeRuns.count < max(1, settings.parallel_run_limit)
    }
}

/// User-interface preferences kept in UserDefaults (the core's settings.json holds everything
/// the CLI shares).
@MainActor
final class AppSettings: ObservableObject {
    static let shared = AppSettings()
    private let d = UserDefaults.standard

    // Editor
    @Published var fontName: String { didSet { d.set(fontName, forKey: "fontName") } }
    @Published var fontSize: Double { didSet { d.set(fontSize, forKey: "fontSize") } }
    @Published var lineHeight: Double { didSet { d.set(lineHeight, forKey: "lineHeight") } }
    @Published var softWrap: Bool { didSet { d.set(softWrap, forKey: "softWrap") } }
    @Published var lineNumbers: Bool { didSet { d.set(lineNumbers, forKey: "lineNumbers") } }
    @Published var tabWidth: Int { didSet { d.set(tabWidth, forKey: "tabWidth") } }
    @Published var useSpaces: Bool { didSet { d.set(useSpaces, forKey: "useSpaces") } }
    @Published var autoPair: Bool { didSet { d.set(autoPair, forKey: "autoPair") } }
    @Published var spellCheck: Bool { didSet { d.set(spellCheck, forKey: "spellCheck") } }
    @Published var grammarCheck: Bool { didSet { d.set(grammarCheck, forKey: "grammarCheck") } }
    @Published var autoFormatTables: Bool { didSet { d.set(autoFormatTables, forKey: "autoFormatTables") } }
    @Published var imageHoverPreview: Bool { didSet { d.set(imageHoverPreview, forKey: "imageHoverPreview") } }
    @Published var showTableToolbar: Bool { didSet { d.set(showTableToolbar, forKey: "showTableToolbar") } }
    @Published var highlightIslands: Bool { didSet { d.set(highlightIslands, forKey: "highlightIslands") } }
    @Published var dimIslandMarkers: Bool { didSet { d.set(dimIslandMarkers, forKey: "dimIslandMarkers") } }
    /// Draw images, Mermaid diagrams and math under their source in the editor.
    @Published var inlineRendering: Bool { didSet { d.set(inlineRendering, forKey: "inlineRendering") } }

    // Preview
    @Published var previewTheme: String { didSet { d.set(previewTheme, forKey: "previewTheme") } }
    @Published var customCSSPath: String { didSet { d.set(customCSSPath, forKey: "customCSSPath") } }
    @Published var showFrontMatter: Bool { didSet { d.set(showFrontMatter, forKey: "showFrontMatter") } }
    /// Off by default: loading a remote image tells its server when the spec was viewed.
    @Published var loadRemoteImages: Bool { didSet { d.set(loadRemoteImages, forKey: "loadRemoteImages") } }
    @Published var showIslandOutlines: Bool { didSet { d.set(showIslandOutlines, forKey: "showIslandOutlines") } }

    // Claude panel
    @Published var showClaudeUsage: Bool { didSet { d.set(showClaudeUsage, forKey: "showClaudeUsage") } }

    // Not used by this app; kept so the editor code shared with Just Notes compiles unchanged.
    let hideStopped = false
    let highlightImpliedTags = false
    let convertImagesToPNG = true

    private init() {
        let d = UserDefaults.standard
        func v<T>(_ k: String, _ def: T) -> T { (d.object(forKey: k) as? T) ?? def }
        fontName = v("fontName", "SF Mono")
        fontSize = v("fontSize", 14.0)
        lineHeight = v("lineHeight", 1.35)
        softWrap = v("softWrap", true)
        lineNumbers = v("lineNumbers", false)
        tabWidth = v("tabWidth", 4)
        useSpaces = v("useSpaces", true)
        autoPair = v("autoPair", true)
        spellCheck = v("spellCheck", true)
        grammarCheck = v("grammarCheck", false)
        autoFormatTables = v("autoFormatTables", true)
        imageHoverPreview = v("imageHoverPreview", true)
        showTableToolbar = v("showTableToolbar", true)
        highlightIslands = v("highlightIslands", true)
        dimIslandMarkers = v("dimIslandMarkers", true)
        inlineRendering = v("inlineRendering", false)
        previewTheme = v("previewTheme", "system")
        customCSSPath = v("customCSSPath", "")
        showFrontMatter = v("showFrontMatter", true)
        loadRemoteImages = v("loadRemoteImages", false)
        showIslandOutlines = v("showIslandOutlines", false)
        showClaudeUsage = v("showClaudeUsage", true)
    }

    var indentUnit: String { useSpaces ? String(repeating: " ", count: max(tabWidth, 1)) : "\t" }

    var editorFont: NSFont {
        let size = CGFloat(fontSize)
        if fontName == "SF Mono" || fontName.isEmpty { return .monospacedSystemFont(ofSize: size, weight: .regular) }
        if fontName == "System" { return .systemFont(ofSize: size) }
        return NSFont(name: fontName, size: size) ?? .monospacedSystemFont(ofSize: size, weight: .regular)
    }
}

/// Small modal prompts.
enum Prompt {
    @MainActor
    static func text(title: String, message: String, initial: String, ok: String = "OK") -> String? {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.addButton(withTitle: ok)
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(string: initial)
        field.frame = NSRect(x: 0, y: 0, width: 280, height: 24)
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return nil }
        let v = field.stringValue.trimmingCharacters(in: .whitespaces)
        return v.isEmpty ? nil : v
    }

    @MainActor
    static func error(_ message: String, detail: String = "") {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = message
        alert.informativeText = detail
        alert.runModal()
    }

    @MainActor
    static func confirm(_ title: String, message: String = "", ok: String = "OK", destructive: Bool = false) -> Bool {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.addButton(withTitle: ok).hasDestructiveAction = destructive
        alert.addButton(withTitle: "Cancel")
        return alert.runModal() == .alertFirstButtonReturn
    }
}
