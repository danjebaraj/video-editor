import SwiftUI
import AppKit

/// Inspector: Claude, Project, Outputs, Validation, Run (spec §17).
struct InspectorView: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject var chatFocus: FocusRequest

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 2) {
                ForEach(InspectorTab.allCases) { tab in
                    Button { document.inspectorTab = tab } label: {
                        VStack(spacing: 2) {
                            ZStack(alignment: .topTrailing) {
                                Image(systemName: tab.symbol).font(.system(size: 14))
                                if let badge = badge(tab) {
                                    Circle().fill(badge).frame(width: 7, height: 7).offset(x: 5, y: -3)
                                }
                            }
                            Text(tab.title).font(.caption2)
                        }
                        .frame(maxWidth: .infinity).padding(.vertical, 5)
                        .foregroundStyle(document.inspectorTab == tab ? Color.accentColor : Color.secondary)
                        .background(document.inspectorTab == tab ? Color.accentColor.opacity(0.12) : Color.clear, in: RoundedRectangle(cornerRadius: 6))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(tab.title)
                    .accessibilityAddTraits(document.inspectorTab == tab ? .isSelected : [])
                }
            }
            .padding(6)
            Divider()
            switch document.inspectorTab {
            case .claude: ClaudePanel(document: document, chatFocus: chatFocus)
            case .project: ProjectInspector(document: document)
            case .outputs: OutputsInspector(document: document)
            case .validation: ValidationView(document: document)
            case .run: RunInspector(document: document)
            }
        }
        .background(.background)
    }

    private func badge(_ tab: InspectorTab) -> Color? {
        switch tab {
        case .validation:
            guard let r = document.report else { return nil }
            return r.blockers > 0 ? .red : (r.warnings > 0 ? .orange : nil)
        case .run:
            return document.runs.question != nil ? .accentColor : nil
        case .claude:
            return document.review.pending != nil ? .accentColor : nil
        default:
            return nil
        }
    }
}

/// A label and its control on one row, with labels in an aligned column. (`LabeledContent`
/// only aligns inside a `Form`; inspectors and the library use plain stacks.)
struct Field<Content: View>: View {
    var label: String
    @ViewBuilder var content: Content
    @Environment(\.fieldLabelWidth) private var width

    init(_ label: String, @ViewBuilder content: () -> Content) {
        self.label = label
        self.content = content()
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            if !label.isEmpty { Text(label).frame(width: width, alignment: .leading).fixedSize(horizontal: false, vertical: true) }
            content
            Spacer(minLength: 0)
        }
    }
}

private struct FieldLabelWidthKey: EnvironmentKey { static let defaultValue: CGFloat = 132 }
extension EnvironmentValues {
    var fieldLabelWidth: CGFloat {
        get { self[FieldLabelWidthKey.self] }
        set { self[FieldLabelWidthKey.self] = newValue }
    }
}

/// A titled group in an inspector form.
struct InspectorSection<Content: View>: View {
    var title: String
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title.uppercased()).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: Project (intent and models, spec §7 and §4.2)

/// Project › App recording: whether production runs may drive and record apps, which apps, and
/// whether macOS allows it.
/// The two macOS permissions a run needs to drive and record apps.
enum CapturePermission: String {
    case screenRecording = "screen_recording"
    case accessibility

    var title: String { self == .screenRecording ? "Screen Recording" : "Accessibility" }
    private var pane: String { self == .screenRecording ? "Privacy_ScreenCapture" : "Privacy_Accessibility" }

    private var service: String { self == .screenRecording ? "ScreenCapture" : "Accessibility" }

    /// Registers the app with macOS for this permission, so it is already in the list, then
    /// opens that list in System Settings. All that is left for the user is the switch.
    ///
    /// This is only offered while macOS says the permission is missing. If the list already
    /// shows the app switched on, that entry belongs to another build: macOS ties a grant to
    /// the app's code signature, and an ad-hoc signed build gets a new one every time it is
    /// built. Such an entry can't be reused, so it is cleared first and this build is added.
    func allow() {
        if let id = Bundle.main.bundleIdentifier {
            let reset = Process()
            reset.executableURL = URL(fileURLWithPath: "/usr/bin/tccutil")
            reset.arguments = ["reset", service, id]
            reset.standardOutput = FileHandle.nullDevice
            reset.standardError = FileHandle.nullDevice
            if (try? reset.run()) != nil { reset.waitUntilExit() }
        }
        switch self {
        case .screenRecording:
            CGRequestScreenCaptureAccess()
        case .accessibility:
            let prompt = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
            _ = AXIsProcessTrustedWithOptions([prompt: true] as CFDictionary)
        }
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(pane)") { NSWorkspace.shared.open(url) }
    }
}

struct AppRecordingSection: View {
    @ObservedObject var document: ProjectDocument
    @State private var permissions: CapturePermissions?
    @State private var appsDraft = ""

    private var capture: AppCapture { document.project.app_capture }

    private func refresh() {
        permissions = try? Core.run("capture.permissions", ["request": false])
    }

    var body: some View {
        InspectorSection(title: "App recording") {
            Toggle("Let Claude drive and record apps", isOn: Binding(get: { capture.enabled }, set: { v in document.update { $0.app_capture.enabled = v } }))
                .help("During a production run, Claude Code opens the desktop app or web app the spec names, operates it and records the screen. cutline only edits the recordings.")
            Text("When a spec asks for footage of an app that isn't attached, the production run records it: Claude Code opens the app or web page, clicks and types, and records the window. Nothing is driven in a dry run.")
                .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            if capture.enabled {
                Field("Apps") {
                    TextField("Apps", text: $appsDraft, prompt: Text("Any app"))
                        .labelsHidden()
                        .onSubmit(commitApps)
                        .help("Names as shown in Applications, separated by commas, for example: Safari, Syncfusion Video Studio. Leave empty to allow any app except terminals, System Settings, password managers and Finder.")
                        .accessibilityLabel("Apps a run may drive")
                }
                .onChange(of: appsDraft) { commitApps() }
                if let p = permissions, p.supported {
                    permissionRow("Screen recording", ok: p.screen_recording, permission: .screenRecording)
                    permissionRow("Accessibility (click and type)", ok: p.accessibility, permission: .accessibility)
                    if !p.screen_recording || !p.accessibility {
                        Button("Re-check") { refresh() }.controlSize(.small)
                        Text("Allow opens System Settings with Syncfusion Video Studio in the list, switched off: turn its switch on. If it was already on, that was for an earlier build of the app, and Allow replaces it. For screen recording, macOS then offers to quit and reopen the app.")
                            .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                } else if permissions != nil {
                    Label("Driving apps isn't available on this system yet.", systemImage: "exclamationmark.triangle").font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .onAppear {
            appsDraft = capture.apps.joined(separator: ", ")
            refresh()
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in refresh() }
    }

    private func commitApps() {
        let apps = appsDraft.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        if apps != capture.apps { document.update { $0.app_capture.apps = apps } }
    }

    private func permissionRow(_ title: String, ok: Bool, permission: CapturePermission) -> some View {
        HStack(spacing: 6) {
            Image(systemName: ok ? "checkmark.circle.fill" : "exclamationmark.circle.fill").foregroundStyle(ok ? Color.green : Color.orange)
            Text(title).font(.callout)
            Spacer(minLength: 0)
            if !ok {
                Button("Allow…") { permission.allow() }
                    .buttonStyle(.link).controlSize(.small)
                    .help("Opens System Settings with Syncfusion Video Studio in the \(permission.title) list")
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(title): \(ok ? "allowed" : "not allowed")")
    }
}

struct ProjectInspector: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject private var app = AppState.shared

    private func text(_ keyPath: WritableKeyPath<Intent, String?>) -> Binding<String> {
        Binding(get: { document.project.intent[keyPath: keyPath] ?? "" }, set: { v in document.update { $0.intent[keyPath: keyPath] = v.isEmpty ? nil : v } })
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                InspectorSection(title: "Project") {
                    TextField("Name", text: Binding(get: { document.project.name }, set: { v in
                        document.update { $0.name = v }
                        document.windowController?.updateTitle()
                    }))
                    Text(document.fileURL.path).font(.caption).foregroundStyle(.secondary).lineLimit(2).truncationMode(.middle).textSelection(.enabled)
                }
                InspectorSection(title: "Intent") {
                    Picker("Intent", selection: Binding(get: { document.project.intent.kind }, set: { v in
                        document.update { $0.intent.kind = v }
                        document.windowController?.updateTitle()
                    })) {
                        if document.project.intent.kind == "unset" { Text("Choose…").tag("unset") }
                        ForEach(IntentKind.all) { Text($0.label).tag($0.key) }
                    }
                    if document.project.intent.kind == "unset" {
                        Label("Required: the intent sets defaults and is given to Claude.", systemImage: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(.orange)
                    }
                    if document.project.intent.kind == "other" { TextField("Describe the intent", text: text(\.other)) }
                    TextField("Audience", text: text(\.audience), prompt: Text("Who is this for?"))
                    TextField("Tone", text: text(\.tone), prompt: Text("Confident, friendly, technical, playful…"))
                    Field("Target length") {
                        HStack(spacing: 4) {
                            TextField("", value: Binding(get: { document.project.intent.target_length_s }, set: { v in document.update { $0.intent.target_length_s = (v ?? 0) > 0 ? v : nil } }), format: .number)
                                .frame(width: 64).multilineTextAlignment(.trailing)
                                .accessibilityLabel("Target length in seconds")
                            Text("seconds").foregroundStyle(.secondary)
                        }
                    }
                    TextField("Call to action", text: text(\.cta), prompt: Text("Start a free trial"))
                    TextField("Languages", text: Binding(get: { document.project.intent.locales.joined(separator: ", ") }, set: { v in
                        document.update { $0.intent.locales = v.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty } }
                    }), prompt: Text("en, de, ja"))
                    .help("Language codes, the base language first")
                    if let d = app.platforms.intents?[document.project.intent.kind] {
                        Text("Defaults for this intent: \(d.pacing) pacing, \(d.captions) captions, \(d.music_level) music. The spec overrides them.")
                            .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                }
                FinishSection(document: document)
                InspectorSection(title: "Models") {
                    ModelPicker(title: "Spec assistant", model: Binding(get: { document.project.models.assistant }, set: { v in document.update { $0.models.assistant = v } }),
                                settingsDefault: AppState.shared.settings.default_assistant_model)
                        .help("Used for chat, spec edits and questions")
                    ModelPicker(title: "Dry run", model: Binding(get: { document.project.models.dry_run }, set: { v in document.update { $0.models.dry_run = v } }),
                                settingsDefault: AppState.shared.settings.default_dry_run_model)
                        .help("The model that rehearses the run before the production model is used. Settings default follows Settings › Claude.")
                    EffortPicker(effort: Binding(get: { document.project.models.effort }, set: { v in document.update { $0.models.effort = v } }))
                    Text("The production model is set on each run config, in Outputs.").font(.caption).foregroundStyle(.secondary)
                }
                AppRecordingSection(document: document)
                InspectorSection(title: "Media search folders") {
                    if document.project.media_roots.isEmpty {
                        Text("Folders searched for media that has moved.").font(.caption).foregroundStyle(.secondary)
                    }
                    ForEach(document.project.media_roots, id: \.self) { root in
                        HStack {
                            Image(systemName: "folder").foregroundStyle(.secondary)
                            Text(root).lineLimit(1).truncationMode(.middle)
                            Spacer()
                            Button { document.update { $0.media_roots.removeAll { $0 == root } } } label: { Image(systemName: "minus.circle") }.buttonStyle(.borderless)
                                .accessibilityLabel("Remove \(root)")
                        }
                    }
                    Button("Add Folder…") {
                        let panel = NSOpenPanel()
                        panel.canChooseDirectories = true
                        panel.canChooseFiles = false
                        guard panel.runModal() == .OK, let url = panel.url else { return }
                        document.update { if !$0.media_roots.contains(url.path) { $0.media_roots.append(url.path) } }
                        document.resolveMedia()
                    }
                }
            }
            .textFieldStyle(.roundedBorder)
            .controlSize(.small)
            .padding(14)
        }
    }
}

// MARK: Outputs and run configs (spec §9)

struct OutputsInspector: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject private var app = AppState.shared
    @State private var editingOutput: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                runConfigSwitcher
                if let rc = document.selectedRunConfig {
                    RunConfigEditor(document: document, rc: rc, editingOutput: $editingOutput)
                } else {
                    EmptyState(symbol: "rectangle.stack", title: "No run config", message: "A run config says which outputs to make, with which model, into which folder.", action: ("Add Run Config", { document.addRunConfig(named: "Default") }))
                }
            }
            .textFieldStyle(.roundedBorder)
            .controlSize(.small)
            .padding(14)
        }
        .sheet(item: Binding(get: { editingOutput.map(IdentifiedString.init) }, set: { editingOutput = $0?.id })) { item in
            OutputEditor(document: document, outputID: item.id)
        }
    }

    private var runConfigSwitcher: some View {
        InspectorSection(title: "Run config") {
            HStack {
                if document.project.run_configs.count <= 4 && !document.project.run_configs.isEmpty {
                    Picker("", selection: Binding(get: { document.selectedRunConfig?.id ?? "" }, set: { document.selectedRunConfigID = $0 })) {
                        ForEach(document.project.run_configs) { Text($0.name).tag($0.id) }
                    }
                    .pickerStyle(.segmented).labelsHidden()
                    .accessibilityLabel("Run config")
                } else if !document.project.run_configs.isEmpty {
                    Picker("", selection: Binding(get: { document.selectedRunConfig?.id ?? "" }, set: { document.selectedRunConfigID = $0 })) {
                        ForEach(document.project.run_configs) { Text($0.name).tag($0.id) }
                    }
                    .labelsHidden()
                }
                Menu {
                    Button("New Run Config…") {
                        if let name = Prompt.text(title: "New Run Config", message: "For example “Social pack” or “YouTube final”.", initial: "") { document.addRunConfig(named: name) }
                    }
                    if let rc = document.selectedRunConfig {
                        Button("Duplicate “\(rc.name)”") {
                            document.addRunConfig(named: rc.name + " copy")
                            if let new = document.selectedRunConfig { document.updateRunConfig(new.id) { n in let id = n.id, name = n.name; n = rc; n.id = id; n.name = name } }
                        }
                        Button("Rename…") {
                            if let name = Prompt.text(title: "Rename Run Config", message: "", initial: rc.name) { document.updateRunConfig(rc.id) { $0.name = name } }
                        }
                        Divider()
                        Button("Delete “\(rc.name)”", role: .destructive) {
                            if Prompt.confirm("Delete the run config “\(rc.name)”?", message: "Past runs made with it stay in the history.", ok: "Delete", destructive: true) { document.deleteRunConfig(rc.id) }
                        }
                    }
                } label: { Image(systemName: "ellipsis.circle") }
                .menuStyle(.borderlessButton).fixedSize()
                .accessibilityLabel("Run config actions")
            }
        }
    }
}

struct IdentifiedString: Identifiable { var id: String }

struct RunConfigEditor: View {
    @ObservedObject var document: ProjectDocument
    var rc: RunConfig
    @Binding var editingOutput: String?
    @ObservedObject private var app = AppState.shared

    private func bind<T>(_ keyPath: WritableKeyPath<RunConfig, T>) -> Binding<T> {
        Binding(get: { rc[keyPath: keyPath] }, set: { v in document.updateRunConfig(rc.id) { $0[keyPath: keyPath] = v } })
    }

    var body: some View {
        InspectorSection(title: "Outputs in this run") {
            if document.project.output_configs.isEmpty {
                Text("Add an output to say where the video is going.").font(.caption).foregroundStyle(.secondary)
            }
            ForEach(document.project.output_configs) { out in
                let resolved = document.resolvedOutput(out.id)
                HStack(spacing: 8) {
                    Toggle("", isOn: Binding(get: { rc.outputs.contains(out.id) }, set: { on in
                        document.updateRunConfig(rc.id) { c in
                            if on { if !c.outputs.contains(out.id) { c.outputs.append(out.id) } } else { c.outputs.removeAll { $0 == out.id } }
                        }
                    }))
                    .labelsHidden().toggleStyle(.checkbox)
                    .accessibilityLabel("Include \(resolved?.name ?? out.preset)")
                    VStack(alignment: .leading, spacing: 1) {
                        Text(resolved?.name ?? out.name ?? app.platforms.preset(out.preset)?.name ?? out.preset).lineLimit(1)
                        if let e = resolved?.error {
                            Text(e).font(.caption).foregroundStyle(.red).lineLimit(2)
                        } else if let r = resolved, let w = r.width, let h = r.height {
                            Text("\(w)×\(h) · \(Fmt.aspect(width: w, height: h))\(r.limits?.max_duration_s.map { " · max \(Fmt.duration($0))" } ?? "")\((r.exports?.count ?? 1) > 1 ? " · \(r.exports!.map(\.format).joined(separator: " + "))" : "")")
                                .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }
                    Spacer(minLength: 0)
                    Button { editingOutput = out.id } label: { Image(systemName: "slider.horizontal.3") }.buttonStyle(.borderless).help("Edit this output")
                        .accessibilityLabel("Edit \(resolved?.name ?? out.preset)")
                }
            }
            Menu {
                ForEach(Dictionary(grouping: app.platforms.presets, by: \.platform).sorted { $0.key < $1.key }, id: \.key) { platform, presets in
                    if presets.count == 1, let p = presets.first {
                        Button(p.name) { document.addOutput(preset: p) }
                    } else {
                        Menu(platform) { ForEach(presets) { p in Button(p.name) { document.addOutput(preset: p) } } }
                    }
                }
                Divider()
                Button("Edit Output Presets…") { AppDelegate.shared.showLibrary(tab: .presets, select: nil) }
            } label: { Label("Add Output", systemImage: "plus") }
            .fixedSize()
        }
        InspectorSection(title: "Production") {
            ModelPicker(title: "Model", model: bind(\.production_model))
            EffortPicker(effort: bind(\.effort))
            Picker("Quality", selection: bind(\.quality)) {
                Text("Final").tag("final")
                Text("Draft").tag("draft")
            }
            .help("Draft renders fast at lower resolution, to judge the edit")
            Toggle("Dry run first", isOn: bind(\.dry_run_first))
                .help("A cheaper model writes the edit and runs cutline's checks before the production model is used")
            Toggle("Allow web access", isOn: bind(\.web_access))
            TextField("Languages", text: Binding(get: { rc.locales.joined(separator: ", ") }, set: { v in
                document.updateRunConfig(rc.id) { $0.locales = v.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty } }
            }), prompt: Text(document.project.intent.locales.isEmpty ? "The project's languages" : document.project.intent.locales.joined(separator: ", ")))
        }
        InspectorSection(title: "Output folder") {
            let root = document.outputRoot(for: rc)
            HStack {
                Image(systemName: "folder").foregroundStyle(.secondary)
                Text(root).lineLimit(2).truncationMode(.middle).font(.callout).textSelection(.enabled)
                Spacer(minLength: 0)
            }
            HStack {
                Button("Choose…") {
                    let panel = NSOpenPanel()
                    panel.canChooseDirectories = true
                    panel.canChooseFiles = false
                    panel.canCreateDirectories = true
                    panel.prompt = "Use Folder"
                    panel.message = "Each run writes a dated subfolder here."
                    guard panel.runModal() == .OK, let url = panel.url else { return }
                    document.updateRunConfig(rc.id) { $0.output_folder = url.path }
                    app.settings.recent_output_folders.removeAll { $0 == url.path }
                    app.settings.recent_output_folders.insert(url.path, at: 0)
                    app.settings.recent_output_folders = Array(app.settings.recent_output_folders.prefix(8))
                    app.saveSettings()
                }
                Menu("Recent") {
                    Button("Default (\(app.settings.default_output_folder.isEmpty ? "~/Movies/Syncfusion Video Studio" : app.settings.default_output_folder))") { document.updateRunConfig(rc.id) { $0.output_folder = "" } }
                    if !app.settings.recent_output_folders.isEmpty { Divider() }
                    ForEach(app.settings.recent_output_folders, id: \.self) { f in
                        Button(f) { document.updateRunConfig(rc.id) { $0.output_folder = f } }
                    }
                }
                .fixedSize()
                Button("Reveal") { NSWorkspace.shared.selectFile(nil, inFileViewerRootedAtPath: root) }
                    .disabled(!FileManager.default.fileExists(atPath: root))
            }
            Text("Runs write to a new “date-time-\(rc.name.lowercased().replacingOccurrences(of: " ", with: "-"))” folder inside. Changing the folder never moves past runs.")
                .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        InspectorSection(title: "Limits") {
            Field("ElevenLabs credit cap") {
                TextField("", value: bind(\.eleven_credit_cap), format: .number).frame(width: 90).multilineTextAlignment(.trailing)
                    .accessibilityLabel("ElevenLabs credit cap")
            }
            Field("Claude spend limit") {
                HStack(spacing: 3) {
                    Text("$").foregroundStyle(.secondary)
                    TextField("none", value: Binding(get: { rc.max_spend_usd }, set: { v in document.updateRunConfig(rc.id) { $0.max_spend_usd = (v ?? 0) > 0 ? v : nil } }), format: .number)
                        .frame(width: 76).multilineTextAlignment(.trailing).accessibilityLabel("Claude spend limit in dollars")
                }
            }
            Field("Turn limit") {
                TextField("none", value: Binding(get: { rc.max_turns }, set: { v in document.updateRunConfig(rc.id) { $0.max_turns = (v ?? 0) > 0 ? v : nil } }), format: .number)
                    .frame(width: 90).multilineTextAlignment(.trailing).accessibilityLabel("Turn limit")
            }
            Field("Question timeout") {
                HStack(spacing: 4) {
                    TextField("wait", value: Binding(get: { rc.question_timeout_s }, set: { v in document.updateRunConfig(rc.id) { $0.question_timeout_s = (v ?? 0) > 0 ? v : nil } }), format: .number)
                        .frame(width: 64).multilineTextAlignment(.trailing).accessibilityLabel("Question timeout in seconds")
                    Text("s").foregroundStyle(.secondary)
                }
            }
            .help("How long a question from Claude waits before the run proceeds on Claude's stated assumption. Empty waits for you.")
        }
    }
}

/// One output config: its preset and the per-project overrides.
struct OutputEditor: View {
    @ObservedObject var document: ProjectDocument
    var outputID: String
    @ObservedObject private var app = AppState.shared
    @Environment(\.dismiss) private var dismiss

    private var config: OutputConfig? { document.project.output_configs.first { $0.id == outputID } }

    private func bind<T>(_ keyPath: WritableKeyPath<OutputOverrides, T>) -> Binding<T> {
        Binding(get: { config!.overrides[keyPath: keyPath] }, set: { v in document.updateOutput(outputID) { $0.overrides[keyPath: keyPath] = v } })
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if let config, let preset = app.platforms.preset(config.preset) {
                let target = config.overrides.target ?? preset.targets.first ?? "long_1080p"
                Text(config.name ?? preset.name).font(.title3.weight(.semibold))
                if !preset.notes.isEmpty { Text(preset.notes).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true) }
                Form {
                    TextField("Name", text: Binding(get: { config.name ?? "" }, set: { v in document.updateOutput(outputID) { $0.name = v.isEmpty ? nil : v } }), prompt: Text(preset.name))
                    Picker("Preset", selection: Binding(get: { config.preset }, set: { v in document.updateOutput(outputID) { $0.preset = v; $0.overrides = OutputOverrides() } })) {
                        ForEach(app.platforms.presets) { Text($0.name).tag($0.id) }
                    }
                    if preset.targets.count > 1 {
                        Picker("Shape", selection: Binding(get: { target }, set: { v in document.updateOutput(outputID) { $0.overrides.target = v == preset.targets.first ? nil : v } })) {
                            ForEach(preset.targets, id: \.self) { Text(Self.targetLabel($0)).tag($0) }
                        }
                    }
                    if target == "custom" {
                        if let sizes = preset.sizes, !sizes.isEmpty {
                            Picker("Device size", selection: Binding(get: { NamedSize(name: "", width: config.overrides.width ?? preset.width ?? 1920, height: config.overrides.height ?? preset.height ?? 1080) }, set: { s in
                                document.updateOutput(outputID) { $0.overrides.width = s.width; $0.overrides.height = s.height }
                            })) {
                                ForEach(sizes, id: \.self) { s in Text("\(s.name) (\(s.width)×\(s.height))").tag(NamedSize(name: "", width: s.width, height: s.height)) }
                            }
                        }
                        LabeledContent("Size") {
                            HStack(spacing: 4) {
                                TextField("", value: Binding(get: { config.overrides.width ?? preset.width ?? 1920 }, set: { v in document.updateOutput(outputID) { $0.overrides.width = v } }), format: .number.grouping(.never)).frame(width: 70)
                                    .accessibilityLabel("Width")
                                Text("×")
                                TextField("", value: Binding(get: { config.overrides.height ?? preset.height ?? 1080 }, set: { v in document.updateOutput(outputID) { $0.overrides.height = v } }), format: .number.grouping(.never)).frame(width: 70)
                                    .accessibilityLabel("Height")
                            }
                        }
                    }
                    LabeledContent("Frame rate") {
                        TextField("source", value: Binding(get: { config.overrides.fps ?? preset.fps }, set: { v in document.updateOutput(outputID) { $0.overrides.fps = (v ?? 0) > 0 ? v : nil } }), format: .number).frame(width: 70)
                            .accessibilityLabel("Frame rate")
                    }
                    Picker("Captions", selection: Binding(get: { config.overrides.captions ?? preset.captions }, set: { v in document.updateOutput(outputID) { $0.overrides.captions = v == preset.captions ? nil : v } })) {
                        Text("Burned in").tag("burn")
                        Text("Sidecar files only").tag("sidecar")
                        Text("None").tag("none")
                    }
                    Toggle("Audio", isOn: Binding(get: { config.overrides.audio ?? preset.audio }, set: { v in document.updateOutput(outputID) { $0.overrides.audio = v == preset.audio ? nil : v } }))
                    Toggle("Seamless loop", isOn: Binding(get: { config.overrides.loop ?? preset.loop }, set: { v in document.updateOutput(outputID) { $0.overrides.loop = v == preset.loop ? nil : v } }))
                        .help("The end dissolves into the start. A loop has no audio.")
                    LabeledContent("Max length") {
                        HStack(spacing: 4) {
                            TextField(preset.limits.max_duration_s.map { "\(Int($0))" } ?? "none", value: bind(\.max_duration_s), format: .number).frame(width: 70).accessibilityLabel("Maximum length in seconds")
                            Text("s").foregroundStyle(.secondary)
                        }
                    }
                    LabeledContent("Size cap") {
                        HStack(spacing: 4) {
                            TextField(preset.max_bytes.map { "\($0 / 1_000_000)" } ?? "none", value: Binding(get: { (config.overrides.max_bytes ?? 0) > 0 ? Double(config.overrides.max_bytes!) / 1_000_000 : nil }, set: { (v: Double?) in
                                document.updateOutput(outputID) { $0.overrides.max_bytes = (v ?? 0) > 0 ? UInt64(v! * 1_000_000) : nil }
                            }), format: .number).frame(width: 70).accessibilityLabel("Size cap in megabytes")
                            Text("MB").foregroundStyle(.secondary)
                        }
                    }
                    LabeledContent("Formats") {
                        HStack {
                            ForEach(["mp4", "webm"], id: \.self) { f in
                                Toggle(f.uppercased(), isOn: Binding(get: { (config.overrides.formats ?? preset.formats).contains(f) }, set: { on in
                                    var list = config.overrides.formats ?? preset.formats
                                    if on { if !list.contains(f) { list.append(f) } } else if list.count > 1 { list.removeAll { $0 == f } }
                                    document.updateOutput(outputID) { $0.overrides.formats = list == preset.formats ? nil : list }
                                }))
                                .toggleStyle(.checkbox)
                            }
                        }
                    }
                }
                .textFieldStyle(.roundedBorder)
                if let r = document.resolvedOutput(outputID), let w = r.width, let h = r.height {
                    Text("Renders \(w)×\(h) (\(Fmt.aspect(width: w, height: h)))\(r.limits?.min_duration_s.map { ", at least \(Fmt.duration($0))" } ?? "")\(r.limits?.max_duration_s.map { ", at most \(Fmt.duration($0))" } ?? "")\(r.limits?.max_bytes.map { ", under \(Fmt.bytes($0))" } ?? "").")
                        .font(.caption).foregroundStyle(.secondary)
                }
            } else {
                Text("This output uses a preset that no longer exists.").foregroundStyle(.red)
            }
            HStack {
                Button("Delete Output", role: .destructive) {
                    document.deleteOutput(outputID)
                    dismiss()
                }
                Spacer()
                Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 440)
    }

    static func targetLabel(_ target: String) -> String {
        switch target {
        case "shorts": return "Vertical 9:16 (1080×1920)"
        case "long_1080p": return "Landscape 16:9 (1920×1080)"
        case "long_4k": return "Landscape 4K (3840×2160)"
        case "square": return "Square 1:1 (1080×1080)"
        case "portrait_4x5": return "Portrait 4:5 (1080×1350)"
        case "custom": return "Custom size"
        default: return target
        }
    }
}
