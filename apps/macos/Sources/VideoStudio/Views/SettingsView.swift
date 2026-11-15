import SwiftUI
import AppKit

enum SettingsPane: String, CaseIterable, Identifiable {
    case general, claude, cutline, elevenlabs, storage, editor
    var id: String { rawValue }
    var title: String {
        switch self {
        case .general: return "General"
        case .claude: return "Claude"
        case .cutline: return "cutline"
        case .elevenlabs: return "ElevenLabs"
        case .storage: return "Storage"
        case .editor: return "Editor"
        }
    }
    var symbol: String {
        switch self {
        case .general: return "gearshape"
        case .claude: return "sparkles"
        case .cutline: return "scissors"
        case .elevenlabs: return "waveform"
        case .storage: return "internaldrive"
        case .editor: return "text.cursor"
        }
    }
}

@MainActor
final class SettingsWindowState: ObservableObject {
    @Published var pane: SettingsPane = .general
}

/// Settings (spec §18).
struct SettingsView: View {
    @ObservedObject var state: SettingsWindowState
    @ObservedObject private var app = AppState.shared

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 4) {
                ForEach(SettingsPane.allCases) { pane in
                    Button { state.pane = pane } label: {
                        VStack(spacing: 3) {
                            Image(systemName: pane.symbol).font(.system(size: 18))
                            Text(pane.title).font(.caption)
                        }
                        .frame(width: 84, height: 46)
                        .foregroundStyle(state.pane == pane ? Color.accentColor : Color.secondary)
                        .background(state.pane == pane ? Color.accentColor.opacity(0.12) : Color.clear, in: RoundedRectangle(cornerRadius: 7))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(pane.title)
                    .accessibilityAddTraits(state.pane == pane ? .isSelected : [])
                }
            }
            .padding(8)
            Divider()
            ScrollView {
                Group {
                    switch state.pane {
                    case .general: GeneralSettings()
                    case .claude: ClaudeSettings()
                    case .cutline: CutlineSettings()
                    case .elevenlabs: ElevenSettings()
                    case .storage: StorageSettings()
                    case .editor: EditorSettings()
                    }
                }
                .padding(22)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(width: 680, height: 520)
        .onChange(of: app.settings) { app.saveSettings() }
    }
}

private func chooseFolder(_ message: String) -> URL? {
    let panel = NSOpenPanel()
    panel.canChooseDirectories = true
    panel.canChooseFiles = false
    panel.canCreateDirectories = true
    panel.message = message
    return panel.runModal() == .OK ? panel.url : nil
}

struct GeneralSettings: View {
    @ObservedObject private var app = AppState.shared

    var body: some View {
        Form {
            LabeledContent("Default output folder") {
                HStack {
                    Text(app.settings.default_output_folder.isEmpty ? "~/Movies/Syncfusion Video Studio" : app.settings.default_output_folder).lineLimit(1).truncationMode(.middle)
                    Button("Choose…") { if let u = chooseFolder("Run folders are created inside a subfolder per project.") { app.settings.default_output_folder = u.path } }
                    if !app.settings.default_output_folder.isEmpty { Button("Reset") { app.settings.default_output_folder = "" } }
                }
            }
            Stepper("Runs at the same time: \(app.settings.parallel_run_limit)", value: $app.settings.parallel_run_limit, in: 1...4)
                .help("Runs of different projects that may go at once. Renders are CPU-heavy, so the default is 1. A project never has two runs at once.")
            Toggle("Notify when a run finishes or needs an answer", isOn: $app.settings.notifications)
            LabeledContent("Reading speed") {
                HStack {
                    TextField("", value: $app.settings.words_per_minute, format: .number).frame(width: 60).accessibilityLabel("Words per minute")
                    Text("words per minute").foregroundStyle(.secondary)
                }
            }
            .help("Used to check that the voice-over fits the target length")
            Section("Media search folders") {
                Text("Searched for media that has moved, in every project.").font(.caption).foregroundStyle(.secondary)
                ForEach(app.settings.media_roots, id: \.self) { root in
                    HStack {
                        Image(systemName: "folder").foregroundStyle(.secondary)
                        Text(root).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        Button { app.settings.media_roots.removeAll { $0 == root } } label: { Image(systemName: "minus.circle") }.buttonStyle(.borderless).accessibilityLabel("Remove \(root)")
                    }
                }
                Button("Add Folder…") { if let u = chooseFolder("Choose a folder to search for moved media.") , !app.settings.media_roots.contains(u.path) { app.settings.media_roots.append(u.path) } }
            }
        }
        .textFieldStyle(.roundedBorder)
    }
}

struct ClaudeSettings: View {
    @ObservedObject private var app = AppState.shared

    var body: some View {
        Form {
            Section("Claude Code") {
                HStack(alignment: .top) {
                    if app.isDetecting {
                        ProgressView().controlSize(.small)
                        Text("Checking…")
                    } else if let c = app.environment?.claude {
                        Image(systemName: c.supported ? "checkmark.circle.fill" : "exclamationmark.triangle.fill").foregroundStyle(c.supported ? Color.green : Color.orange)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(c.version_text)
                            Text(c.path).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                            if !c.supported { Text("This app needs Claude Code \(app.environment?.min_claude_version ?? "") or later.").font(.caption).foregroundStyle(.orange) }
                            if app.environment?.signed_in == false { Text("Not signed in. Run `claude` in Terminal and sign in.").font(.caption).foregroundStyle(.orange) }
                        }
                    } else {
                        Image(systemName: "xmark.octagon.fill").foregroundStyle(.red)
                        Text("Claude Code wasn't found. It is required.")
                    }
                    Spacer()
                    Button("Re-check") { app.detect(force: true) }.disabled(app.isDetecting)
                }
                TextField("CLI path", text: $app.settings.claude_path, prompt: Text("Detected automatically"))
                    .onSubmit { app.detect(force: true) }
                Text("Sign-in is handled by the CLI. The app never asks for or stores Anthropic credentials.").font(.caption).foregroundStyle(.secondary)
            }
            Section("Default models for new projects") {
                ModelPicker(title: "Spec assistant", model: $app.settings.default_assistant_model)
                ModelPicker(title: "Dry run", model: $app.settings.default_dry_run_model)
                ModelPicker(title: "Production", model: $app.settings.default_production_model)
                if !app.effortLevels.isEmpty {
                    Picker("Effort", selection: $app.settings.effort) {
                        Text("Default").tag("")
                        ForEach(app.effortLevels, id: \.self) { Text($0.capitalized).tag($0) }
                    }
                }
                HStack {
                    Text("Model list updated \(app.models.updated ?? "with the app")").font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button("Refresh") { app.reloadModels() }.help("Reloads models.json from the app library (a newer copy there replaces the built-in list)")
                    Button("Reveal Library") { NSWorkspace.shared.selectFile(nil, inFileViewerRootedAtPath: app.libraryURL.path) }
                }
            }
            Section("Runs") {
                Toggle("Allow web access by default", isOn: $app.settings.web_access_default)
                LabeledContent("Question timeout in headless runs") {
                    HStack {
                        TextField("", value: $app.settings.headless_question_timeout_s, format: .number).frame(width: 60).accessibilityLabel("Question timeout in seconds")
                        Text("seconds (0 waits)").foregroundStyle(.secondary)
                    }
                }
                Toggle("Show token usage and cost after each assistant reply", isOn: Binding(get: { AppSettings.shared.showClaudeUsage }, set: { AppSettings.shared.showClaudeUsage = $0 }))
            }
        }
        .textFieldStyle(.roundedBorder)
    }
}

struct CutlineSettings: View {
    @ObservedObject private var app = AppState.shared
    @State private var busy = ""
    @State private var output = ""

    private func run(_ args: [String], label: String) {
        guard let cutline = app.environment?.cutline?.path, busy.isEmpty else { return }
        busy = label
        output = ""
        Task {
            let text = await Task.detached { () -> String in
                let p = Process()
                p.executableURL = URL(fileURLWithPath: cutline)
                p.arguments = args
                let pipe = Pipe()
                p.standardOutput = pipe
                p.standardError = pipe
                do { try p.run() } catch { return error.localizedDescription }
                let data = pipe.fileHandleForReading.readDataToEndOfFile()
                p.waitUntilExit()
                return String(decoding: data, as: UTF8.self) + "\n(exit \(p.terminationStatus))"
            }.value
            output = text
            busy = ""
            app.detect(force: true)
        }
    }

    var body: some View {
        Form {
            Section("cutline") {
                HStack(alignment: .top) {
                    if let c = app.environment?.cutline {
                        let ok = app.environment?.doctor?.ok == true
                        Image(systemName: ok ? "checkmark.circle.fill" : "exclamationmark.triangle.fill").foregroundStyle(ok ? Color.green : Color.orange)
                        VStack(alignment: .leading, spacing: 2) {
                            Text("cutline \(c.version)")
                            Text(c.path).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                            if let d = app.environment?.doctor {
                                if !d.missing_required.isEmpty { Text("Missing: " + d.missing_required.joined(separator: ", ")).font(.caption).foregroundStyle(.red) }
                                if !d.missing_optional.isEmpty { Text("Optional, not installed: " + d.missing_optional.joined(separator: ", ")).font(.caption).foregroundStyle(.secondary) }
                            }
                        }
                    } else {
                        Image(systemName: "xmark.octagon.fill").foregroundStyle(.red)
                        Text("cutline wasn't found. It is required.")
                    }
                    Spacer()
                    Button("Re-check") { app.detect(force: true) }.disabled(app.isDetecting)
                }
                TextField("Binary path", text: $app.settings.cutline_path, prompt: Text("cutline on PATH")).onSubmit { app.detect(force: true) }
                TextField("cutline repository", text: $app.settings.cutline_repo, prompt: Text("Optional: uses bin/cutline inside it")).onSubmit { app.detect(force: true) }
                Picker("Default quality for new run configs", selection: $app.settings.default_quality) {
                    Text("Final").tag("final")
                    Text("Draft").tag("draft")
                }
            }
            Section("Tools") {
                HStack {
                    Button("Run Doctor") { run(["doctor"], label: "Running doctor…") }
                    Button("Install Dependencies") { run(["deps", "install", "--all"], label: "Installing dependencies…") }
                        .help("Runs `cutline deps install --all`: ffmpeg, models and Chrome for HTML graphics")
                    if !busy.isEmpty { ProgressView().controlSize(.small); Text(busy).font(.caption).foregroundStyle(.secondary) }
                }
                .disabled(app.environment?.cutline == nil || !busy.isEmpty)
                if !output.isEmpty {
                    ScrollView {
                        Text(output).font(.system(size: 11, design: .monospaced)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).padding(8)
                    }
                    .frame(height: 170)
                    .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color(nsColor: .separatorColor)))
                    .overlay(alignment: .topTrailing) { CopyButton("Copy the output") { output }.padding(4) }
                }
            }
        }
        .textFieldStyle(.roundedBorder)
    }
}

struct ElevenSettings: View {
    @ObservedObject private var app = AppState.shared
    @State private var newKey = ""
    @State private var testing = false
    @State private var cacheBytes: UInt64 = 0

    var body: some View {
        Form {
            Section("API key") {
                HStack {
                    if app.keyStatus.present {
                        Image(systemName: "key.fill").foregroundStyle(.green)
                        Text(app.keyStatus.masked ?? "••••").font(.system(.body, design: .monospaced))
                    } else {
                        Image(systemName: "key").foregroundStyle(.secondary)
                        Text("No key stored").foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button {
                        testing = true
                        Task {
                            let s = await Task.detached { Helper.testKey() }.value
                            app.refreshKeyStatus(test: true)
                            testing = false
                            _ = s
                        }
                    } label: { if testing { ProgressView().controlSize(.small) } else { Text("Test") } }
                    .disabled(!app.keyStatus.present || testing)
                    if app.keyStatus.present {
                        Button("Remove", role: .destructive) {
                            Task { _ = await Task.detached { Helper.deleteKey() }.value; app.refreshKeyStatus() }
                        }
                    }
                }
                if let r = app.keyStatus.remaining {
                    Label("The key works. \(r) of \(app.keyStatus.limit ?? 0) credits remain\(app.keyStatus.tier.map { " (\($0))" } ?? "").", systemImage: "checkmark.circle.fill").font(.caption).foregroundStyle(.green)
                } else if let e = app.keyStatus.error {
                    Label(e, systemImage: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(.red)
                }
                HStack {
                    SecureField("Paste a new key", text: $newKey).accessibilityLabel("New ElevenLabs API key")
                    Button("Save") {
                        let key = newKey.trimmingCharacters(in: .whitespacesAndNewlines)
                        newKey = ""
                        Task { _ = await Task.detached { Helper.setKey(key) }.value; app.refreshKeyStatus() }
                    }
                    .disabled(newKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                Text("The key is stored in the Keychain and read only by the app's helper during a run. It is never shown in full, never logged, and never given to Claude.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Section("Defaults") {
                TextField("Default voice ID", text: $app.settings.eleven_default_voice, prompt: Text("Used when a brand preset has no voice"))
                LabeledContent("Default credit cap per run") {
                    TextField("", value: $app.settings.eleven_default_credit_cap, format: .number).frame(width: 100).accessibilityLabel("Default credit cap per run")
                }
            }
            Section("Cache") {
                Text("Identical requests are served from the cache, so re-running an unchanged script costs nothing.").font(.caption).foregroundStyle(.secondary)
                LabeledContent("Size limit") {
                    HStack {
                        TextField("", value: Binding(get: { Double(app.settings.eleven_cache_limit_bytes) / 1_000_000_000 }, set: { app.settings.eleven_cache_limit_bytes = UInt64(max(0.1, $0) * 1_000_000_000) }), format: .number).frame(width: 70).accessibilityLabel("Cache size limit in gigabytes")
                        Text("GB").foregroundStyle(.secondary)
                    }
                }
                HStack {
                    Text("Using \(Fmt.bytes(cacheBytes))")
                    Spacer()
                    Button("Clear Cache") {
                        _ = try? Core.raw("storage.clear_eleven_cache")
                        refresh()
                    }
                    .disabled(cacheBytes == 0)
                }
            }
        }
        .textFieldStyle(.roundedBorder)
        .onAppear { refresh(); app.refreshKeyStatus() }
    }

    private func refresh() {
        let info: StorageInfo? = try? Core.run("storage.info", ["run_folders": [String]()])
        cacheBytes = info?.eleven_cache_bytes ?? 0
    }
}

struct StorageSettings: View {
    @ObservedObject private var app = AppState.shared
    @State private var info = StorageInfo()
    @State private var loading = false

    var body: some View {
        Form {
            Section("Sizes") {
                LabeledContent("ElevenLabs cache", value: Fmt.bytes(info.eleven_cache_bytes))
                LabeledContent("Dry runs", value: Fmt.bytes(info.dry_runs_bytes))
                LabeledContent("Run folders (\(info.runs.filter(\.exists).count))", value: Fmt.bytes(info.runs.filter { $0.kind != "dry_run" }.map(\.total_bytes).reduce(0, +)))
                LabeledContent("Logs in those runs", value: Fmt.bytes(info.runs.map(\.logs_bytes).reduce(0, +)))
            }
            Section("Clean up") {
                Text("Logs and runs are never removed automatically.").font(.caption).foregroundStyle(.secondary)
                HStack {
                    Text("Delete runs older than")
                    TextField("", value: $app.settings.cleanup_older_than_days, format: .number).frame(width: 50).accessibilityLabel("Days")
                    Text("days")
                    Spacer()
                    Button("Clean Up…") {
                        let days = app.settings.cleanup_older_than_days
                        let old = info.runs.filter { r in
                            guard r.exists, let d = Fmt.date(r.started) else { return false }
                            return Date().timeIntervalSince(d) >= Double(days) * 86400
                        }
                        guard !old.isEmpty else { Prompt.error("Nothing is older than \(days) days."); return }
                        let size = old.map(\.total_bytes).reduce(0, +)
                        guard Prompt.confirm("Delete \(old.count) run folder\(old.count == 1 ? "" : "s")?", message: "\(Fmt.bytes(size)) of outputs and logs older than \(days) days will be deleted. This can't be undone.", ok: "Delete", destructive: true) else { return }
                        let _: UInt64? = try? Core.run("storage.cleanup", ["run_folders": app.knownRunFolders, "days": days])
                        load()
                    }
                }
            }
            Section("Runs") {
                if loading { ProgressView().controlSize(.small) }
                ForEach(info.runs.filter(\.exists).sorted { ($0.started ?? "") > ($1.started ?? "") }.prefix(40)) { r in
                    HStack {
                        VStack(alignment: .leading, spacing: 1) {
                            Text((r.folder as NSString).lastPathComponent).lineLimit(1).truncationMode(.middle)
                            Text([r.kind == "dry_run" ? "Dry run" : "Production", Fmt.when(r.started)].joined(separator: " · ")).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Text(Fmt.bytes(r.total_bytes)).monospacedDigit().foregroundStyle(.secondary)
                        Button { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: r.folder)]) } label: { Image(systemName: "magnifyingglass.circle") }.buttonStyle(.borderless)
                            .accessibilityLabel("Reveal \((r.folder as NSString).lastPathComponent)")
                    }
                }
            }
        }
        .textFieldStyle(.roundedBorder)
        .onAppear { load() }
    }

    private func load() {
        loading = true
        let folders = app.knownRunFolders
        Task {
            let i: StorageInfo? = try? await Core.async("storage.info", ["run_folders": folders])
            info = i ?? StorageInfo()
            loading = false
        }
    }
}

struct EditorSettings: View {
    @ObservedObject private var s = AppSettings.shared

    var body: some View {
        Form {
            Section("Text") {
                Picker("Font", selection: $s.fontName) {
                    Text("SF Mono").tag("SF Mono")
                    Text("System").tag("System")
                    Text("Menlo").tag("Menlo")
                    Text("Courier New").tag("Courier New")
                }
                Stepper("Size: \(Int(s.fontSize)) pt", value: $s.fontSize, in: 10...28)
                LabeledContent("Line height") {
                    Slider(value: $s.lineHeight, in: 1...2).frame(width: 180).accessibilityLabel("Line height")
                }
                Toggle("Wrap lines", isOn: $s.softWrap)
                Toggle("Line numbers", isOn: $s.lineNumbers)
                Toggle("Check spelling", isOn: $s.spellCheck)
                Toggle("Check grammar", isOn: $s.grammarCheck)
            }
            Section("Editing") {
                Toggle("Close brackets and quotes automatically", isOn: $s.autoPair)
                Toggle("Format tables when leaving them", isOn: $s.autoFormatTables)
                Toggle("Show the table toolbar", isOn: $s.showTableToolbar)
                Toggle("Preview images when the pointer rests on a link", isOn: $s.imageHoverPreview)
                Toggle("Show images and diagrams under their source", isOn: $s.inlineRendering)
                Toggle("Tint islands", isOn: $s.highlightIslands)
            }
            Section("Preview") {
                Picker("Appearance", selection: $s.previewTheme) {
                    Text("Match the system").tag("system")
                    Text("Light").tag("light")
                    Text("Dark").tag("dark")
                }
                Toggle("Load remote images", isOn: $s.loadRemoteImages)
                    .help("Off by default: loading a remote image tells its server when the spec was viewed")
                Toggle("Outline islands", isOn: $s.showIslandOutlines)
            }
        }
    }
}
