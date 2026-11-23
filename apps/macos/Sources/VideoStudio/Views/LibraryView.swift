import SwiftUI
import AppKit
import UniformTypeIdentifiers

enum LibraryTab: String, CaseIterable { case brands = "Brand Presets", presets = "Output Presets" }

@MainActor
final class LibraryWindowState: ObservableObject {
    @Published var tab: LibraryTab = .brands
    @Published var selectedBrandID: String?
    @Published var selectedPresetID: String?
}

/// The app library window (⇧⌘L): brand presets and output config presets (spec §8, §9.1).
struct LibraryView: View {
    @ObservedObject var state: LibraryWindowState

    var body: some View {
        VStack(spacing: 0) {
            Picker("", selection: $state.tab) { ForEach(LibraryTab.allCases, id: \.self) { Text($0.rawValue).tag($0) } }
                .pickerStyle(.segmented).labelsHidden().frame(width: 320).padding(10)
                .accessibilityLabel("Library section")
            Divider()
            switch state.tab {
            case .brands: BrandLibrary(state: state)
            case .presets: PresetLibrary(state: state)
            }
        }
        .frame(minWidth: 820, minHeight: 540)
    }
}

// MARK: Brand presets

struct BrandLibrary: View {
    @ObservedObject var state: LibraryWindowState
    @ObservedObject private var app = AppState.shared

    var body: some View {
        HSplitView {
            VStack(spacing: 0) {
                List(app.brands, selection: $state.selectedBrandID) { b in
                    HStack(spacing: 8) {
                        HStack(spacing: 0) {
                            ForEach([b.colors.primary, b.colors.secondary, b.colors.accent], id: \.self) { c in Rectangle().fill(Color(hex: c) ?? .gray).frame(width: 7, height: 22) }
                        }
                        .clipShape(RoundedRectangle(cornerRadius: 3))
                        VStack(alignment: .leading, spacing: 1) {
                            Text(b.name)
                            if !b.description.isEmpty { Text(b.description).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                        }
                    }
                    .tag(b.id as String?)
                }
                Divider()
                HStack {
                    Button { newBrand() } label: { Image(systemName: "plus") }.help("New brand preset").accessibilityLabel("New brand preset")
                    Button { importBrand() } label: { Image(systemName: "square.and.arrow.down") }.help("Import a .cutbrand file").accessibilityLabel("Import brand")
                    Spacer()
                }
                .buttonStyle(.borderless).padding(.horizontal, 10).frame(height: 30)
            }
            .frame(minWidth: 200, idealWidth: 230, maxWidth: 300)
            Group {
                if let id = state.selectedBrandID, let brand = app.brands.first(where: { $0.id == id }) {
                    BrandEditor(brand: brand, state: state).id(id)
                } else {
                    EmptyState(symbol: "paintpalette", title: app.brands.isEmpty ? "No brand presets" : "Select a brand preset",
                               message: "A preset holds logos, colors, fonts, a voice and rules. Projects keep a snapshot, so they stay reproducible.",
                               action: app.brands.isEmpty ? ("New Brand Preset", newBrand) : nil)
                }
            }
            .frame(minWidth: 480, maxWidth: .infinity, maxHeight: .infinity)
        }
        .onAppear { if state.selectedBrandID == nil { state.selectedBrandID = app.brands.first?.id } }
    }

    private func newBrand() {
        guard let name = Prompt.text(title: "New Brand Preset", message: "", initial: ""), var b: BrandPreset = try? Core.run("brand.new", ["name": name]) else { return }
        b = (try? Core.run("brand.save", ["brand": Core.arg(b)])) ?? b
        app.reloadBrands()
        state.selectedBrandID = b.id
    }

    private func importBrand() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.cutbrand]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        AppDelegate.shared.importBrand(url)
    }
}

extension Color {
    init?(hex: String) {
        var h = hex.trimmingCharacters(in: .whitespaces)
        guard h.hasPrefix("#") else { return nil }
        h.removeFirst()
        if h.count == 3 { h = h.map { "\($0)\($0)" }.joined() }
        guard h.count >= 6, let v = UInt32(h.prefix(6), radix: 16) else { return nil }
        self.init(red: Double((v >> 16) & 255) / 255, green: Double((v >> 8) & 255) / 255, blue: Double(v & 255) / 255)
    }

    var hexString: String {
        let c = NSColor(self).usingColorSpace(.sRGB) ?? .black
        return String(format: "#%02X%02X%02X", Int(round(c.redComponent * 255)), Int(round(c.greenComponent * 255)), Int(round(c.blueComponent * 255)))
    }
}

struct BrandEditor: View {
    @State var brand: BrandPreset
    @ObservedObject var state: LibraryWindowState
    @ObservedObject private var app = AppState.shared
    @State private var saveWork: DispatchWorkItem?
    @State private var voiceSettings = ""

    private func changed() {
        saveWork?.cancel()
        let b = brand
        let work = DispatchWorkItem {
            _ = try? Core.raw("brand.save", ["brand": Core.arg(b)])
            AppState.shared.reloadBrands()
        }
        saveWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6, execute: work)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                InspectorSection(title: "Preset") {
                    TextField("Name", text: $brand.name)
                    TextField("Description", text: $brand.description)
                }
                InspectorSection(title: "Logos") {
                    asset("Primary", \.logos.primary, types: [.image])
                    asset("Horizontal", \.logos.horizontal, types: [.image])
                    asset("Icon", \.logos.icon, types: [.image])
                    asset("For light backgrounds", \.logos.light, types: [.image])
                    asset("For dark backgrounds", \.logos.dark, types: [.image])
                }
                InspectorSection(title: "Colors") {
                    color("Primary", \.colors.primary)
                    color("Secondary", \.colors.secondary)
                    color("Accent", \.colors.accent)
                    color("Background", \.colors.background)
                    color("Text", \.colors.text)
                }
                InspectorSection(title: "Fonts") {
                    font("Heading", \.fonts.heading)
                    font("Body", \.fonts.body)
                }
                InspectorSection(title: "Bumpers and watermark") {
                    asset("Intro bumper", \.intro, types: [.movie, .image])
                    asset("Outro bumper", \.outro, types: [.movie, .image])
                    asset("Watermark", \.watermark.asset, types: [.image])
                    Picker("Watermark position", selection: $brand.watermark.position) {
                        Text("Top left").tag("top_left"); Text("Top right").tag("top_right"); Text("Bottom left").tag("bottom_left"); Text("Bottom right").tag("bottom_right")
                    }
                    Field("Watermark opacity") {
                        HStack {
                            Slider(value: $brand.watermark.opacity, in: 0.1...1).frame(width: 160).accessibilityLabel("Watermark opacity")
                            Text("\(Int(brand.watermark.opacity * 100))%").monospacedDigit().frame(width: 40, alignment: .trailing)
                        }
                    }
                }
                InspectorSection(title: "Style") {
                    TextField("Lower-third style", text: $brand.lower_third_style, prompt: Text("Name over role, left aligned, accent bar"))
                    TextField("Caption style", text: $brand.caption_style, prompt: Text("Sentence captions, bottom third, high contrast"))
                    TextField("Music mood", text: $brand.music_mood, prompt: Text("Light, optimistic, unobtrusive"))
                }
                InspectorSection(title: "Voice") {
                    TextField("Voice ID", text: $brand.voice.voice_id)
                    TextField("Cartesia voice ID", text: Binding(get: { brand.voice.cartesia_voice_id ?? "" }, set: { brand.voice.cartesia_voice_id = $0.isEmpty ? nil : $0 }), prompt: Text("Used when Settings has Cartesia speak"))
                    TextField("Voice name", text: Binding(get: { brand.voice.name ?? "" }, set: { brand.voice.name = $0.isEmpty ? nil : $0 }), prompt: Text("For your reference"))
                    TextField("Model", text: Binding(get: { brand.voice.model ?? "" }, set: { brand.voice.model = $0.isEmpty ? nil : $0 }), prompt: Text("eleven_v4"))
                    if let m = brand.voice.model, !m.isEmpty, !Self.performsTags(m) {
                        Text("This model reads delivery tags such as [warmly] aloud, so runs leave them out. Eleven v3 and later (eleven_v4 is the default) perform them.").font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                    TextField("Settings (JSON)", text: $voiceSettings, prompt: Text("{\"stability\": 0.4, \"similarity_boost\": 0.8}"))
                        .font(.system(.body, design: .monospaced))
                        .onChange(of: voiceSettings) {
                            if voiceSettings.trimmingCharacters(in: .whitespaces).isEmpty { brand.voice.settings = nil }
                            else if let v = Core.decode(JSONValue.self, from: voiceSettings), v.objectValue != nil { brand.voice.settings = v }
                        }
                    if !voiceSettings.isEmpty && Core.decode(JSONValue.self, from: voiceSettings)?.objectValue == nil {
                        Text("Not valid JSON yet; the last valid settings are kept.").font(.caption).foregroundStyle(.orange)
                    }
                }
                InspectorSection(title: "Rules") {
                    Text("Do's and don'ts in your own words. The Assistant is given them verbatim.").font(.caption).foregroundStyle(.secondary)
                    TextEditor(text: $brand.rules).font(.body).frame(minHeight: 110).overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color(nsColor: .separatorColor)))
                        .accessibilityLabel("Brand rules")
                }
                InspectorSection(title: "Instructions") {
                    Text("Labelled context for one product line or audience, such as “Bold Reports” or “Syncfusion WPF”. Each project chooses the ones it uses from the brand menu in its toolbar; the Assistant is given those verbatim when writing the spec and making the video. Rules above apply to every project.")
                        .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    ForEach(instructions) { $set in
                        VStack(alignment: .leading, spacing: 6) {
                            HStack {
                                TextField("Label", text: $set.label, prompt: Text("Bold Reports"))
                                    .accessibilityLabel("Instructions label")
                                Button(role: .destructive) {
                                    guard Prompt.confirm("Delete the instructions “\(set.label)”?", message: "Projects that already use them keep them in their snapshot until they update the brand.", ok: "Delete", destructive: true) else { return }
                                    instructions.wrappedValue.removeAll { $0.id == set.id }
                                } label: { Image(systemName: "trash") }
                                .buttonStyle(.borderless)
                                .help("Delete these instructions")
                                .accessibilityLabel("Delete instructions “\(set.label)”")
                            }
                            TextEditor(text: $set.text).font(.body).frame(minHeight: 90).overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color(nsColor: .separatorColor)))
                                .accessibilityLabel("Instructions for \(set.label)")
                        }
                        .padding(.bottom, 6)
                    }
                    Button("Add Instructions") { instructions.wrappedValue.append(.new(label: "")) }
                        .help("Add a labelled set of instructions projects can choose")
                }
                HStack {
                    Button("Export…") { export() }.help("Save as a .cutbrand file (the preset and its assets) to share between machines")
                    Button("Duplicate") {
                        var copy = brand
                        copy.id = ""
                        copy.name = brand.name + " copy"
                        if let saved: BrandPreset = try? Core.run("brand.save", ["brand": Core.arg(copy)]) {
                            app.reloadBrands()
                            state.selectedBrandID = saved.id
                        }
                    }
                    Spacer()
                    Button("Delete Preset", role: .destructive) {
                        guard Prompt.confirm("Delete the brand preset “\(brand.name)”?", message: "Projects that use it keep their snapshot.", ok: "Delete", destructive: true) else { return }
                        saveWork?.cancel()
                        _ = try? Core.raw("brand.delete", ["id": brand.id])
                        app.reloadBrands()
                        state.selectedBrandID = app.brands.first?.id
                    }
                }
            }
            .textFieldStyle(.roundedBorder)
            .environment(\.fieldLabelWidth, 170)
            .padding(20)
            .frame(maxWidth: 640, alignment: .leading)
        }
        .onChange(of: brand) { changed() }
        .onAppear {
            if let s = brand.voice.settings { voiceSettings = Core.encode(s) }
        }
        .onDisappear {
            if let w = saveWork, !w.isCancelled { w.perform() }
        }
    }

    /// Eleven v3 and later perform `[audio tags]` (the core's `eleven_performs_tags`).
    static func performsTags(_ model: String) -> Bool {
        guard model.hasPrefix("eleven_v"), let n = Int(model.dropFirst("eleven_v".count).prefix { $0.isNumber }) else { return false }
        return n >= 3
    }

    /// The brand's instruction sets; an empty list is stored as none.
    private var instructions: Binding<[InstructionSet]> {
        Binding(get: { brand.instructions ?? [] }, set: { brand.instructions = $0.isEmpty ? nil : $0 })
    }

    private func export() {
        saveWork?.perform()
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.cutbrand]
        panel.nameFieldStringValue = brand.name + ".cutbrand"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do { _ = try Core.raw("brand.export", ["brand": Core.arg(brand), "destination": url.path]) } catch { Prompt.error("The brand couldn't be exported.", detail: error.localizedDescription) }
    }

    private func pick(_ types: [UTType]) -> AssetRef? {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = types
        panel.message = "The file is referenced where it is, like media."
        guard panel.runModal() == .OK, let url = panel.url else { return nil }
        return try? Core.run("brand.asset", ["path": url.path])
    }

    private func asset(_ label: String, _ keyPath: WritableKeyPath<BrandPreset, AssetRef?>, types: [UTType]) -> some View {
        Field(label) {
            HStack(spacing: 6) {
                if let a = brand[keyPath: keyPath] {
                    let exists = FileManager.default.fileExists(atPath: (a.path as NSString).expandingTildeInPath)
                    if !exists { Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.red).help("This file wasn't found. Choose it again.") }
                    Text((a.path as NSString).lastPathComponent).lineLimit(1).truncationMode(.middle).foregroundStyle(exists ? Color.primary : Color.red).help(a.path)
                    Button { brand[keyPath: keyPath] = nil } label: { Image(systemName: "xmark.circle.fill") }.buttonStyle(.borderless).accessibilityLabel("Remove \(label)")
                } else {
                    Text("None").foregroundStyle(.secondary)
                }
                Button("Choose…") { if let a = pick(types) { brand[keyPath: keyPath] = a } }.controlSize(.small)
            }
        }
    }

    private func color(_ label: String, _ keyPath: WritableKeyPath<BrandPreset, String>) -> some View {
        Field(label) {
            HStack(spacing: 8) {
                ColorPicker("", selection: Binding(get: { Color(hex: brand[keyPath: keyPath]) ?? .gray }, set: { brand[keyPath: keyPath] = $0.hexString }), supportsOpacity: false).labelsHidden()
                    .accessibilityLabel("\(label) color")
                TextField("#RRGGBB", text: Binding(get: { brand[keyPath: keyPath] }, set: { brand[keyPath: keyPath] = $0 })).frame(width: 96).font(.system(.body, design: .monospaced))
                if Color(hex: brand[keyPath: keyPath]) == nil { Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.red).help("Use #RRGGBB") }
            }
        }
    }

    private func font(_ label: String, _ keyPath: WritableKeyPath<BrandPreset, FontRef>) -> some View {
        Field(label) {
            HStack(spacing: 6) {
                TextField("Installed family name", text: Binding(get: { brand[keyPath: keyPath].family ?? "" }, set: { brand[keyPath: keyPath].family = $0.isEmpty ? nil : $0 })).frame(width: 190)
                if let f = brand[keyPath: keyPath].file {
                    Text((f.path as NSString).lastPathComponent).lineLimit(1).truncationMode(.middle).help(f.path)
                    Button { brand[keyPath: keyPath].file = nil } label: { Image(systemName: "xmark.circle.fill") }.buttonStyle(.borderless).accessibilityLabel("Remove \(label) font file")
                }
                Button("File…") { if let a = pick([.font]) { brand[keyPath: keyPath].file = a } }.controlSize(.small)
            }
        }
    }
}

// MARK: Output presets

struct PresetLibrary: View {
    @ObservedObject var state: LibraryWindowState
    @ObservedObject private var app = AppState.shared

    var body: some View {
        HSplitView {
            VStack(spacing: 0) {
                List(selection: $state.selectedPresetID) {
                    Section("Built in") {
                        ForEach(app.platforms.presets.filter { $0.user != true }) { p in Text(p.name).tag(p.id as String?) }
                    }
                    let mine = app.platforms.presets.filter { $0.user == true }
                    if !mine.isEmpty {
                        Section("Yours") { ForEach(mine) { p in Text(p.name).tag(p.id as String?) } }
                    }
                }
                Divider()
                Text("Limits are data: platforms.json, updated \(app.platforms.updated ?? "with the app").").font(.caption2).foregroundStyle(.secondary).padding(8).frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(minWidth: 200, idealWidth: 230, maxWidth: 300)
            Group {
                if let id = state.selectedPresetID, let preset = app.platforms.preset(id) {
                    PresetDetail(preset: preset, state: state).id(id)
                } else {
                    EmptyState(symbol: "rectangle.on.rectangle", title: "Select an output preset", message: "Each preset maps a platform to a cutline target and export settings. Duplicate one to make your own.")
                }
            }
            .frame(minWidth: 480, maxWidth: .infinity, maxHeight: .infinity)
        }
        .onAppear { if state.selectedPresetID == nil { state.selectedPresetID = app.platforms.presets.first?.id } }
    }
}

struct PresetDetail: View {
    @State var preset: OutputPreset
    @ObservedObject var state: LibraryWindowState
    @ObservedObject private var app = AppState.shared

    private var editable: Bool { preset.user == true }

    private func save() {
        guard editable else { return }
        _ = try? Core.raw("preset.save", ["preset": Core.arg(preset)])
        app.reloadPlatforms()
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(preset.name).font(.title3.weight(.semibold))
                        Text(preset.platform + (editable ? " · your preset" : " · built in")).font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("Duplicate…") {
                        guard let name = Prompt.text(title: "Duplicate Output Preset", message: "", initial: preset.name + " copy"),
                              let p: OutputPreset = try? Core.run("preset.duplicate", ["id": preset.id, "name": name]) else { return }
                        app.reloadPlatforms()
                        state.selectedPresetID = p.id
                    }
                    if editable {
                        Button("Delete", role: .destructive) {
                            guard Prompt.confirm("Delete the output preset “\(preset.name)”?", message: "Outputs that use it will report a missing preset.", ok: "Delete", destructive: true) else { return }
                            _ = try? Core.raw("preset.delete", ["id": preset.id])
                            app.reloadPlatforms()
                            state.selectedPresetID = app.platforms.presets.first?.id
                        }
                    }
                }
                if !editable { Text("Built-in presets can't be changed. Duplicate this one to make your own.").font(.callout).foregroundStyle(.secondary) }
                Form {
                    TextField("Name", text: $preset.name)
                    TextField("Platform", text: $preset.platform)
                    LabeledContent("Shapes") {
                        VStack(alignment: .leading, spacing: 2) {
                            ForEach(["long_1080p", "long_4k", "shorts", "square", "portrait_4x5", "custom"], id: \.self) { t in
                                Toggle(OutputEditor.targetLabel(t), isOn: Binding(get: { preset.targets.contains(t) }, set: { on in
                                    if on { if !preset.targets.contains(t) { preset.targets.append(t) } } else if preset.targets.count > 1 { preset.targets.removeAll { $0 == t } }
                                }))
                                .toggleStyle(.checkbox)
                            }
                            Text("The first shape is the default: \(OutputEditor.targetLabel(preset.targets.first ?? ""))").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    Picker("Captions", selection: $preset.captions) {
                        Text("Burned in").tag("burn"); Text("Sidecar files only").tag("sidecar"); Text("None").tag("none")
                    }
                    Toggle("Audio", isOn: $preset.audio)
                    Toggle("Seamless loop", isOn: $preset.loop)
                    LabeledContent("Min length (s)") { TextField("none", value: $preset.limits.min_duration_s, format: .number).frame(width: 90) }
                    LabeledContent("Max length (s)") { TextField("none", value: $preset.limits.max_duration_s, format: .number).frame(width: 90) }
                    LabeledContent("Platform size limit (MB)") {
                        TextField("none", value: Binding(get: { preset.limits.max_bytes.map { Double($0) / 1_000_000 } }, set: { (v: Double?) in preset.limits.max_bytes = (v ?? 0) > 0 ? UInt64(v! * 1_000_000) : nil }), format: .number).frame(width: 90)
                    }
                    LabeledContent("Max frame rate") { TextField("none", value: $preset.limits.max_fps, format: .number).frame(width: 90) }
                    TextField("Notes", text: $preset.notes, axis: .vertical).lineLimit(2...4)
                }
                .textFieldStyle(.roundedBorder)
                .disabled(!editable)
            }
            .padding(20)
            .frame(maxWidth: 620, alignment: .leading)
        }
        .onChange(of: preset) { save() }
    }
}
