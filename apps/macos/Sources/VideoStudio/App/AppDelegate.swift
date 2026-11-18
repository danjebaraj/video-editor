import AppKit
import Combine
import SwiftUI
import UniformTypeIdentifiers

extension UTType {
    static let cutproj = UTType(exportedAs: "com.syncfusion.videostudio.project")
    static let cutbrand = UTType(exportedAs: "com.syncfusion.videostudio.brand")
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    static private(set) var shared: AppDelegate!
    /// One window per open project.
    private(set) var controllers: [ProjectWindowController] = []
    private var welcomeWindow: NSWindow?
    private var libraryWindow: NSWindow?
    private var settingsWindow: NSWindow?
    private let libraryState = LibraryWindowState()
    private let settingsState = SettingsWindowState()
    private var pendingOpens: [URL] = []
    private var finishedLaunching = false

    override init() {
        super.init()
        AppDelegate.shared = self
    }

    var activeController: ProjectWindowController? {
        if let c = NSApp.keyWindow?.windowController as? ProjectWindowController { return c }
        if let c = NSApp.mainWindow?.windowController as? ProjectWindowController { return c }
        return controllers.last
    }
    var activeDocument: ProjectDocument? { activeController?.doc }

    func applicationWillFinishLaunching(_ notification: Notification) {
        NSApp.mainMenu = MainMenu.build()
        UserDefaults.standard.set(false, forKey: "NSQuitAlwaysKeepsWindows")
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        finishedLaunching = true
        AppState.shared.detect(force: true)
        #if DEBUG
        if ProcessInfo.processInfo.environment["VS_SELFTEST"] != nil {
            SelfTest.run()
            return
        }
        if ProcessInfo.processInfo.environment["VS_DOCSHOTS"] != nil {
            DocShots.run()
            return
        }
        #endif
        if !pendingOpens.isEmpty {
            pendingOpens.forEach { open($0) }
            pendingOpens = []
        } else if let last = UserDefaults.standard.stringArray(forKey: "openProjects"), !last.isEmpty {
            // Reopen what was open at quit.
            for p in last where FileManager.default.fileExists(atPath: p) { open(URL(fileURLWithPath: p)) }
        }
        if controllers.isEmpty { showWelcome() }
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        // Tools may have been installed or signed in meanwhile; media may have moved.
        AppState.shared.detect()
        NSApp.dockTile.badgeLabel = nil
        for c in controllers { c.doc.resolveMedia() }
    }

    func applicationWillResignActive(_ notification: Notification) {
        for c in controllers { c.doc.save() }
    }

    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls {
            if url.pathExtension.lowercased() == "cutbrand" {
                importBrand(url)
            } else if finishedLaunching {
                open(url)
            } else {
                pendingOpens.append(url)
            }
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { showWelcome() }
        return true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        let running = controllers.filter { $0.doc.runs.isRunning }
        if !running.isEmpty {
            let names = running.map(\.doc.displayName).joined(separator: ", ")
            guard Prompt.confirm("A run is in progress.", message: "Quitting stops the run for \(names). Partial outputs and logs are kept.", ok: "Stop and Quit", destructive: true) else { return .terminateCancel }
        }
        UserDefaults.standard.set(controllers.map(\.doc.fileURL.path), forKey: "openProjects")
        for c in controllers { c.doc.close() }
        return .terminateNow
    }

    // MARK: Projects

    @discardableResult
    func open(_ url: URL) -> ProjectWindowController? {
        let url = url.standardizedFileURL
        if let existing = controllers.first(where: { $0.doc.fileURL.standardizedFileURL == url }) {
            existing.showWindow(nil)
            return existing
        }
        do {
            let doc = try ProjectDocument.open(url)
            return show(doc)
        } catch {
            Prompt.error("“\(url.lastPathComponent)” couldn't be opened.", detail: error.localizedDescription)
            return nil
        }
    }

    @discardableResult
    func show(_ doc: ProjectDocument) -> ProjectWindowController {
        let wc = ProjectWindowController(document: doc)
        controllers.append(wc)
        wc.showWindow(nil)
        if ProcessInfo.processInfo.environment["VS_SELFTEST"] == nil { NSDocumentController.shared.noteNewRecentDocumentURL(doc.fileURL) }
        welcomeWindow?.close()
        // Missing media is listed in the Relink sheet as the project opens (spec §6.3).
        doc.resolveMedia(showRelink: true)
        return wc
    }

    func windowClosed(_ wc: ProjectWindowController) {
        controllers.removeAll { $0 === wc }
        if controllers.isEmpty && NSApp.isActive { showWelcome() }
    }

    @objc func newProject(_ sender: Any?) {
        showWelcome(startNew: true)
    }

    @objc func openProject(_ sender: Any?) {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.cutproj]
        panel.allowsMultipleSelection = true
        panel.message = "Choose a Video Studio project"
        guard panel.runModal() == .OK else { return }
        panel.urls.forEach { open($0) }
    }

    /// Creates a project file from a template and opens it.
    func createProject(name: String, intent: String, intentOther: String?, template: String?, templateFile: URL?, at url: URL) {
        do {
            var p: Project
            if let templateFile {
                p = try Core.run("project.from_template", ["path": templateFile.path, "name": name])
            } else {
                p = try Core.run("project.new", ["name": name, "intent": intent, "intent_other": intentOther, "template": template])
            }
            p = try Core.run("project.save", ["project": Core.arg(p), "path": url.path])
            show(ProjectDocument(project: p, url: url))
        } catch {
            Prompt.error("The project couldn't be created.", detail: error.localizedDescription)
        }
    }

    // MARK: Windows

    func showWelcome(startNew: Bool = false) {
        if welcomeWindow == nil {
            let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 760, height: 520), styleMask: [.titled, .closable, .fullSizeContentView], backing: .buffered, defer: false)
            w.titlebarAppearsTransparent = true
            w.title = "Syncfusion Video Studio"
            w.titleVisibility = .hidden
            w.isReleasedWhenClosed = false
            w.center()
            welcomeWindow = w
        }
        welcomeWindow?.contentView = NSHostingView(rootView: WelcomeView(startWithNewProject: startNew))
        welcomeWindow?.makeKeyAndOrderFront(nil)
    }

    func closeWelcome() { welcomeWindow?.close() }

    @objc func showLibraryWindow(_ sender: Any?) { showLibrary(tab: .brands, select: nil) }

    func showLibrary(tab: LibraryTab, select: String?) {
        libraryState.tab = tab
        if let select { libraryState.selectedBrandID = select }
        if libraryWindow == nil {
            let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 620), styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
            w.title = "App Library"
            w.isReleasedWhenClosed = false
            w.contentView = NSHostingView(rootView: LibraryView(state: libraryState))
            w.setFrameAutosaveName("LibraryWindow")
            w.center()
            libraryWindow = w
        }
        libraryWindow?.makeKeyAndOrderFront(nil)
    }

    @objc func showSettingsWindow(_ sender: Any?) { showSettings(pane: nil) }

    func showSettings(pane: String?) {
        if let pane, let p = SettingsPane(rawValue: pane) { settingsState.pane = p }
        if settingsWindow == nil {
            let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 680, height: 520), styleMask: [.titled, .closable], backing: .buffered, defer: false)
            w.title = "Settings"
            w.isReleasedWhenClosed = false
            w.contentView = NSHostingView(rootView: SettingsView(state: settingsState))
            w.center()
            settingsWindow = w
        }
        settingsWindow?.makeKeyAndOrderFront(nil)
    }

    func importBrand(_ url: URL) {
        do {
            let b: BrandPreset = try Core.run("brand.import", ["file": url.path])
            AppState.shared.reloadBrands()
            showLibrary(tab: .brands, select: b.id)
        } catch {
            Prompt.error("The brand couldn't be imported.", detail: error.localizedDescription)
        }
    }
}
