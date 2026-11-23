import SwiftUI
import AppKit
import UniformTypeIdentifiers

/// The window shown with no project open: recent projects, New Project, and the first-run
/// checklist (spec §17 Onboarding).
struct WelcomeView: View {
    var startWithNewProject = false
    @ObservedObject private var app = AppState.shared
    @State private var showNew = false

    private var recents: [URL] { NSDocumentController.shared.recentDocumentURLs.filter { $0.pathExtension == "cutproj" } }

    var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 14) {
                Image(nsImage: NSApp.applicationIconImage).resizable().frame(width: 72, height: 72).accessibilityHidden(true)
                Text("Syncfusion Video Studio").font(.title.weight(.semibold))
                Text("Describe a video in Markdown, refine it with the Assistant, attach media by reference, then let the Assistant and cutline produce it.")
                    .foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                Spacer()
                OnboardingChecklist()
                Spacer()
                HStack {
                    Button { showNew = true } label: { Label("New Project…", systemImage: "plus") }.keyboardShortcut("n").controlSize(.large)
                    Button { AppDelegate.shared.openProject(nil) } label: { Label("Open…", systemImage: "folder") }.controlSize(.large)
                }
            }
            .padding(28)
            .frame(width: 400, alignment: .leading)
            Divider()
            VStack(alignment: .leading, spacing: 0) {
                Text("Recent Projects").font(.headline).padding(.horizontal, 16).padding(.top, 20).padding(.bottom, 8)
                if recents.isEmpty {
                    EmptyState(symbol: "film.stack", title: "No recent projects", message: "Projects you open appear here.")
                } else {
                    List(recents, id: \.self) { url in
                        Button { AppDelegate.shared.open(url) } label: {
                            HStack(spacing: 10) {
                                Image(systemName: "doc.richtext").font(.title3).foregroundStyle(Color.accentColor)
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(url.deletingPathExtension().lastPathComponent)
                                    Text(url.deletingLastPathComponent().path.replacingOccurrences(of: NSHomeDirectory(), with: "~")).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                                }
                                Spacer(minLength: 0)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                    .listStyle(.inset)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(nsColor: .controlBackgroundColor))
        }
        .frame(width: 760, height: 520)
        .sheet(isPresented: $showNew) { NewProjectSheet() }
        .onAppear { if startWithNewProject { showNew = true } }
    }
}

/// First-run checklist: the selected provider (Claude Code or Codex), cutline, the ElevenLabs key, a default brand preset, and
/// the default output folder.
struct OnboardingChecklist: View {
    @ObservedObject private var app = AppState.shared
    @State private var installing = false
    @State private var installMessage = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            Text("SETUP").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            // The provider chosen in Settings › Assistant (FEAT-spry-alpaca-5RQZ).
            if app.usesCodex {
                row(ok: app.codexStatus.map(\.ready),
                    title: "Codex",
                    detail: app.codexStatus.map { s in s.ready ? "Ready" + (s.plan.map { " (\($0) plan)" } ?? "") : "\(s.finding?.title ?? s.state): \(app.providerExecutable)" } ?? "Checking \(app.providerExecutable)…",
                    action: ("Settings…", { AppDelegate.shared.showSettings(pane: "codex") }))
            } else {
                row(ok: app.environment == nil ? nil : (app.claudeAvailable && app.environment?.signed_in != false),
                    title: "Claude Code",
                    detail: app.environment?.claude.map { c in !c.supported ? "Version \(c.version) is too old; \(app.environment?.min_claude_version ?? "") or later is needed" : (app.environment?.signed_in == false ? "Installed, but not signed in: run `claude` in Terminal" : c.version_text) } ?? "Required. Not found (looked for \(app.providerExecutable)).",
                    action: ("Settings…", { AppDelegate.shared.showSettings(pane: "claude") }))
            }
            row(ok: app.environment == nil ? nil : (app.environment?.cutline != nil && app.environment?.doctor?.ok == true),
                title: "cutline",
                detail: app.environment?.cutline.map { c in app.environment?.doctor?.ok == true ? "Version \(c.version)" : "Tools are missing: \((app.environment?.doctor?.missing_required ?? []).joined(separator: ", "))" } ?? "Required. Not found.",
                action: app.environment?.cutline != nil && app.environment?.doctor?.ok != true ? (installing ? "Installing…" : "Install Dependencies", installDeps) : ("Settings…", { AppDelegate.shared.showSettings(pane: "cutline") }))
            if !installMessage.isEmpty { Text(installMessage).font(.caption).foregroundStyle(.secondary).lineLimit(2) }
            row(ok: app.keyStatus.present, title: "ElevenLabs key", detail: app.keyStatus.present ? "Stored in the Keychain (\(app.keyStatus.masked ?? ""))" : "Optional: needed for generated voice-over and music", optional: true,
                action: ("Settings…", { AppDelegate.shared.showSettings(pane: "elevenlabs") }))
            row(ok: !app.brands.isEmpty, title: "Brand preset", detail: app.brands.first.map { "“\($0.name)”" + (app.brands.count > 1 ? " and \(app.brands.count - 1) more" : "") } ?? "Optional: logos, colors, fonts, voice and rules", optional: true,
                action: (app.brands.isEmpty ? "Create Default" : "Edit…", {
                    if app.brands.isEmpty { _ = try? Core.raw("brand.ensure_default"); app.reloadBrands() }
                    AppDelegate.shared.showLibrary(tab: .brands, select: nil)
                }))
            row(ok: true, title: "Output folder", detail: app.settings.default_output_folder.isEmpty ? "~/Movies/Syncfusion Video Studio" : app.settings.default_output_folder, action: ("Change…", {
                let panel = NSOpenPanel()
                panel.canChooseDirectories = true
                panel.canChooseFiles = false
                panel.canCreateDirectories = true
                if panel.runModal() == .OK, let url = panel.url {
                    app.settings.default_output_folder = url.path
                    app.saveSettings()
                }
            }))
        }
    }

    private func installDeps() {
        guard let cutline = app.environment?.cutline?.path, !installing else { return }
        installing = true
        installMessage = "Running `cutline deps install --all`. This downloads ffmpeg and models and can take a few minutes."
        Task {
            let status = await Task.detached { () -> Int32 in
                let p = Process()
                p.executableURL = URL(fileURLWithPath: cutline)
                p.arguments = ["deps", "install", "--all"]
                p.standardOutput = FileHandle.nullDevice
                p.standardError = FileHandle.nullDevice
                do { try p.run() } catch { return -1 }
                p.waitUntilExit()
                return p.terminationStatus
            }.value
            installing = false
            installMessage = status == 0 ? "Dependencies installed." : "The install didn't finish (exit \(status)). Run `cutline deps install --all` in Terminal to see why."
            app.detect(force: true)
        }
    }

    private func row(ok: Bool?, title: String, detail: String, optional: Bool = false, action: (String, () -> Void)) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Group {
                if let ok {
                    Image(systemName: ok ? "checkmark.circle.fill" : (optional ? "circle" : "exclamationmark.circle.fill")).foregroundStyle(ok ? Color.green : (optional ? Color.secondary : Color.orange))
                } else {
                    ProgressView().controlSize(.small)
                }
            }
            .frame(width: 18)
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(.callout.weight(.medium))
                Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(2).truncationMode(.middle)
            }
            Spacer(minLength: 4)
            Button(action.0, action: action.1).controlSize(.small).buttonStyle(.link)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(title): \(ok == true ? "ready" : ok == nil ? "checking" : (optional ? "not set" : "needs attention")). \(detail)")
    }
}

/// New Project: name, intent (which picks the spec template and suggested outputs), and where to
/// save. A project always has a file, so autosave and pasted images have somewhere to go.
struct NewProjectSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var app = AppState.shared
    @State private var name = ""
    @State private var intent = "marketing"
    @State private var intentOther = ""
    @State private var template = ""
    @State private var templates: [SpecTemplate] = []
    @State private var userTemplates: [URL] = []
    @State private var userTemplate: URL?

    private var effectiveTemplate: String { template.isEmpty ? (app.platforms.intents?[intent]?.template ?? "blank") : template }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New Project").font(.title3.weight(.semibold))
            Form {
                TextField("Name", text: $name, prompt: Text("Spring launch teaser"))
                Picker("Intent", selection: $intent) { ForEach(IntentKind.all) { Text($0.label).tag($0.key) } }
                    .onChange(of: intent) { template = ""; userTemplate = nil }
                if intent == "other" { TextField("Describe it", text: $intentOther) }
                Picker("Spec template", selection: Binding(get: { userTemplate?.path ?? effectiveTemplate }, set: { v in
                    if let u = userTemplates.first(where: { $0.path == v }) { userTemplate = u } else { userTemplate = nil; template = v }
                })) {
                    ForEach(templates) { Text($0.title).tag($0.id) }
                    if !userTemplates.isEmpty {
                        Divider()
                        ForEach(userTemplates, id: \.self) { Text($0.deletingPathExtension().lastPathComponent).tag($0.path) }
                    }
                }
            }
            .textFieldStyle(.roundedBorder)
            if let d = app.platforms.intents?[intent], userTemplate == nil {
                Text("Suggested outputs: " + d.outputs.compactMap { app.platforms.preset($0)?.name }.joined(separator: ", ") + ". You can change them later.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Create…") { create() }.keyboardShortcut(.defaultAction).disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(22)
        .frame(width: 460)
        .onAppear {
            templates = (try? Core.run("spec.templates")) ?? []
            let dir = app.libraryURL.appendingPathComponent("templates", isDirectory: true)
            userTemplates = ((try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? []).filter { $0.pathExtension == "cutproj" }.sorted { $0.lastPathComponent < $1.lastPathComponent }
        }
    }

    private func create() {
        let clean = name.trimmingCharacters(in: .whitespaces)
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.cutproj]
        panel.nameFieldStringValue = clean + ".cutproj"
        panel.title = "Save the New Project"
        panel.message = "The project is one file. Media stays where it is and is used by reference."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        dismiss()
        AppDelegate.shared.createProject(name: clean, intent: intent, intentOther: intent == "other" && !intentOther.isEmpty ? intentOther : nil, template: effectiveTemplate, templateFile: userTemplate, at: url)
    }
}
