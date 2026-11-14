import AppKit
import Combine
import MarkdownCore

enum InspectorTab: String, CaseIterable, Identifiable {
    case claude, project, outputs, validation, run
    var id: String { rawValue }
    var title: String {
        switch self {
        case .claude: return "Claude"
        case .project: return "Project"
        case .outputs: return "Outputs"
        case .validation: return "Validation"
        case .run: return "Run"
        }
    }
    var symbol: String {
        switch self {
        case .claude: return "sparkles"
        case .project: return "doc.text"
        case .outputs: return "rectangle.on.rectangle"
        case .validation: return "checkmark.shield"
        case .run: return "play.circle"
        }
    }
}

enum SidebarTab: String, CaseIterable, Identifiable {
    case outline, media, runs
    var id: String { rawValue }
    var title: String { rawValue.capitalized }
    var symbol: String {
        switch self {
        case .outline: return "list.bullet.indent"
        case .media: return "photo.on.rectangle"
        case .runs: return "clock.arrow.circlepath"
        }
    }
}

/// A message across the top of the window that needs a decision.
enum Banner: Equatable {
    /// The file changed on disk while there are unsaved edits (spec §16).
    case externalChange
    /// The brand preset in the library is newer than the project's snapshot (spec §8).
    case brandUpdate(name: String)
    case error(String)
}

/// One open project: the `.cutproj` contents, the spec editor, and everything derived from them.
@MainActor
final class ProjectDocument: ObservableObject, Identifiable {
    let id = UUID()
    @Published private(set) var project: Project
    @Published private(set) var fileURL: URL
    @Published private(set) var isDirty = false
    @Published private(set) var resolutions: [Resolution] = []
    @Published private(set) var outline: [OutlineItem] = []
    @Published private(set) var lint: [LintIssue] = []
    @Published private(set) var resolvedOutputs: [ResolvedOutput] = []
    @Published private(set) var report: ValidationReport?
    @Published private(set) var isValidating = false
    @Published var banner: Banner?
    @Published var inspectorTab: InspectorTab = .claude
    @Published var sidebarTab: SidebarTab = .outline
    @Published var showPreview = false { didSet { if showPreview != oldValue { previewChanged() } } }
    @Published var showFindBar = false
    @Published var findShowsReplace = false
    @Published var selectedRunConfigID: String? { didSet { if selectedRunConfigID != oldValue { refreshOutputs() } } }
    @Published var selectedRunID: String?
    @Published var selectedMediaID: String?
    @Published var showRelinkSheet = false
    /// Warnings acknowledged for the next run (issue IDs).
    @Published var acknowledged: Set<String> = []

    private(set) lazy var editor: EditorController = {
        let e = EditorController(document: self, text: project.spec)
        e.observeUndo()
        return e
    }()
    private(set) lazy var preview = PreviewController(document: self)
    private(set) lazy var runs = RunController(document: self)
    private(set) lazy var assistant = AssistantModel(document: self)
    private(set) lazy var review = SpecReview(document: self)
    weak var windowController: ProjectWindowController?

    /// The project as last read from or written to disk, to tell our edits from external ones.
    private var base: Project
    private var autosaveWork: DispatchWorkItem?
    private var deriveWork: DispatchWorkItem?
    private var watcher: DispatchSourceFileSystemObject?
    private var lastWrite = Date.distantPast
    private var cancellables = Set<AnyCancellable>()
    /// Editor position to apply once the text view is on screen.
    var pendingSelection: Int?
    var pendingScrollLine: Int?

    init(project: Project, url: URL) {
        self.project = project
        self.base = project
        self.fileURL = url
        selectedRunConfigID = project.run_configs.first?.id
        selectedRunID = project.runs.last?.id
        refreshDerived(now: true)
        refreshOutputs()
        startWatching()
        AppState.shared.rememberRunFolders(project.runs.filter { $0.kind == "production" }.map(\.folder))
        checkBrandUpdate()
    }

    deinit { watcher?.cancel() }

    static func open(_ url: URL) throws -> ProjectDocument {
        let p: Project = try Core.run("project.load", ["path": url.path])
        return ProjectDocument(project: p, url: url)
    }

    var displayName: String { project.name.isEmpty ? fileURL.deletingPathExtension().lastPathComponent : project.name }
    var selectedRunConfig: RunConfig? { project.runConfig(selectedRunConfigID) ?? project.run_configs.first }
    var missingMedia: [Resolution] { resolutions.filter { $0.status == "missing" } }
    var assistantLocked: Bool { review.pending != nil }

    // MARK: Editing

    /// Changes the project and schedules an autosave. Use for everything but the spec text, which
    /// the editor owns.
    func update(_ change: (inout Project) -> Void) {
        var p = project
        change(&p)
        guard p != project else { return }
        project = p
        markDirty()
        refreshOutputs()
        scheduleDerived()
    }

    /// Called by the editor after every text change.
    func textDidChange() {
        let text = editor.text
        guard text != project.spec else { return }
        project.spec = text
        markDirty()
        scheduleDerived()
        if showPreview { preview.scheduleRender() }
    }

    private func markDirty() {
        isDirty = true
        windowController?.window?.isDocumentEdited = true
        autosaveWork?.cancel()
        // Autosave runs one second after editing stops (spec §16).
        let work = DispatchWorkItem { [weak self] in self?.save() }
        autosaveWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 1, execute: work)
    }

    private func scheduleDerived() {
        deriveWork?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.refreshDerived(now: false) }
        deriveWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3, execute: work)
    }

    /// Outline and lint come from the core, so the CLI and the app agree on both.
    private func refreshDerived(now: Bool) {
        let snapshot = project
        let apply: ([OutlineItem], [LintIssue]) -> Void = { [weak self] outline, lint in
            guard let self else { return }
            if self.outline != outline { self.outline = outline }
            if self.lint != lint { self.lint = lint }
        }
        if now {
            apply((try? Core.run("spec.outline", ["spec": snapshot.spec])) ?? [], (try? Core.run("spec.lint", ["project": Core.arg(snapshot)])) ?? [])
            return
        }
        Task {
            let outline: [OutlineItem] = (try? await Core.async("spec.outline", ["spec": snapshot.spec])) ?? []
            let lint: [LintIssue] = (try? await Core.async("spec.lint", ["project": Core.arg(snapshot)])) ?? []
            apply(outline, lint)
        }
    }

    func refreshOutputs() {
        let snapshot = project
        Task {
            let all: [ResolvedOutput] = (try? await Core.async("outputs.resolve", ["project": Core.arg(snapshot)])) ?? []
            if self.resolvedOutputs != all { self.resolvedOutputs = all }
        }
    }

    func resolvedOutput(_ id: String) -> ResolvedOutput? { resolvedOutputs.first { $0.id == id } }

    // MARK: Saving

    @discardableResult
    func save() -> Bool {
        autosaveWork?.cancel()
        guard isDirty else { return true }
        // Run history written by a run or the CLI since our last read must not be lost.
        mergeRunsFromDisk()
        do {
            lastWrite = Date()
            let saved: Project = try Core.run("project.save", ["project": Core.arg(project), "path": fileURL.path])
            project.modified = saved.modified
            base = saved
            isDirty = false
            windowController?.window?.isDocumentEdited = false
            return true
        } catch {
            banner = .error("Couldn't save: \(error.localizedDescription)")
            return false
        }
    }

    func saveAs(_ url: URL) -> Bool {
        do {
            let saved: Project = try Core.run("project.save_as", ["project": Core.arg(project), "old_path": fileURL.path, "path": url.path])
            watcher?.cancel()
            project = saved
            base = saved
            fileURL = url
            isDirty = false
            windowController?.window?.isDocumentEdited = false
            windowController?.updateTitle()
            startWatching()
            resolveMedia()
            return true
        } catch {
            Prompt.error("Couldn't save the project.", detail: error.localizedDescription)
            return false
        }
    }

    private func mergeRunsFromDisk() {
        guard let r: MergeRunsResult = try? Core.run("project.merge_runs", ["project": Core.arg(project), "path": fileURL.path]), r.changed else { return }
        project.runs = r.project.runs
    }

    // MARK: External changes (spec §16)

    private func startWatching() {
        watcher?.cancel()
        let fd = Darwin.open(fileURL.path, O_EVTONLY)
        guard fd >= 0 else { return }
        // Atomic saves replace the file, so the source is rebuilt after each event.
        let source = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .delete, .rename, .extend], queue: .main)
        source.setEventHandler { [weak self] in
            guard let self else { return }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
                self.startWatching()
                self.fileChangedOnDisk()
            }
        }
        source.setCancelHandler { Darwin.close(fd) }
        source.resume()
        watcher = source
    }

    private func fileChangedOnDisk() {
        // Our own save triggers the watcher too.
        guard Date().timeIntervalSince(lastWrite) > 0.6, FileManager.default.fileExists(atPath: fileURL.path) else { return }
        guard let r: ExternalChangeResult = try? Core.run("project.external_change", ["base": Core.arg(base), "path": fileURL.path]) else { return }
        switch r.change {
        case "runs_only":
            // The CLI or a run appended history: merge silently.
            project.runs = mergedRuns(mine: project.runs, disk: r.disk.runs)
            base.runs = r.disk.runs
            if selectedRunID == nil { selectedRunID = project.runs.last?.id }
            AppState.shared.rememberRunFolders(project.runs.filter { $0.kind == "production" }.map(\.folder))
        case "other":
            if isDirty {
                banner = .externalChange
            } else {
                reloadFromDisk(r.disk)
            }
        default:
            break
        }
    }

    private func mergedRuns(mine: [RunRecord], disk: [RunRecord]) -> [RunRecord] {
        var out = mine
        for r in disk {
            if let i = out.firstIndex(where: { $0.id == r.id }) {
                if out[i].status == "running" || r.finished != nil { out[i] = r }
            } else {
                out.append(r)
            }
        }
        return out.sorted { $0.started < $1.started }
    }

    /// Reload: take the file on disk, dropping unsaved edits.
    func reloadFromDisk(_ disk: Project? = nil) {
        guard let p = disk ?? (try? Core.run("project.load", ["path": fileURL.path], as: Project.self)) else { return }
        autosaveWork?.cancel()
        project = p
        base = p
        isDirty = false
        windowController?.window?.isDocumentEdited = false
        if editor.text != p.spec { editor.replaceAllText(p.spec, undoable: false) }
        banner = nil
        refreshDerived(now: true)
        refreshOutputs()
        resolveMedia()
    }

    /// Keep Mine: overwrite the file with the version in the window.
    func keepMine() {
        banner = nil
        if let disk = try? Core.run("project.load", ["path": fileURL.path], as: Project.self) {
            project.runs = mergedRuns(mine: project.runs, disk: disk.runs)
        }
        isDirty = true
        save()
    }

    /// Compare: writes both versions of the spec to temporary files and opens them in FileMerge
    /// (or reveals them when FileMerge isn't installed).
    func compareWithDisk() {
        guard let disk = try? Core.run("project.load", ["path": fileURL.path], as: Project.self) else { return }
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("video-studio-compare-\(UUID().uuidString.prefix(8))", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let mine = dir.appendingPathComponent("spec (this window).md"), theirs = dir.appendingPathComponent("spec (on disk).md")
        try? project.spec.write(to: mine, atomically: true, encoding: .utf8)
        try? disk.spec.write(to: theirs, atomically: true, encoding: .utf8)
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/opendiff")
        p.arguments = [theirs.path, mine.path]
        if (try? p.run()) == nil { NSWorkspace.shared.activateFileViewerSelecting([mine, theirs]) }
    }

    // MARK: Brand

    private func checkBrandUpdate() {
        guard let b = project.brand else { return }
        Task {
            let newer: Bool = (try? await Core.async("brand.is_newer", ["brand": Core.arg(b)])) ?? false
            if newer, self.banner == nil { self.banner = .brandUpdate(name: b.snapshot.name) }
        }
    }

    /// Switches the project to a preset: its ID and a snapshot are stored (spec §8).
    func setBrand(_ preset: BrandPreset?) {
        update { $0.brand = preset.map { BrandRef(preset_id: $0.id, snapshot: $0) } }
        if case .brandUpdate = banner { banner = nil }
    }

    func updateBrandToLatest() {
        guard let id = project.brand?.preset_id, let latest = AppState.shared.brands.first(where: { $0.id == id }) else { banner = nil; return }
        setBrand(latest)
    }

    // MARK: Media

    func mediaURL(_ id: String) -> URL? {
        guard let p = resolutions.first(where: { $0.id == id })?.path else { return nil }
        return URL(fileURLWithPath: p)
    }

    func mediaStatus(_ id: String) -> String { resolutions.first { $0.id == id }?.status ?? (project.mediaItem(id) == nil ? "unknown" : "missing") }

    /// Resolves every item, applies silent path updates, and probes what hasn't been probed.
    func resolveMedia(showRelink: Bool = false) {
        let snapshot = project
        let path = fileURL.path
        Task {
            guard let r: MediaResolveResult = try? await Core.async("media.resolve", ["project": Core.arg(snapshot), "path": path]) else { return }
            // Media changed while this was running (a relink, an add): the answer is about an
            // older list, so ask again rather than apply it.
            guard self.project.media == snapshot.media else {
                self.resolveMedia(showRelink: showRelink)
                return
            }
            self.resolutions = r.resolutions
            if r.dirty { self.update { $0.media = r.project.media } }
            if showRelink, !self.missingMedia.isEmpty { self.showRelinkSheet = true }
            if self.showPreview { self.preview.scheduleRender() }
            self.editor.mediaChanged()
            let needsProbe = self.project.media.contains { $0.probe == nil && ["video", "audio", "image"].contains($0.kind) }
            guard needsProbe, AppState.shared.cutlineAvailable else { return }
            let current = self.project
            if let probed: ProbeResult = try? await Core.async("media.probe", ["project": Core.arg(current), "resolutions": Core.arg(r.resolutions)]), probed.changed {
                self.mergeProbes(from: probed.project)
            }
        }
    }

    /// Takes probe results computed from a snapshot. Only the probe is copied, and only for items
    /// that still point at the same file, so edits made meanwhile are never overwritten.
    private func mergeProbes(from other: Project) {
        update { p in
            for item in other.media {
                guard let i = p.media.firstIndex(where: { $0.id == item.id }), p.media[i].paths.absolute == item.paths.absolute, p.media[i].fingerprint == item.fingerprint else { continue }
                p.media[i].probe = item.probe
            }
        }
    }

    /// Adds files or folders by reference and returns the new IDs.
    @discardableResult
    func addMedia(_ urls: [URL]) -> [String] {
        guard !urls.isEmpty else { return [] }
        guard let r: MediaAddResult = try? Core.run("media.add", ["project": Core.arg(project), "path": fileURL.path, "files": urls.map(\.path)]) else { return [] }
        update { $0.media = r.project.media }
        if let first = r.errors.first { banner = .error(first) }
        resolveMedia()
        return r.ids
    }

    /// Stores a pasted image in the sidecar and references it (spec §5). Returns its media ID.
    func addPastedImage(_ data: Data) -> String? {
        guard save() else { return nil }
        guard let path = try? storePastedImage(projectPath: fileURL.path, image: data) else { return nil }
        guard let r: MediaAddResult = try? Core.run("media.add", ["project": Core.arg(project), "path": fileURL.path, "files": [path]]), let id = r.ids.first else { return nil }
        update { p in
            p.media = r.project.media
            if let i = p.media.firstIndex(where: { $0.id == id }) { p.media[i].pasted = true }
        }
        resolveMedia()
        return id
    }

    func removeMedia(_ id: String) {
        update { $0.media.removeAll { $0.id == id } }
        resolutions.removeAll { $0.id == id }
    }

    func relink(_ id: String, to url: URL) -> RelinkResult? {
        guard let r: RelinkResult = try? Core.run("media.relink", ["project": Core.arg(project), "path": fileURL.path, "id": id, "file": url.path]) else { return nil }
        update { $0.media = r.project.media }
        resolveMedia()
        return r
    }

    func applyPrefixFix(_ fix: PrefixFix) -> [String] {
        guard let r: RelinkedResult = try? Core.run("media.apply_prefix_fix", ["project": Core.arg(project), "path": fileURL.path, "fix": Core.arg(fix)]) else { return [] }
        update { $0.media = r.project.media }
        resolveMedia()
        return r.relinked
    }

    func searchFolder(_ url: URL) -> [String] {
        guard let r: RelinkedResult = try? Core.run("media.search_folder", ["project": Core.arg(project), "path": fileURL.path, "folder": url.path]) else { return [] }
        update { $0.media = r.project.media }
        resolveMedia()
        return r.relinked
    }

    func acceptChanged(_ id: String) {
        guard let path = resolutions.first(where: { $0.id == id })?.path,
              let p: Project = try? Core.run("media.accept_changed", ["project": Core.arg(project), "path": fileURL.path, "id": id, "file": path]) else { return }
        update { $0.media = p.media }
        resolveMedia()
    }

    // MARK: Validation (spec §11)

    /// Runs the static checks for the selected run config and shows the report.
    @discardableResult
    func validate(kind: String = "production", readiness: ReadinessReport? = nil, show: Bool = true) async -> ValidationReport? {
        guard !isValidating else { return report }
        isValidating = true
        defer { isValidating = false }
        if show { inspectorTab = .validation }
        save()
        let app = AppState.shared
        var args: Core.Args = ["project": Core.arg(project), "path": fileURL.path, "kind": kind, "run_config": selectedRunConfig?.id,
                               "eleven_key_present": app.keyStatus.present, "eleven_remaining": app.keyStatus.remaining.map { NSNumber(value: $0) }]
        if let env = app.environment { args["environment"] = Core.arg(env) }
        if let readiness { args["readiness"] = Core.arg(readiness) }
        guard let r: ValidateResult = try? await Core.async("validate", args) else {
            banner = .error("Validation couldn't run.")
            return nil
        }
        report = r.report
        resolutions = r.resolutions
        // Probe results are a cache worth keeping.
        mergeProbes(from: r.project)
        // Acknowledgements only hold for issues that are still there.
        acknowledged = acknowledged.intersection(r.report.issues.map(\.id))
        return r.report
    }

    /// Adds the dry run's findings to the report (spec §11.2).
    func addDryRunFindings(_ readiness: ReadinessReport) {
        Task { await validate(kind: "production", readiness: readiness, show: true) }
    }

    /// Goes to the place an issue can be fixed.
    func reveal(_ fix: FixLink?) {
        guard let fix else { return }
        switch fix.kind {
        case "spec":
            showPreview = false
            jump(toLine: fix.line ?? 1)
        case "media":
            sidebarTab = .media
            selectedMediaID = fix.target
            if let t = fix.target, mediaStatus(t) == "missing" { showRelinkSheet = true }
        case "brand":
            AppDelegate.shared.showLibrary(tab: .brands, select: project.brand?.preset_id)
        case "outputs", "run_config":
            inspectorTab = .outputs
        case "project":
            inspectorTab = .project
        case "settings":
            AppDelegate.shared.showSettings(pane: fix.target)
        default:
            break
        }
    }

    func jump(toLine line: Int) {
        if showPreview { showPreview = false }
        editor.goToLine(line)
        windowController?.window?.makeFirstResponder(editor.textView)
    }

    private func previewChanged() {
        if showPreview { preview.scheduleRender(immediate: true) }
        windowController?.modeChanged()
    }

    // MARK: Run configs and outputs

    func addRunConfig(named name: String) {
        let app = AppState.shared
        var rc = RunConfig(id: "rc_" + UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(16).lowercased(), name: name)
        rc.production_model = app.settings.default_production_model
        rc.eleven_credit_cap = app.settings.eleven_default_credit_cap
        rc.quality = app.settings.default_quality
        rc.web_access = app.settings.web_access_default
        rc.outputs = selectedRunConfig?.outputs ?? project.output_configs.map(\.id)
        update { $0.run_configs.append(rc) }
        selectedRunConfigID = rc.id
    }

    func updateRunConfig(_ id: String, _ change: (inout RunConfig) -> Void) {
        update { p in
            if let i = p.run_configs.firstIndex(where: { $0.id == id }) { change(&p.run_configs[i]) }
        }
    }

    func deleteRunConfig(_ id: String) {
        update { $0.run_configs.removeAll { $0.id == id } }
        if selectedRunConfigID == id { selectedRunConfigID = project.run_configs.first?.id }
    }

    /// Adds an output config from a preset and selects it in the current run config.
    func addOutput(preset: OutputPreset) {
        var id = "out_" + preset.id
        var n = 2
        while project.output_configs.contains(where: { $0.id == id }) { id = "out_\(preset.id)_\(n)"; n += 1 }
        let rcID = selectedRunConfig?.id
        update { p in
            p.output_configs.append(OutputConfig(id: id, preset: preset.id))
            if let i = p.run_configs.firstIndex(where: { $0.id == rcID }) { p.run_configs[i].outputs.append(id) }
        }
    }

    func updateOutput(_ id: String, _ change: (inout OutputConfig) -> Void) {
        update { p in
            if let i = p.output_configs.firstIndex(where: { $0.id == id }) { change(&p.output_configs[i]) }
        }
    }

    func deleteOutput(_ id: String) {
        update { p in
            p.output_configs.removeAll { $0.id == id }
            for i in p.run_configs.indices { p.run_configs[i].outputs.removeAll { $0 == id } }
        }
    }

    /// The folder this run config's runs go to, with `~` expanded.
    func outputRoot(for rc: RunConfig) -> String {
        (try? Core.run("outputs.root", ["project": Core.arg(project), "run_config": rc.id], as: String.self)) ?? rc.output_folder
    }

    // MARK: Run history

    /// Called by the run controller when the core has written history to the file.
    func runHistoryChanged() {
        guard let disk = try? Core.run("project.load", ["path": fileURL.path], as: Project.self) else { return }
        project.runs = mergedRuns(mine: project.runs, disk: disk.runs)
        base.runs = disk.runs
        AppState.shared.rememberRunFolders(project.runs.filter { $0.kind == "production" }.map(\.folder))
    }

    /// Points a run record at its folder after the folder moved (Locate in the run history).
    func relocateRun(_ id: String, to folder: URL) {
        update { p in
            if let i = p.runs.firstIndex(where: { $0.id == id }) { p.runs[i].folder = folder.path }
        }
    }

    func close() {
        autosaveWork?.cancel()
        save()
        watcher?.cancel()
        watcher = nil
        runs.stop()
        assistant.stop()
    }
}
