import AppKit
import SwiftUI
import UniformTypeIdentifiers
import MarkdownCore

/// The main window of one project (spec §17): toolbar, sidebar, spec editor or preview, inspector.
@MainActor
final class ProjectWindowController: NSWindowController, NSWindowDelegate, NSMenuItemValidation {
    /// The open project. (`document` is taken by NSWindowController.)
    let doc: ProjectDocument
    let findBar = FindBarModel()
    /// Bumped to move keyboard focus into the Claude composer.
    let chatFocus = FocusRequest()

    var currentEditor: EditorController? { doc.editor }

    init(document: ProjectDocument) {
        self.doc = document
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1360, height: 860), styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView], backing: .buffered, defer: false)
        window.minSize = NSSize(width: 980, height: 600)
        window.isReleasedWhenClosed = false
        window.tabbingMode = .disallowed
        window.toolbarStyle = .unified
        super.init(window: window)
        doc.windowController = self
        findBar.windowController = self
        let host = NSHostingController(rootView: ProjectRootView(document: doc, findBar: findBar, chatFocus: chatFocus))
        host.sceneBridgingOptions = [.toolbars]
        window.contentViewController = host
        window.delegate = self
        window.setFrameAutosaveName("ProjectWindow")
        if window.frame.width < 980 { window.setContentSize(NSSize(width: 1360, height: 860)) }
        window.center()
        updateTitle()
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    func updateTitle() {
        window?.title = doc.displayName
        window?.representedURL = doc.fileURL
        window?.subtitle = IntentKind.label(doc.project.intent.kind)
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if doc.runs.isRunning {
            guard Prompt.confirm("A run is in progress.", message: "Closing the project stops the run. Partial outputs and logs are kept.", ok: "Stop and Close", destructive: true) else { return false }
        }
        return true
    }

    func windowWillClose(_ notification: Notification) {
        doc.close()
        AppDelegate.shared.windowClosed(self)
    }

    func windowDidBecomeKey(_ notification: Notification) {
        doc.resolveMedia()
    }

    /// The editor or the preview became the centre view.
    func modeChanged() {
        if !doc.showPreview {
            DispatchQueue.main.async { [weak self] in self?.doc.editor.focus() }
        }
    }

    func focusChat() { chatFocus.bump() }

    // MARK: Menu validation

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        switch item.action {
        case #selector(togglePreview(_:)):
            item.title = doc.showPreview ? "Show Source" : "Show Preview"
        case #selector(toggleInlineRendering(_:)):
            item.state = AppSettings.shared.inlineRendering ? .on : .off
        case #selector(toggleLineNumbers(_:)):
            item.state = AppSettings.shared.lineNumbers ? .on : .off
        case #selector(stopRun(_:)):
            return doc.runs.isRunning || doc.assistant.isRunning
        case #selector(runProject(_:)), #selector(dryRun(_:)):
            return !doc.runs.isRunning && AppState.shared.claudeAvailable
        case #selector(gatherInfo(_:)):
            return !doc.assistant.isRunning && AppState.shared.claudeAvailable
        case #selector(revealRunFolder(_:)), #selector(exportLogs(_:)), #selector(copyIssueReport(_:)):
            return selectedRunFolder != nil
        case #selector(formatCommand(_:)), #selector(insertSpecBlock(_:)), #selector(insertIsland(_:)):
            return !doc.showPreview && doc.editor.textView.isEditable
        case #selector(showSidebarTab(_:)):
            item.state = (item.representedObject as? String) == doc.sidebarTab.rawValue ? .on : .off
        case #selector(showInspectorTab(_:)):
            item.state = (item.representedObject as? String) == doc.inspectorTab.rawValue ? .on : .off
        default:
            break
        }
        return true
    }

    // MARK: File

    @objc func saveProject(_ sender: Any?) {
        doc.save()
    }

    @objc func saveProjectAs(_ sender: Any?) {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.cutproj]
        panel.nameFieldStringValue = doc.fileURL.lastPathComponent
        panel.directoryURL = doc.fileURL.deletingLastPathComponent()
        panel.message = "Pasted images are copied next to the new file. Other media stays where it is."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        if doc.saveAs(url) { NSDocumentController.shared.noteNewRecentDocumentURL(url) }
    }

    @objc func duplicateProject(_ sender: Any?) {
        doc.save()
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.cutproj]
        panel.nameFieldStringValue = doc.fileURL.deletingPathExtension().lastPathComponent + " copy.cutproj"
        panel.directoryURL = doc.fileURL.deletingLastPathComponent()
        panel.title = "Duplicate Project"
        let keepRuns = NSButton(checkboxWithTitle: "Keep run history", target: nil, action: nil)
        let keepSessions = NSButton(checkboxWithTitle: "Keep Claude sessions", target: nil, action: nil)
        let stack = NSStackView(views: [keepRuns, keepSessions])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.edgeInsets = NSEdgeInsets(top: 8, left: 16, bottom: 8, right: 16)
        panel.accessoryView = stack
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let _: Project = try Core.run("project.duplicate", ["source": doc.fileURL.path, "destination": url.path, "keep_runs": keepRuns.state == .on, "keep_sessions": keepSessions.state == .on])
            AppDelegate.shared.open(url)
        } catch {
            Prompt.error("The project couldn't be duplicated.", detail: error.localizedDescription)
        }
    }

    @objc func saveAsTemplate(_ sender: Any?) {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.cutproj]
        panel.nameFieldStringValue = doc.displayName + " template.cutproj"
        panel.title = "Save As Template"
        panel.message = "A template keeps the spec, intent, brand and run configs. Media, runs and sessions are left out."
        let templates = AppState.shared.libraryURL.appendingPathComponent("templates", isDirectory: true)
        try? FileManager.default.createDirectory(at: templates, withIntermediateDirectories: true)
        panel.directoryURL = templates
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let t: Project = try Core.run("project.as_template", ["project": Core.arg(doc.project), "name": url.deletingPathExtension().lastPathComponent])
            let _: Project = try Core.run("project.save", ["project": Core.arg(t), "path": url.path])
        } catch {
            Prompt.error("The template couldn't be saved.", detail: error.localizedDescription)
        }
    }

    @objc func exportSpec(_ sender: Any?) {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "md") ?? .plainText]
        panel.nameFieldStringValue = doc.displayName + ".md"
        panel.message = "media: links are written as file paths."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        doc.save()
        let text = (try? Core.run("project.export_spec", ["project": Core.arg(doc.project), "path": doc.fileURL.path], as: String.self)) ?? doc.project.spec
        do { try text.write(to: url, atomically: true, encoding: .utf8) } catch { Prompt.error("The spec couldn't be exported.", detail: error.localizedDescription) }
    }

    /// File › Add Media… (and the editor's attach command): files are referenced, never copied.
    @objc func addMedia(_ sender: Any?) {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = true
        panel.message = "Media is used where it is. Nothing is copied into the project."
        panel.prompt = "Add"
        guard panel.runModal() == .OK else { return }
        doc.addMedia(panel.urls)
        doc.sidebarTab = .media
    }

    /// Picks files and inserts `![name](media:<id>)` for each at the caret.
    func pickMedia() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.message = "Choose media to place in the spec. Files are used where they are."
        panel.prompt = "Insert"
        guard panel.runModal() == .OK else { return }
        doc.editor.insertMediaFiles(panel.urls)
    }

    @objc func addMediaFolder(_ sender: Any?) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.message = "Every media file in the folder is added, including subfolders."
        panel.prompt = "Add Folder"
        let kinds = NSPopUpButton()
        kinds.addItems(withTitles: ["All media", "Video only", "Audio only", "Images only"])
        let row = NSStackView(views: [NSTextField(labelWithString: "Add:"), kinds])
        row.edgeInsets = NSEdgeInsets(top: 8, left: 16, bottom: 8, right: 16)
        panel.accessoryView = row
        guard panel.runModal() == .OK, let url = panel.url else { return }
        let filter: [String]? = [nil, ["video"], ["audio"], ["image"]][kinds.indexOfSelectedItem]
        var args: Core.Args = ["project": Core.arg(doc.project), "path": doc.fileURL.path, "files": [url.path]]
        if let filter { args["kinds"] = filter }
        if let r: MediaAddResult = try? Core.run("media.add", args) {
            doc.update { $0.media = r.project.media }
            doc.resolveMedia()
        }
        doc.sidebarTab = .media
    }

    @objc func showRelink(_ sender: Any?) {
        doc.showRelinkSheet = true
    }

    /// File › Clean Up Pasted Images…: lists sidecar images the spec no longer uses and moves them
    /// to the Trash after confirmation. Nothing is ever deleted automatically (spec §5).
    @objc func cleanUpPastedImages(_ sender: Any?) {
        doc.save()
        let unused: [String] = (try? Core.run("project.unused_pasted_images", ["project": Core.arg(doc.project), "path": doc.fileURL.path])) ?? []
        guard !unused.isEmpty else {
            Prompt.error("There is nothing to clean up.", detail: "Every pasted image is used in the spec.")
            return
        }
        let names = unused.prefix(12).map { ($0 as NSString).lastPathComponent.prefix(16) + "…png" }.joined(separator: "\n")
        guard Prompt.confirm("Move \(unused.count) unused pasted image\(unused.count == 1 ? "" : "s") to the Trash?", message: "These are no longer referenced by the spec:\n\(names)\(unused.count > 12 ? "\n…" : "")", ok: "Move to Trash") else { return }
        let urls = unused.map { URL(fileURLWithPath: $0) }
        NSWorkspace.shared.recycle(urls) { [weak self] _, _ in
            guard let self else { return }
            let gone = Set(unused)
            self.doc.update { p in p.media.removeAll { $0.pasted == true && gone.contains($0.paths.absolute ?? "") } }
            self.doc.resolveMedia()
        }
    }

    // MARK: Edit

    @objc func showFind(_ sender: Any?) {
        if doc.showPreview { doc.showPreview = false }
        doc.findShowsReplace = false
        doc.showFindBar = true
        findBar.useSelectionIfAny()
        findBar.focusRequested += 1
    }

    @objc func showReplace(_ sender: Any?) {
        showFind(sender)
        doc.findShowsReplace = true
    }

    @objc func findNext(_ sender: Any?) { findBar.next() }
    @objc func findPrevious(_ sender: Any?) { findBar.previous() }

    // MARK: Format

    @objc func formatCommand(_ sender: NSMenuItem) {
        guard let name = sender.representedObject as? String else { return }
        let map: [String: FormatCommand] = ["bold": .bold, "italic": .italic, "strikethrough": .strikethrough, "code": .inlineCode, "link": .link, "h1": .heading1, "h2": .heading2, "h3": .heading3,
                                            "bullet": .bulletList, "numbered": .numberedList, "task": .taskList, "quote": .blockquote, "codeBlock": .codeBlock, "table": .table, "rule": .horizontalRule]
        if let cmd = map[name] { doc.editor.perform(cmd) }
    }

    /// Format commands from the editor's own menus (table tools, context menus).
    @objc func performFormatCommand(_ sender: NSMenuItem) {
        guard let raw = sender.representedObject as? String, let cmd = FormatCommand(rawValue: raw) else { return }
        doc.editor.perform(cmd)
    }

    @objc func insertSpecBlock(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        doc.editor.insertSpecBlock(id)
    }

    @objc func insertIsland(_ sender: Any?) { doc.editor.insertIsland() }

    // MARK: View

    @objc func togglePreview(_ sender: Any?) { doc.showPreview.toggle() }
    @objc func toggleInlineRendering(_ sender: Any?) { AppSettings.shared.inlineRendering.toggle() }
    @objc func toggleLineNumbers(_ sender: Any?) { AppSettings.shared.lineNumbers.toggle() }

    @objc func showSidebarTab(_ sender: NSMenuItem) {
        if let raw = sender.representedObject as? String, let tab = SidebarTab(rawValue: raw) { doc.sidebarTab = tab }
    }

    @objc func showInspectorTab(_ sender: NSMenuItem) {
        if let raw = sender.representedObject as? String, let tab = InspectorTab(rawValue: raw) { doc.inspectorTab = tab }
    }

    // MARK: Project

    @objc func validateProject(_ sender: Any?) {
        Task { await doc.validate() }
    }

    @objc func dryRun(_ sender: Any?) { doc.runs.requestDryRun() }
    @objc func runProject(_ sender: Any?) { doc.runs.requestRun() }

    @objc func stopRun(_ sender: Any?) {
        if doc.runs.isRunning { doc.runs.stop() } else { doc.assistant.stop() }
    }

    @objc func gatherInfo(_ sender: Any?) {
        doc.inspectorTab = .claude
        doc.assistant.gatherMissingInfo()
    }

    @objc func newChatSession(_ sender: Any?) {
        doc.inspectorTab = .claude
        doc.assistant.newSession()
        focusChat()
    }

    var selectedRunFolder: URL? {
        guard let id = doc.selectedRunID ?? doc.project.runs.last?.id, let r = doc.project.runs.first(where: { $0.id == id }) else { return nil }
        return URL(fileURLWithPath: r.folder, isDirectory: true)
    }

    @objc func revealRunFolder(_ sender: Any?) {
        guard let f = selectedRunFolder else { return }
        NSWorkspace.shared.activateFileViewerSelecting([f])
    }

    @objc func exportLogs(_ sender: Any?) {
        guard let f = selectedRunFolder else { return }
        RunActions.exportLogs(folder: f)
    }

    @objc func copyIssueReport(_ sender: Any?) {
        guard let f = selectedRunFolder else { return }
        RunActions.copyIssueReport(folder: f)
    }

    @objc func showHelp(_ sender: Any?) {
        if let url = URL(string: "https://github.com/danjebaraj/video-editor/blob/main/docs/video-studio/README.md") { NSWorkspace.shared.open(url) }
    }
}

/// A counter views observe to take keyboard focus on request.
@MainActor
final class FocusRequest: ObservableObject {
    @Published private(set) var count = 0
    func bump() { count += 1 }
}

/// Actions on a run folder shared by the menu and the Run inspector (spec §15).
@MainActor
enum RunActions {
    static func exportLogs(folder: URL) {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.zip]
        panel.nameFieldStringValue = folder.lastPathComponent + "-logs.zip"
        panel.title = "Export Logs"
        panel.message = "A zip of this run's logs, the instructions Claude was given, the spec and the edit. No media is included."
        let redact = NSButton(checkboxWithTitle: "Replace media paths, media file names and my home folder with placeholders", target: nil, action: nil)
        let box = NSStackView(views: [redact])
        box.edgeInsets = NSEdgeInsets(top: 8, left: 16, bottom: 8, right: 16)
        panel.accessoryView = box
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let _: Int = try Core.run("logs.export", ["folder": folder.path, "destination": url.path, "redact": redact.state == .on])
            NSWorkspace.shared.activateFileViewerSelecting([url])
        } catch {
            Prompt.error("The logs couldn't be exported.", detail: error.localizedDescription)
        }
    }

    static func copyIssueReport(folder: URL) {
        guard let report = try? Core.run("logs.issue_report", ["folder": folder.path], as: String.self) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(report, forType: .string)
    }
}
