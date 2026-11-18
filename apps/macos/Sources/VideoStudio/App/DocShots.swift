import AppKit
import ScreenCaptureKit
import WebKit

#if DEBUG
/// Takes the screenshots in the user guide. Run with `VS_DOCSHOTS=<folder>` (see scripts/docshots.sh).
///
/// It drives the real app, as the self test does, but with a made-up product, "Acme Diagrams":
/// a polished spec, sample media, a brand, a conversation with the (scripted) Claude and a run
/// that the real cutline renders. Windows are captured with ScreenCaptureKit, which includes
/// sheets, popups, video and shadows (an app may capture its own windows). Pictures go to `<folder>/shots/<name>.png`.
///
/// It uses its own library folder and a stand-in API key, and puts the user's recent projects and
/// window positions back afterwards.
@MainActor
enum DocShots {
    static var root = URL(fileURLWithPath: "/Users/Shared/Demo", isDirectory: true)
    private static var shotsDir: URL { root.appendingPathComponent("shots", isDirectory: true) }
    private static var log = ""

    /// The run folders the app knew before: Settings › Storage lists them, so the pictures start from none.
    nonisolated(unsafe) private static var savedRunFolders: [String]?

    /// Called from main.swift before anything else, so the core never touches the real library.
    nonisolated static func prepareEnvironment() {
        guard let dir = ProcessInfo.processInfo.environment["VS_DOCSHOTS"], !dir.isEmpty else { return }
        savedRunFolders = UserDefaults.standard.stringArray(forKey: "knownRunFolders")
        UserDefaults.standard.removeObject(forKey: "knownRunFolders")
        setenv("VIDEO_STUDIO_HOME", dir + "/library", 1)
        setenv("VIDEO_STUDIO_TEST_SECRETS", dir + "/secrets.json", 1)
        // The tools are reached through links in the demo folder, so Settings shows neutral paths
        // (not the build folder, not the home folder).
        let fm = FileManager.default
        let bin = dir + "/bin"
        try? fm.createDirectory(atPath: bin, withIntermediateDirectories: true)
        func link(_ name: String, to target: String, env: String, copy: Bool = false) {
            let path = bin + "/" + name
            try? fm.removeItem(atPath: path)
            // cutline reports the real path of what it finds, so it is a copy, not a link.
            let made = copy ? (try? fm.copyItem(atPath: target, toPath: path)) != nil : (try? fm.createSymbolicLink(atPath: path, withDestinationPath: target)) != nil
            if made { setenv(env, path, 1) }
        }
        let helpers = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers")
        link("claude", to: helpers.appendingPathComponent("vs-fake-claude").path, env: "VIDEO_STUDIO_CLAUDE")
        let searchPath = ProcessInfo.processInfo.environment["PATH"] ?? ""
        if let cutline = searchPath.split(separator: ":").map({ String($0) + "/cutline" }).first(where: { fm.isExecutableFile(atPath: $0) }) {
            link("cutline", to: cutline, env: "VIDEO_STUDIO_CUTLINE", copy: true)
        }
    }

    // MARK: Helpers

    private static func note(_ s: String) {
        log += s + "\n"
        FileHandle.standardError.write(Data((s + "\n").utf8))
        try? log.write(to: root.appendingPathComponent("docshots.log"), atomically: true, encoding: .utf8)
    }

    private static func sleep(_ seconds: Double) async {
        try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
    }

    @discardableResult
    private static func wait(_ what: String, timeout: Double = 20, _ condition: () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return true }
            await sleep(0.05)
        }
        note("    (timed out waiting for \(what))")
        return false
    }

    /// The given windows as the window server composes them (shadows included), on a clear background.
    private static func shoot(_ windows: [NSWindow]) async -> NSBitmapImageRep? {
        do {
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
            let ids = Set(windows.map { CGWindowID($0.windowNumber) })
            let shown = content.windows.filter { ids.contains($0.windowID) }
            guard let first = shown.first else { note("    (the window isn't on screen)"); return nil }
            let union = shown.dropFirst().reduce(first.frame) { $0.union($1.frame) }
            guard let display = content.displays.first(where: { $0.frame.intersects(union) }) else { return nil }
            // A margin for the shadow, kept inside the display.
            let rect = union.insetBy(dx: -36, dy: -36).intersection(display.frame)
            let scale = NSScreen.main?.backingScaleFactor ?? 2
            let config = SCStreamConfiguration()
            config.sourceRect = rect.offsetBy(dx: -display.frame.minX, dy: -display.frame.minY)
            config.width = Int(rect.width * scale)
            config.height = Int(rect.height * scale)
            config.showsCursor = false
            config.backgroundColor = .clear
            let image = try await SCScreenshotManager.captureImage(contentFilter: SCContentFilter(display: display, including: shown), configuration: config)
            return NSBitmapImageRep(cgImage: image)
        } catch {
            note("    (ScreenCaptureKit: \(error.localizedDescription))")
            return nil
        }
    }

    private static func write(_ rep: NSBitmapImageRep, _ name: String) {
        try? FileManager.default.createDirectory(at: shotsDir, withIntermediateDirectories: true)
        try? rep.representation(using: .png, properties: [:])?.write(to: shotsDir.appendingPathComponent(name + ".png"))
    }

    /// A picture of a window with its sheet and popups.
    private static func capture(_ window: NSWindow?, _ name: String) async {
        guard let window else { note("  ✘ \(name): no window"); return }
        // Windows look active only in the active app.
        NSApp.activate(ignoringOtherApps: true)
        (window.attachedSheet ?? window).makeKeyAndOrderFront(nil)
        window.displayIfNeeded()
        await sleep(0.8)
        if !NSApp.isActive { note("    (the app isn't active: its windows will look inactive)") }
        var windows = [window]
        func collect(_ w: NSWindow) {
            for c in w.childWindows ?? [] { windows.append(c); collect(c) }
            if let s = w.attachedSheet { windows.append(s); collect(s) }
        }
        collect(window)
        if let rep = await shoot(windows) {
            write(rep, name)
            note("  ✔ \(name) \(rep.pixelsWide)×\(rep.pixelsHigh)")
            return
        }
        // Without ScreenCaptureKit: the view's own drawing (no sheets, no video).
        guard let view = window.contentView?.superview ?? window.contentView, let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        write(rep, name)
        note("  ✔ \(name) (view drawing)")
    }

    private static func place(_ window: NSWindow?, width: CGFloat, height: CGFloat) {
        guard let window, let screen = NSScreen.main else { return }
        let area = screen.visibleFrame
        let w = min(width, area.width), h = min(height, area.height)
        window.setFrame(NSRect(x: area.midX - w / 2, y: area.midY - h / 2, width: w, height: h), display: true)
    }

    /// The inspector at the width the app asks for (380 pt). The panes' layout leaves it at the minimum.
    private static func widenInspector(_ window: NSWindow?, to width: CGFloat = 400) {
        guard let content = window?.contentView else { return }
        func find(_ v: NSView) -> NSSplitView? {
            if let split = v as? NSSplitView, split.arrangedSubviews.count == 3 { return split }
            for sub in v.subviews { if let found = find(sub) { return found } }
            return nil
        }
        guard let split = find(content) else { note("    (no split view found)"); return }
        split.setPosition(split.bounds.width - width - split.dividerThickness, ofDividerAt: 1)
        split.layoutSubtreeIfNeeded()
    }

    private static func writeScript(_ name: String, _ steps: [[String: Any]]) -> String {
        let dir = root.appendingPathComponent("scripts", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent(name)
        try? JSONSerialization.data(withJSONObject: steps).write(to: url)
        return url.path
    }

    // MARK: Run

    static func run() {
        root = URL(fileURLWithPath: ProcessInfo.processInfo.environment["VS_DOCSHOTS"] ?? root.path, isDirectory: true)
        let fm = FileManager.default
        // A clean start: nothing from an earlier run. The media in media/ is kept.
        for sub in ["library", "scripts", "out", "Archive", "shots", "assets-copy"] { try? fm.removeItem(at: root.appendingPathComponent(sub)) }
        try? fm.removeItem(at: root.appendingPathComponent("Diagram editor launch.cutproj"))
        try? fm.removeItem(at: root.appendingPathComponent("Diagram editor launch.cutproj.assets"))
        try? fm.createDirectory(at: root, withIntermediateDirectories: true)
        try? "{\"elevenlabs-api-key\": \"sk_demo_key_8f3a\"}".write(to: root.appendingPathComponent("secrets.json"), atomically: true, encoding: .utf8)
        NSApp.activate(ignoringOtherApps: true)
        Task {
            // The user's own state, put back at the end.
            let defaults = UserDefaults.standard
            let keys = ["openProjects", "NSWindow Frame ProjectWindow", "NSWindow Frame LibraryWindow"]
            let savedDefaults = keys.map { ($0, defaults.object(forKey: $0)) }
            let savedRecents = NSDocumentController.shared.recentDocumentURLs
            NSDocumentController.shared.clearRecentDocuments(nil)

            await body()

            NSApp.appearance = nil
            // Only the folders that still exist: earlier test runs left entries for deleted ones.
            let known = (savedRunFolders ?? []).filter { FileManager.default.fileExists(atPath: $0) && !$0.hasPrefix("/Users/Shared/Demo/") }
            if known.isEmpty { defaults.removeObject(forKey: "knownRunFolders") } else { defaults.set(known, forKey: "knownRunFolders") }
            for (key, value) in savedDefaults { if let value { defaults.set(value, forKey: key) } else { defaults.removeObject(forKey: key) } }
            NSDocumentController.shared.clearRecentDocuments(nil)
            for url in savedRecents.reversed() { NSDocumentController.shared.noteNewRecentDocumentURL(url) }
            note("DONE")
            exit(0)
        }
    }

    private static func body() async {
        let app = AppState.shared
        let fm = FileManager.default
        let delegate = AppDelegate.shared!
        NSApp.appearance = NSAppearance(named: .aqua)
        app.reloadLibrary()
        app.detect(force: true)
        await wait("tool detection", timeout: 60) { app.environment != nil && !app.isDetecting }
        note("Claude Code: \(String(describing: app.environment?.claude)) · cutline: \(String(describing: app.environment?.cutline?.path))")
        let media = root.appendingPathComponent("media")
        guard app.claudeAvailable, fm.fileExists(atPath: media.appendingPathComponent("diagram-demo.mov").path) else {
            note("✘ Claude Code (the scripted one) or the sample media in \(media.path) is missing")
            return
        }

        // MARK: Brand
        note("Brand")
        var acme: BrandPreset = (try? Core.run("brand.new", ["name": "Acme Diagrams"])) ?? BrandPreset(id: "", name: "Acme Diagrams")
        acme.description = "The Acme Diagrams product line"
        acme.colors.primary = "#F26B21"
        acme.colors.secondary = "#1F2A44"
        acme.colors.accent = "#2D9CDB"
        acme.logos.primary = try? Core.run("brand.asset", ["path": media.appendingPathComponent("logo-dark.png").path])
        acme.caption_style = "bold_pop"
        acme.lower_third_style = "Slim accent bar, white text"
        acme.music_mood = "Bright, optimistic, unobtrusive"
        acme.voice.voice_id = "JBFqnCBsd6RMkjVDRZzb"
        acme.voice.name = "George"
        acme.voice.model = "eleven_v4"
        acme.rules = "Do: lead with what the viewer can do, not how it works.\nDo: end on the product name and a clear next step.\nDon't: use stock footage of people at laptops.\nDon't: promise features that aren't released."
        var launches = InstructionSet.new(label: "Product launches")
        launches.text = "Audience: developers and product managers who draw diagrams every week.\nTerminology: say \"diagram editor\", never \"charting tool\".\nMessage: diagrams that take seconds, not afternoons."
        var tutorials = InstructionSet.new(label: "Tutorials")
        tutorials.text = "Audience: new users in their first week.\nKeep each step to one action, and name every button the way it is labelled."
        acme.instructions = [launches, tutorials]
        acme = (try? Core.run("brand.save", ["brand": Core.arg(acme)])) ?? acme
        app.reloadBrands()

        // MARK: Project
        note("Project")
        let projectURL = root.appendingPathComponent("Diagram editor launch.cutproj")
        delegate.createProject(name: "Diagram editor launch", intent: "product_demo", intentOther: nil, template: nil, templateFile: nil, at: projectURL)
        guard let wc = delegate.controllers.first else { note("✘ the project window didn't open"); return }
        let doc = wc.doc
        let ed = doc.editor
        place(wc.window, width: 1360, height: 860)
        await sleep(0.5)
        widenInspector(wc.window)
        NSDocumentController.shared.noteNewRecentDocumentURL(projectURL)

        let ids = doc.addMedia(["palette-closeup.mov", "diagram-demo.mov", "logo-dark.png", "brand-theme.m4a"].map { media.appendingPathComponent($0) })
        note("media: \(ids)")
        let idOf: (String) -> String = { name in doc.project.media.first { ($0.paths.absolute ?? "").hasSuffix("/" + name) }?.id ?? "m-\(name)" }
        let idPalette = idOf("palette-closeup.mov"), idDemo = idOf("diagram-demo.mov"), idMusic = idOf("brand-theme.m4a")
        await wait("probe", timeout: 30) { doc.project.mediaItem(idDemo)?.probe?.duration_s != nil && doc.project.mediaItem(idPalette)?.probe?.duration_s != nil }

        let spec = """
        # Diagram editor: product launch

        A 45-second launch video for the new diagram editor. Friendly and confident, for developers and product managers.

        ## Scene 1: Hook
        - **Duration:** 6 s
        - **Media:** ![Shape palette](media:\(idPalette))
        - **On-screen text:** Flowcharts without the friction

        ```vo
        [warmly] Ever spent an afternoon dragging boxes around just to explain a process?
        ```

        ## Scene 2: Build a flowchart
        - **Duration:** 24 s
        - **Media:** ![Diagram editor demo](media:\(idDemo))
        - **On-screen text:** Drag. Drop. Connect.

        ```vo
        [confident] Drag shapes from the palette, drop them on the canvas, and connect them in seconds. [short pause] The layout takes care of itself.
        ```

        ## Scene 3: Export anywhere
        - **Duration:** 15 s
        - **Record:** Diagram Builder
          1. Open the "Onboarding flow" diagram
          2. Choose Export › PNG
          3. Wait for the file to appear on the desktop
        - **On-screen text:** One click to PNG, SVG or PDF

        ```vo
        [explaining patiently] When you're done, export to PNG, SVG or PDF with a single click, and share it anywhere your team works.
        ```

        ## Call to action
        [inviting] Try the diagram editor free today at example.com/diagrams.

        ```music
        mood: bright, optimistic
        tempo: medium
        length: match the video
        source: media:\(idMusic)
        ```

        """
        ed.replaceAllText(spec, undoable: false)
        doc.textDidChange()
        doc.setBrand(acme)
        doc.setInstructions(launches.id, used: true)
        doc.update { p in
            p.intent.audience = "Developers and product managers"
            p.intent.tone = "Friendly, confident"
            p.intent.target_length_s = 45
            p.intent.cta = "Try it free today"
            p.intent.locales = ["en"]
            p.output_configs.append(OutputConfig(id: "out_shorts", preset: "youtube_shorts"))
            let all = p.output_configs.map(\.id)
            p.run_configs[0].name = "Launch cut"
            p.run_configs[0].outputs = all
            p.run_configs[0].output_folder = root.appendingPathComponent("out").path
            var shorts = p.run_configs[0]
            shorts.id = "rc_shorts"
            shorts.name = "Shorts only"
            shorts.outputs = ["out_shorts"]
            shorts.dry_run_first = false
            p.run_configs.append(shorts)
        }
        let launchID = doc.project.run_configs[0].id
        doc.selectedRunConfigID = launchID
        await wait("outline") { !doc.outline.isEmpty }
        await wait("media resolution") { doc.mediaStatus(idDemo) == "ok" }
        doc.save()

        // MARK: Welcome and New Project
        note("Welcome")
        delegate.showWelcome()
        await sleep(0.8)
        let welcome = NSApp.windows.first { $0.title == "Syncfusion Video Studio" }
        await capture(welcome, "welcome")
        delegate.showWelcome(startNew: true)
        await wait("the New Project sheet") { welcome?.attachedSheet != nil }
        await capture(welcome, "new-project")
        if let sheet = welcome?.attachedSheet { welcome?.endSheet(sheet) }
        delegate.closeWelcome()

        // MARK: The project window
        note("Project window")
        wc.window?.makeKeyAndOrderFront(nil)
        place(wc.window, width: 1360, height: 860)
        widenInspector(wc.window)
        doc.sidebarTab = .outline
        doc.inspectorTab = .claude
        ed.goToLine(1)
        await capture(wc.window, "project-window")

        // The slash menu, on a new empty line at the end.
        note("Slash menu")
        ed.textView.window?.makeFirstResponder(ed.textView)
        ed.textView.setSelectedRange(NSRange(location: ed.nsText.length, length: 0))
        ed.textView.insertText("\n", replacementRange: ed.textView.selectedRange())
        ed.textView.insertText("/", replacementRange: ed.textView.selectedRange())
        await wait("the slash popup", timeout: 5) { ed.completion.isVisible }
        await capture(wc.window, "slash-menu")
        ed.completion.close()
        ed.replaceAllText(spec, undoable: false)
        doc.textDidChange()
        ed.goToLine(1)

        // MARK: Preview
        note("Preview")
        doc.showPreview = true
        var rendered = false
        doc.preview.onRendered = { rendered = true }
        await wait("preview render", timeout: 20) { rendered }
        await sleep(1.5)
        await capture(wc.window, "preview")
        doc.showPreview = false

        // MARK: Media
        note("Media")
        doc.sidebarTab = .media
        doc.selectedMediaID = idDemo
        await sleep(1.0)
        await capture(wc.window, "media")
        doc.sidebarTab = .outline

        // MARK: Claude: a question, then changes to review
        note("Claude")
        let specPath = app.libraryURL.appendingPathComponent("assistant/\(doc.project.id)/\(doc.assistant.current?.id ?? "")/spec.md").path
        doc.assistant.extraEnv = ["FAKE_CLAUDE_SCRIPT": writeScript("assistant.json", [
            ["say": "I'll tighten the hook and make the closing line more direct."],
            ["tool": "AskUserQuestion", "input": ["questions": [["question": "How long should the hook be?", "header": "Hook", "multiSelect": false,
                "options": [["label": "4 s", "description": "Punchy: one line of narration"], ["label": "6 s", "description": "Keep it as it is"]]]]]],
            ["tool": "Edit", "input": ["file_path": specPath, "old_string": "- **Duration:** 6 s", "new_string": "- **Duration:** 4 s"]],
            ["tool": "Edit", "input": ["file_path": specPath, "old_string": "[warmly] Ever spent an afternoon dragging boxes around just to explain a process?", "new_string": "[warmly] Ever lost an afternoon to dragging boxes around?"]],
            ["tool": "Edit", "input": ["file_path": specPath, "old_string": "[inviting] Try the diagram editor free today at example.com/diagrams.", "new_string": "[inviting] Try it free at example.com/diagrams."]],
            ["result": ["text": "The hook is now 4 seconds and the call to action is one short line. Review the changes in the editor.", "cost": 0.03]],
        ])]
        doc.inspectorTab = .claude
        _ = doc.assistant.send("Make the hook shorter and punchier, and make the call to action more direct.")
        await wait("question card") { doc.assistant.current?.messages.contains { $0.role == .question && $0.questionState == .pending } == true }
        await capture(wc.window, "claude-question")
        if let q = doc.assistant.current?.messages.first(where: { $0.role == .question }) {
            doc.assistant.answer(messageID: q.id, answers: ["How long should the hook be?": "4 s"])
        }
        await wait("review", timeout: 30) { doc.review.pending != nil }
        await sleep(0.5)
        await capture(wc.window, "claude-review")
        let hunkCount = doc.review.pending?.hunks.count ?? 0
        for i in 0..<hunkCount {
            if doc.review.pending == nil { break }
            doc.review.decide(i, accept: true)
        }
        await wait("review done") { doc.review.pending == nil }

        // MARK: Inspector tabs
        note("Inspector")
        doc.inspectorTab = .project
        await sleep(0.4)
        await capture(wc.window, "project-inspector")
        doc.inspectorTab = .outputs
        await sleep(0.4)
        await capture(wc.window, "outputs")

        // MARK: Validation: a blocker first, then fixed
        note("Validation")
        let good = ed.text
        ed.replaceAllText(good.replacingOccurrences(of: "- **On-screen text:** Flowcharts without the friction", with: "- **On-screen text:** Flowcharts without the friction\n- **Shot:** ![Intro bumper](media:m-intro) in 0:00, out 0:02"), undoable: false)
        doc.textDidChange()
        doc.inspectorTab = .validation
        _ = await doc.validate()
        await sleep(0.6)
        await capture(wc.window, "validation")
        ed.replaceAllText(good, undoable: false)
        doc.textDidChange()
        _ = await doc.validate()

        // MARK: Run: confirmation (with a dry run), then a real run
        note("Run")
        doc.inspectorTab = .run
        doc.selectedRunConfigID = launchID
        doc.runs.requestRun()
        await wait("confirmation sheet", timeout: 30) { doc.runs.plan != nil }
        await sleep(1.0)
        await capture(wc.window, "run-confirm")
        doc.runs.plan = nil
        await sleep(0.5)

        let a = media.appendingPathComponent("palette-closeup.mov").path, b = media.appendingPathComponent("diagram-demo.mov").path, music = media.appendingPathComponent("brand-theme.m4a").path
        let edit = """
        {
          version: 1, name: "diagram-editor-launch", target: "shorts",
          default_reframe: { mode: "fit_blur" },
          assets: { palette: {path: "\(a)"}, demo: {path: "\(b)"}, music: {path: "\(music)", role: "music"} },
          main: [
            { id: "hook", clip: "palette", in: 1.0, out: 4.0 },
            { transition: { type: "fade", duration: 0.3 } },
            { id: "build", clip: "demo", in: 1.0, out: 5.0 },
          ],
          layers: [
            { type: "text", template: "hook_title", text: "Flowcharts without the friction", start: 0, duration: 2.6, position: "center" },
            { type: "audio", asset: "music", start: 0, end: "end", gain_db: -8, fade_in: 0.3, fade_out: 0.5 },
          ],
          exports: [ {name: "youtube-shorts", target: "shorts", out: "outputs/youtube-shorts/youtube-shorts.mp4"} ],
        }
        """
        let runScript = writeScript("run.json", [
            ["say": "Reading the spec and the media manifest."],
            ["tool": "Read", "input": ["file_path": "spec.md"]],
            ["tool": "Write", "input": ["file_path": "project.json5", "content": edit]],
            ["tool": "Bash", "input": ["command": "cutline validate --json project.json5"]],
            ["tool": "AskUserQuestion", "input": ["questions": [["question": "Should the music play under the whole video?", "header": "Music", "multiSelect": false,
                "options": [["label": "Yes, and fade out at the end", "description": "Recommended for a 6-second Short"], ["label": "Only under the intro"]]]]]],
            ["tool": "Bash", "input": ["command": "cutline render project.json5"]],
            ["tool": "Write", "input": ["file_path": "summary.md", "content": "## What I made\nOne YouTube Short: the palette hook, then the flowchart build, with the brand music faded out.\n\n## Assumptions\nThe hook is 3 seconds.\n"]],
            ["result": ["text": "The Short is rendered and passed QA.", "cost": 0.42]],
        ])
        doc.runs.testOptions = ["extra_env": ["FAKE_CLAUDE_SCRIPT": runScript]]
        doc.selectedRunConfigID = "rc_shorts"
        doc.runs.requestRun()
        await wait("confirmation sheet", timeout: 30) { doc.runs.plan != nil }
        if let plan = doc.runs.plan { doc.runs.confirm(plan) }
        await wait("run question", timeout: 60) { doc.runs.question != nil }
        await sleep(0.8)
        await capture(wc.window, "run-question")
        doc.runs.answer(["Should the music play under the whole video?": "Yes, and fade out at the end"])
        await wait("render stage", timeout: 30) { doc.runs.currentStage == "render" }
        await sleep(0.5)
        await capture(wc.window, "run-live")
        await wait("run to finish", timeout: 240) { !doc.runs.isRunning }
        note("run: \(doc.runs.lastOutcome?.status ?? "?") \(doc.runs.lastOutcome?.warnings ?? [])")
        doc.sidebarTab = .runs
        doc.inspectorTab = .run
        await sleep(2.5)
        await capture(wc.window, "run-results")
        doc.sidebarTab = .outline
        doc.save()

        // MARK: App Library
        note("Library")
        delegate.showLibrary(tab: .brands, select: acme.id)
        await sleep(0.8)
        let library = NSApp.windows.first { $0.title == "App Library" }
        place(library, width: 980, height: 720)
        await capture(library, "library-brand")
        delegate.showLibrary(tab: .presets, select: nil)
        await sleep(0.8)
        await capture(library, "library-outputs")
        library?.close()

        // MARK: Settings
        note("Settings")
        for pane in ["general", "finish", "claude", "cutline", "elevenlabs", "storage", "editor"] {
            delegate.showSettings(pane: pane)
            await sleep(0.8)
            await capture(NSApp.windows.first { $0.title == "Settings" }, "settings-\(pane)")
        }
        NSApp.windows.first { $0.title == "Settings" }?.close()

        // MARK: Relink: the media has moved
        note("Relink")
        doc.save()
        wc.window?.close()
        await sleep(0.4)
        let archived = root.appendingPathComponent("Archive/media")
        try? fm.createDirectory(at: archived.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? fm.copyItem(at: media, to: archived)
        try? fm.removeItem(at: media)
        if let wc2 = delegate.open(projectURL) {
            place(wc2.window, width: 1360, height: 860)
            widenInspector(wc2.window)
            await wait("the Relink sheet", timeout: 15) { wc2.doc.showRelinkSheet }
            await sleep(1.0)
            await capture(wc2.window, "relink")
            wc2.doc.showRelinkSheet = false
            wc2.doc.save()
            wc2.window?.close()
        }
        // The media goes back where it was.
        try? fm.moveItem(at: archived, to: media)
        try? fm.removeItem(at: root.appendingPathComponent("Archive"))
    }
}
#endif
