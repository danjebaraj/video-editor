import AppKit

/// The menu bar. Every action in the app is reachable from here, with a key equivalent for the
/// frequent ones (spec §17: keyboard access to every action).
@MainActor
enum MainMenu {
    static func build() -> NSMenu {
        let main = NSMenu()
        func add(_ title: String, _ items: [NSMenuItem]) -> NSMenu {
            let item = NSMenuItem()
            let menu = NSMenu(title: title)
            items.forEach { menu.addItem($0) }
            item.submenu = menu
            main.addItem(item)
            return menu
        }
        func item(_ title: String, _ action: Selector?, _ key: String = "", _ mods: NSEvent.ModifierFlags = [.command], target: AnyObject? = nil) -> NSMenuItem {
            let i = NSMenuItem(title: title, action: action, keyEquivalent: key)
            i.keyEquivalentModifierMask = mods
            i.target = target
            return i
        }
        let sep = { NSMenuItem.separator() }
        let app = AppDelegate.shared!
        typealias W = ProjectWindowController

        _ = add("Syncfusion Video Studio", [
            item("About Syncfusion Video Studio", #selector(NSApplication.orderFrontStandardAboutPanel(_:))),
            sep(),
            item("Settings…", #selector(AppDelegate.showSettingsWindow(_:)), ",", target: app),
            item("App Library", #selector(AppDelegate.showLibraryWindow(_:)), "l", [.command, .shift], target: app),
            sep(),
            item("Hide Syncfusion Video Studio", #selector(NSApplication.hide(_:)), "h"),
            item("Hide Others", #selector(NSApplication.hideOtherApplications(_:)), "h", [.command, .option]),
            item("Show All", #selector(NSApplication.unhideAllApplications(_:))),
            sep(),
            item("Quit Syncfusion Video Studio", #selector(NSApplication.terminate(_:)), "q"),
        ])

        let recent = NSMenuItem(title: "Open Recent", action: nil, keyEquivalent: "")
        let recentMenu = NSMenu(title: "Open Recent")
        recentMenu.delegate = RecentProjectsMenu.shared
        recent.submenu = recentMenu
        _ = add("File", [
            item("New Project…", #selector(AppDelegate.newProject(_:)), "n", target: app),
            item("Open…", #selector(AppDelegate.openProject(_:)), "o", target: app),
            recent,
            sep(),
            item("Close", #selector(NSWindow.performClose(_:)), "w"),
            item("Save", #selector(W.saveProject(_:)), "s"),
            item("Save As…", #selector(W.saveProjectAs(_:)), "s", [.command, .shift]),
            item("Duplicate…", #selector(W.duplicateProject(_:))),
            item("Save As Template…", #selector(W.saveAsTemplate(_:))),
            sep(),
            item("Add Media…", #selector(W.addMedia(_:)), "i", [.command, .shift]),
            item("Add Media Folder…", #selector(W.addMediaFolder(_:))),
            item("Relink Media…", #selector(W.showRelink(_:))),
            item("Clean Up Pasted Images…", #selector(W.cleanUpPastedImages(_:))),
            sep(),
            item("Export Spec…", #selector(W.exportSpec(_:)), "e", [.command, .shift]),
        ])

        _ = add("Edit", [
            item("Undo", Selector(("undo:")), "z"),
            item("Redo", Selector(("redo:")), "z", [.command, .shift]),
            sep(),
            item("Cut", #selector(NSText.cut(_:)), "x"),
            item("Copy", #selector(NSText.copy(_:)), "c"),
            item("Paste", #selector(NSText.paste(_:)), "v"),
            item("Paste and Match Style", #selector(NSTextView.pasteAsPlainText(_:)), "v", [.command, .option, .shift]),
            item("Select All", #selector(NSText.selectAll(_:)), "a"),
            sep(),
            item("Find…", #selector(W.showFind(_:)), "f"),
            item("Find and Replace…", #selector(W.showReplace(_:)), "f", [.command, .option]),
            item("Find Next", #selector(W.findNext(_:)), "g"),
            item("Find Previous", #selector(W.findPrevious(_:)), "g", [.command, .shift]),
        ])

        let fmt = add("Format", [])
        // Format commands carry their own shortcuts (FormatCommand.shortcut), so the menu and the
        // editor agree. Toggle Task (⌘Return) stays out of the menu: as a menu shortcut it would
        // take ⌘Return from the Claude message box, where it sends.
        let groups: [[FormatCommand]] = [
            [.bold, .italic, .strikethrough, .inlineCode, .link],
            [.heading1, .heading2, .heading3, .heading4, .heading5, .heading6, .paragraph],
            [.bulletList, .numberedList, .taskList, .blockquote, .codeBlock, .table, .formatTable, .horizontalRule],
            [.indent, .outdent, .moveLineUp, .moveLineDown],
        ]
        for (n, group) in groups.enumerated() {
            if n > 0 { fmt.addItem(sep()) }
            for cmd in group {
                fmt.addItem(cmd.menuItem())
            }
        }
        fmt.addItem(sep())
        for (title, command) in [("Scene", "scene"), ("Voice-over", "vo"), ("Shot", "shot"), ("Music Brief", "music"), ("Call to Action", "cta")] {
            let i = item("Insert \(title)", #selector(W.insertSpecBlock(_:)))
            i.representedObject = command
            fmt.addItem(i)
        }
        fmt.addItem(sep())
        let (islandKey, islandMods) = FormatCommand.island.shortcut ?? ("", [])
        fmt.addItem(item("Insert Island", #selector(W.insertIsland(_:)), islandKey, islandMods))

        _ = add("View", [
            item("Show Preview", #selector(W.togglePreview(_:)), "/"),
            item("Show Images and Diagrams Inline", #selector(W.toggleInlineRendering(_:))),
            item("Line Numbers", #selector(W.toggleLineNumbers(_:))),
            sep(),
            item("Outline", #selector(W.showSidebarTab(_:)), "1", [.command, .control]).with("outline"),
            item("Media", #selector(W.showSidebarTab(_:)), "2", [.command, .control]).with("media"),
            item("Runs", #selector(W.showSidebarTab(_:)), "3", [.command, .control]).with("runs"),
            sep(),
            item("Claude", #selector(W.showInspectorTab(_:)), "1", [.command, .option]).with("claude"),
            item("Project", #selector(W.showInspectorTab(_:)), "2", [.command, .option]).with("project"),
            item("Outputs", #selector(W.showInspectorTab(_:)), "3", [.command, .option]).with("outputs"),
            item("Validation", #selector(W.showInspectorTab(_:)), "4", [.command, .option]).with("validation"),
            item("Run", #selector(W.showInspectorTab(_:)), "5", [.command, .option]).with("run"),
            sep(),
            item("Enter Full Screen", #selector(NSWindow.toggleFullScreen(_:)), "f", [.command, .control]),
        ])

        _ = add("Project", [
            item("Validate", #selector(W.validateProject(_:)), "v", [.command, .shift]),
            item("Dry Run", #selector(W.dryRun(_:)), "r", [.command, .option]),
            sep(),
            item("Run", #selector(W.runProject(_:)), "r"),
            item("Stop", #selector(W.stopRun(_:)), "."),
            sep(),
            item("Gather Missing Info", #selector(W.gatherInfo(_:)), "g", [.command, .control]),
            item("New Claude Session", #selector(W.newChatSession(_:)), "n", [.command, .shift]),
            sep(),
            item("Reveal Run Folder", #selector(W.revealRunFolder(_:))),
            item("Export Logs…", #selector(W.exportLogs(_:))),
            item("Copy Issue Report", #selector(W.copyIssueReport(_:))),
        ])

        let window = add("Window", [
            item("Minimize", #selector(NSWindow.performMiniaturize(_:)), "m"),
            item("Zoom", #selector(NSWindow.performZoom(_:))),
            sep(),
            item("Welcome to Video Studio", #selector(AppDelegate.newProject(_:)), target: app),
            sep(),
            item("Bring All to Front", #selector(NSApplication.arrangeInFront(_:))),
        ])
        NSApp.windowsMenu = window
        _ = add("Help", [item("Syncfusion Video Studio Help", #selector(W.showHelp(_:)), "?")])
        return main
    }
}

/// Fills File › Open Recent from the recent documents list each time it opens.
@MainActor
final class RecentProjectsMenu: NSObject, NSMenuDelegate {
    static let shared = RecentProjectsMenu()

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let urls = NSDocumentController.shared.recentDocumentURLs.filter { $0.pathExtension == "cutproj" }
        for url in urls {
            let item = NSMenuItem(title: url.deletingPathExtension().lastPathComponent, action: #selector(open(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = url
            item.toolTip = url.path
            menu.addItem(item)
        }
        if urls.isEmpty {
            let none = NSMenuItem(title: "No Recent Projects", action: nil, keyEquivalent: "")
            none.isEnabled = false
            menu.addItem(none)
        }
        menu.addItem(.separator())
        let clear = NSMenuItem(title: "Clear Menu", action: #selector(NSDocumentController.clearRecentDocuments(_:)), keyEquivalent: "")
        clear.target = NSDocumentController.shared
        menu.addItem(clear)
    }

    @objc private func open(_ sender: NSMenuItem) {
        if let url = sender.representedObject as? URL { AppDelegate.shared.open(url) }
    }
}

extension NSMenuItem {
    func with(_ object: Any) -> NSMenuItem {
        representedObject = object
        return self
    }
}
