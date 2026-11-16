import AppKit
import WebKit

#if DEBUG
/// The in-app self test (spec §20). Run with `VS_SELFTEST=<scratch folder>` (see build.sh).
///
/// It drives the real app: spec editing and preview, Claude edit review, the relink flow on a
/// moved project, brand switching, a validation report with blockers, and a full run with the
/// fake Claude Code CLI driving the real cutline over `cutline selftest --quick` assets.
/// Results go to `<scratch>/selftest.log`, window snapshots to `<scratch>/shots/`, and the exit
/// status is the number of failed checks.
@MainActor
enum SelfTest {
    static var scratch = URL(fileURLWithPath: NSTemporaryDirectory())
    private static var failures = 0
    private static var checks = 0
    private static var log = ""
    private static var shot = 0

    /// Called from main.swift before anything else, so the core never touches the real library.
    nonisolated static func prepareEnvironment() {
        guard let dir = ProcessInfo.processInfo.environment["VS_SELFTEST"], !dir.isEmpty else { return }
        setenv("VIDEO_STUDIO_HOME", dir + "/library", 1)
        setenv("VIDEO_STUDIO_TEST_SECRETS", dir + "/secrets.json", 1)
        let helpers = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers")
        // The real Claude Code is replaced by the scripted fake; cutline is the real one.
        setenv("VIDEO_STUDIO_CLAUDE", helpers.appendingPathComponent("vs-fake-claude").path, 1)
        UserDefaults.standard.set(false, forKey: "NSQuitAlwaysKeepsWindows")
    }

    private static func note(_ s: String) {
        log += s + "\n"
        FileHandle.standardError.write(Data((s + "\n").utf8))
        try? log.write(to: scratch.appendingPathComponent("selftest.log"), atomically: true, encoding: .utf8)
    }

    private static func check(_ name: String, _ ok: Bool, _ detail: @autoclosure () -> String = "") {
        checks += 1
        if ok {
            note("  ✔ \(name)")
        } else {
            failures += 1
            let d = detail()
            note("  ✘ \(name)\(d.isEmpty ? "" : " — \(d)")")
        }
    }

    private static func sleep(_ seconds: Double) async {
        try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
    }

    /// Waits until `condition` holds, polling; returns false on timeout.
    @discardableResult
    private static func wait(_ what: String, timeout: Double = 15, _ condition: () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return true }
            await sleep(0.05)
        }
        note("    (timed out waiting for \(what))")
        return false
    }

    /// Saves a picture of a window, title bar and toolbar included.
    private static func snapshot(_ window: NSWindow?, _ name: String) async {
        guard let window, let view = window.contentView?.superview ?? window.contentView else { return }
        window.displayIfNeeded()
        await sleep(0.35)
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        shot += 1
        let dir = scratch.appendingPathComponent("shots", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? rep.representation(using: .png, properties: [:])?.write(to: dir.appendingPathComponent(String(format: "%02d-%@.png", shot, name)))
    }

    private static func snapshot(web: WKWebView, _ name: String) async {
        let image: NSImage? = await withCheckedContinuation { c in
            web.takeSnapshot(with: nil) { image, _ in c.resume(returning: image) }
        }
        guard let tiff = image?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff) else { return }
        shot += 1
        let dir = scratch.appendingPathComponent("shots", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? rep.representation(using: .png, properties: [:])?.write(to: dir.appendingPathComponent(String(format: "%02d-%@.png", shot, name)))
    }

    private static func js(_ doc: ProjectDocument, _ script: String) async -> Any? {
        await withCheckedContinuation { c in doc.preview.evaluate(script) { c.resume(returning: $0) } }
    }

    private static func writeScript(_ name: String, _ steps: [[String: Any]]) -> String {
        let url = scratch.appendingPathComponent(name)
        try? JSONSerialization.data(withJSONObject: steps).write(to: url)
        return url.path
    }

    private static func runProcess(_ exe: String, _ args: [String]) async -> Int32 {
        await Task.detached {
            let p = Process()
            p.executableURL = URL(fileURLWithPath: exe)
            p.arguments = args
            p.standardOutput = FileHandle.nullDevice
            p.standardError = FileHandle.nullDevice
            do { try p.run() } catch { return -1 }
            p.waitUntilExit()
            return p.terminationStatus
        }.value
    }

    static func run() {
        scratch = URL(fileURLWithPath: ProcessInfo.processInfo.environment["VS_SELFTEST"] ?? NSTemporaryDirectory(), isDirectory: true)
        try? FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        // A stand-in key, so the spec's voice-over doesn't block validation. Nothing calls ElevenLabs.
        try? "{\"elevenlabs-api-key\": \"selftest-key\"}".write(to: scratch.appendingPathComponent("secrets.json"), atomically: true, encoding: .utf8)
        Task {
            await body()
            note(failures == 0 ? "SELF TEST PASSED (\(checks) checks)" : "SELF TEST FAILED: \(failures) of \(checks) checks")
            exit(Int32(min(failures, 100)))
        }
    }

    private static func body() async {
        let app = AppState.shared
        let fm = FileManager.default
        let delegate = AppDelegate.shared!
        app.reloadLibrary()

        // MARK: Environment
        note("Environment")
        app.detect(force: true)
        await wait("tool detection", timeout: 60) { app.environment != nil && !app.isDetecting }
        check("the fake Claude Code CLI is detected and supported", app.claudeAvailable, "\(String(describing: app.environment?.claude))")
        check("effort levels are read from the CLI's help", app.effortLevels == ["low", "medium", "high"], "\(app.effortLevels)")
        check("cutline is found and its doctor passes", app.environment?.cutline != nil && app.environment?.doctor?.ok == true, "\(String(describing: app.environment?.cutline)) \(String(describing: app.environment?.doctor?.missing_required))")
        check("the helper binary is bundled", Helper.exists, Helper.path)
        await wait("key status") { app.keyStatus.present }
        check("the ElevenLabs key is only ever shown masked", app.keyStatus.masked == "••••-key", app.keyStatus.masked ?? "nil")
        check("the library is the scratch one", app.libraryURL.path.hasPrefix(scratch.path), app.libraryURL.path)
        guard app.claudeAvailable, let cutline = app.environment?.cutline?.path else { return }

        // The asset set: synthetic media from cutline's own quick self test.
        let assets = scratch.appendingPathComponent("selftest-assets")
        let status = await runProcess(cutline, ["selftest", "--quick", "--keep", "--out", assets.path])
        let footage = assets.appendingPathComponent("pipeline_shorts_render")
        check("`cutline selftest --quick` passes and leaves its assets", status == 0 && fm.fileExists(atPath: footage.appendingPathComponent("a.mp4").path), "exit \(status)")

        // MARK: Welcome and new project
        note("New project")
        delegate.showWelcome()
        await snapshot(NSApp.windows.first { $0.title == "Syncfusion Video Studio" }, "welcome")
        let projectDir = scratch.appendingPathComponent("proj")
        try? fm.createDirectory(at: projectDir, withIntermediateDirectories: true)
        let projectURL = projectDir.appendingPathComponent("Launch teaser.cutproj")
        delegate.createProject(name: "Launch teaser", intent: "marketing", intentOther: nil, template: nil, templateFile: nil, at: projectURL)
        guard let wc = delegate.controllers.first else { check("a project window opens", false); return }
        var doc = wc.doc
        check("a project window opens with the intent's template", doc.project.spec.hasPrefix("# Launch teaser: marketing video"), String(doc.project.spec.prefix(60)))
        check("the intent's suggested outputs and a run config are created", doc.project.output_configs.count == 3 && doc.project.run_configs.count == 1)
        check("the project file is JSON with the spec embedded as Markdown", (try? String(contentsOf: projectURL, encoding: .utf8))?.contains("\"spec\": \"# Launch teaser") == true)
        await wait("outline") { !doc.outline.isEmpty }
        check("the outline lists headings and scenes", doc.outline.contains { $0.kind == "scene" } && doc.outline.first?.title.hasPrefix("Launch teaser") == true)
        check("live lint flags the template's placeholders", doc.lint.contains { $0.rule == "placeholder" })
        await snapshot(wc.window, "new-project")

        // MARK: Spec editing
        note("Spec editing")
        let ed = doc.editor
        let spec = """
        # Launch teaser

        A 12-second vertical teaser.

        ## Scene 1: Hook
        - **Duration:** 4 s
        - **Media:** ![clip a](media:m-a)
        - **On-screen text:** Sharp in 60 seconds

        ```vo
        Meet the fastest way to ship.
        ```

        ## Scene 2: Steps
        - **Duration:** 8 s
        - **Media:** ![clip b](media:m-b) and ![missing](media:m-nope)

        | Step | What it shows |
        |------|---------------|
        | 1    | Setup         |

        ```mermaid
        flowchart LR
          A[Hook] --> B[Steps] --> C[Done]
        ```

        ## Call to action
        Start a free trial.

        """
        ed.replaceAllText(spec, undoable: false)
        doc.textDidChange()
        check("typing updates the project's spec", doc.project.spec == spec)
        check("the window is marked edited", doc.isDirty)
        await wait("autosave", timeout: 4) { !doc.isDirty }
        check("autosave writes the file one second after editing stops", (try? String(contentsOf: projectURL, encoding: .utf8))?.contains("Sharp in 60 seconds") == true)
        await wait("lint") { doc.lint.contains { $0.rule == "unresolved_media" } }
        check("lint reports media IDs that aren't in the project", doc.lint.filter { $0.rule == "unresolved_media" }.count == 3, "\(doc.lint.map(\.message))")
        ed.textView.setSelectedRange(NSRange(location: ed.nsText.length, length: 0))
        ed.insertSpecBlock("scene")
        check("/scene inserts a scene block and selects its first placeholder", ed.text.contains("## Scene: {{name}}") && ed.nsText.substring(with: ed.textView.selectedRange()) == "{{name}}")
        ed.textView.undoManager?.undo()
        check("the insertion is one undo step", !ed.text.contains("## Scene: {{name}}"))
        ed.goToLine(5)
        check("jumping to a line moves the caret", ed.caretLine() == 4)

        // MARK: Media by reference
        note("Media")
        let ids = doc.addMedia([footage])
        check("a folder is added recursively, by reference", ids.count >= 5 && doc.project.media.contains { $0.id == "m-a" } && doc.project.media.contains { $0.id == "m-music" }, "\(ids)")
        check("media stays where it is", doc.project.mediaItem("m-a")?.paths.absolute == footage.appendingPathComponent("a.mp4").path && !fm.fileExists(atPath: projectDir.appendingPathComponent("a.mp4").path))
        await wait("media resolution") { doc.mediaStatus("m-a") == "ok" }
        await wait("probe", timeout: 30) { doc.project.mediaItem("m-a")?.probe?.duration_s != nil }
        check("media is probed (duration, size)", (doc.project.mediaItem("m-a")?.probe?.duration_s ?? 0) > 1 && doc.project.mediaItem("m-a")?.probe?.width != nil, "\(String(describing: doc.project.mediaItem("m-a")?.probe))")
        check("kinds and roles are inferred", doc.project.mediaItem("m-music")?.kind == "audio" && doc.project.mediaItem("m-music")?.role == "music" && doc.project.mediaItem("m-logo")?.role == "logo")
        await wait("lint after media") { doc.lint.filter { $0.rule == "unresolved_media" }.count == 1 }
        check("only the unknown ID is still flagged", doc.lint.filter { $0.rule == "unresolved_media" }.count == 1)
        // A pasted image has no source file: it goes to the sidecar and is referenced from there.
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 32, pixelsHigh: 32, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        let png = rep.representation(using: .png, properties: [:])!
        let pastedID = doc.addPastedImage(png)
        let sidecar = URL(fileURLWithPath: projectURL.path + ".assets")
        let sidecarFiles = (try? fm.contentsOfDirectory(atPath: sidecar.path)) ?? []
        check("a pasted image is stored in the .cutproj.assets sidecar by content hash", pastedID != nil && sidecarFiles.count == 1 && sidecarFiles[0].hasSuffix(".png") && sidecarFiles[0].count == 68, "\(sidecarFiles)")
        _ = doc.addPastedImage(png)
        check("pasting the same picture twice stores it once", ((try? fm.contentsOfDirectory(atPath: sidecar.path)) ?? []).count == 1 && doc.project.media.filter { $0.pasted == true }.count == 1)
        doc.sidebarTab = .media
        doc.selectedMediaID = "m-a"
        await sleep(0.8)
        await snapshot(wc.window, "media")

        // MARK: Preview
        note("Preview")
        doc.showPreview = true
        var rendered = false
        doc.preview.onRendered = { rendered = true }
        await wait("preview render", timeout: 20) { rendered }
        await sleep(0.6)
        let cards = await js(doc, "document.querySelectorAll('.media-card').length") as? Int
        check("media embeds render as cards", cards == 3, "\(String(describing: cards))")
        let missing = await js(doc, "document.querySelectorAll('.media-card.status-unknown').length") as? Int
        check("an unknown media ID is shown as such", missing == 1, "\(String(describing: missing))")
        let cardText = await js(doc, "document.querySelector('.media-card .media-meta').textContent") as? String
        check("a card shows kind, duration and size", cardText?.contains("Video") == true && cardText?.contains("media:m-a") == true, cardText ?? "")
        let mermaid = await js(doc, "document.querySelectorAll('pre.mermaid svg').length") as? Int
        check("Mermaid renders offline", mermaid == 1, "\(String(describing: mermaid))")
        let vo = await js(doc, "document.querySelector('.spec-vo .spec-block-body').textContent") as? String
        check("the voice-over block is styled", vo == "Meet the fastest way to ship.", vo ?? "")
        let table = await js(doc, "document.querySelectorAll('table tr').length") as? Int
        check("tables render", table == 2, "\(String(describing: table))")
        await sleep(1.0)
        let thumb = await js(doc, "(function(){var i=document.querySelector('.media-card img');return i?i.naturalWidth:0})()") as? Int
        check("video thumbnails load through the app's own scheme", (thumb ?? 0) > 0, "\(String(describing: thumb))")
        await snapshot(web: doc.preview.webView, "preview")
        await snapshot(wc.window, "preview-window")
        doc.showPreview = false

        // MARK: Claude edit review
        note("Claude assistant")
        let specPath = app.libraryURL.appendingPathComponent("assistant/\(doc.project.id)/\(doc.assistant.current?.id ?? "")/spec.md").path
        let sharedDir = scratch.appendingPathComponent("shared-docs", isDirectory: true)
        try? FileManager.default.createDirectory(at: sharedDir, withIntermediateDirectories: true)
        try? "The brief.".write(to: sharedDir.appendingPathComponent("brief.md"), atomically: true, encoding: .utf8)
        doc.assistant.extraEnv = ["FAKE_CLAUDE_SCRIPT": writeScript("assistant.json", [
            ["say": "I'll shorten the hook and sharpen the call to action."],
            ["tool": "AskUserQuestion", "input": ["questions": [["question": "How long should the hook be?", "header": "Hook", "multiSelect": false, "options": [["label": "3 s", "description": "Punchy"], ["label": "5 s"]]]]]],
            ["tool": "Edit", "input": ["file_path": specPath, "old_string": "- **Duration:** 4 s", "new_string": "- **Duration:** 3 s"]],
            ["tool": "Edit", "input": ["file_path": specPath, "old_string": "Start a free trial.", "new_string": "Start a free trial at example.com."]],
            ["tool": "Write", "input": ["file_path": projectURL.path, "content": "{}"]],
            ["tool": "Bash", "input": ["command": "rm -rf ~"]],
            ["tool": "Read", "input": ["file_path": sharedDir.appendingPathComponent("brief.md").path]],
            ["result": ["text": "Done.", "cost": 0.03]],
        ]), "FAKE_CLAUDE_TRACE": scratch.appendingPathComponent("assistant-trace.ndjson").path]
        doc.update { $0.assistant_folders = [sharedDir.path] }
        let before = ed.text
        check("a message starts an assistant turn", doc.assistant.send("Tighten the hook."))
        await wait("question card") { doc.assistant.current?.messages.contains { $0.role == .question && $0.questionState == .pending } == true }
        doc.inspectorTab = .claude
        await snapshot(wc.window, "claude-question")
        if let q = doc.assistant.current?.messages.first(where: { $0.role == .question }) {
            doc.assistant.answer(messageID: q.id, answers: ["How long should the hook be?": "3 s"])
        }
        await wait("assistant turn", timeout: 30) { !doc.assistant.isRunning }
        let trace = (try? String(contentsOf: scratch.appendingPathComponent("assistant-trace.ndjson"), encoding: .utf8)) ?? ""
        check("the answer reaches Claude", trace.contains("\"How long should the hook be?\":\"3 s\""))
        check("writes outside spec.md and shell commands are denied", trace.components(separatedBy: "\n").filter { $0.contains("\"decision\":\"deny\"") }.count == 2, trace)
        check("a folder shared with Claude is readable and saved in the project", trace.components(separatedBy: "\n").contains { $0.contains("brief.md") && $0.contains("\"decision\":\"allow\"") } && trace.contains("shared-docs") && (try? String(contentsOf: projectURL, encoding: .utf8))?.contains("assistant_folders") == true, trace)
        check("the project file is untouched by Claude", (try? String(contentsOf: projectURL, encoding: .utf8))?.contains("\"format\"") == true)
        check("Claude's edit waits for review as hunks", doc.review.pending?.hunks.count == 2, "\(String(describing: doc.review.pending?.hunks.count))")
        check("the editor keeps the old text during review", ed.text == before && !ed.textView.isEditable)
        await snapshot(wc.window, "review")
        doc.review.decide(0, accept: true)
        doc.review.decide(1, accept: false)
        check("accepting a hunk writes it to the spec; rejecting keeps the old text", doc.review.pending == nil && doc.project.spec.contains("- **Duration:** 3 s") && doc.project.spec.contains("Start a free trial.\n") && !doc.project.spec.contains("example.com"))
        ed.textView.undoManager?.undo()
        check("the accepted change is one undo step", ed.text == before)
        ed.textView.undoManager?.redo()
        let recSummary: SummaryResult? = try? Core.run("project.summary", ["project": Core.arg(doc.project), "path": doc.fileURL.path, "run_config": doc.selectedRunConfig?.id])
        check("the assistant is told that runs can drive and record apps", recSummary?.summary.contains("App recording: on") == true, recSummary?.summary ?? "")
        check("macOS permissions for app recording can be read", (try? Core.run("capture.permissions", as: CapturePermissions.self))?.supported == true)
        check("the session is recorded in the project", doc.project.assistant_sessions.count == 1 && doc.project.assistant_sessions.first?.claude_session?.hasPrefix("fake-session") == true)

        // MARK: Relink on a moved project
        note("Relink")
        doc.save()
        wc.window?.close()
        await sleep(0.3)
        // A move on the same volume is followed silently through the bookmark (spec §6.2 step 1).
        let renamed = scratch.appendingPathComponent("selftest-assets/footage-renamed")
        try? fm.moveItem(at: footage, to: renamed)
        guard let wcMoved = delegate.open(projectURL) else { check("the project reopens", false); return }
        doc = wcMoved.doc
        await wait("bookmark resolution", timeout: 10) { doc.project.mediaItem("m-a")?.paths.absolute?.contains("footage-renamed") == true }
        check("a renamed folder is followed through the bookmark, with no relinking", doc.missingMedia.isEmpty && doc.project.mediaItem("m-a")?.paths.absolute?.contains("footage-renamed") == true && !doc.showRelinkSheet, "\(doc.missingMedia.count) missing; \(doc.project.mediaItem("m-a")?.paths.absolute ?? "")")
        doc.save()
        wcMoved.window?.close()
        await sleep(0.3)
        // A copy to another place (a new machine, another volume): bookmarks and paths are dead.
        let moved = scratch.appendingPathComponent("Volumes/Work/pipeline_shorts_render")
        try? fm.createDirectory(at: moved.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? fm.copyItem(at: renamed, to: moved)
        try? fm.removeItem(at: renamed)
        guard let wc2 = delegate.open(projectURL) else { check("the project reopens", false); return }
        doc = wc2.doc
        await wait("missing media", timeout: 10) { doc.missingMedia.count >= 5 }
        check("media that is gone from its place is reported missing", doc.missingMedia.count >= 5, "\(doc.missingMedia.count)")
        await wait("relink sheet") { doc.showRelinkSheet }
        check("the Relink sheet opens with the project", doc.showRelinkSheet)
        check("the pasted image in the sidecar still resolves", pastedID.map { doc.mediaStatus($0) == "ok" } ?? false)
        await sleep(0.6)
        await snapshot(wc2.window, "relink")
        let relinked = doc.relink("m-a", to: moved.appendingPathComponent("a.mp4"))
        check("locating one item offers the prefix fix for the rest", (relinked?.prefix_fix != nil) && (relinked?.candidates.count ?? 0) >= 4, "\(String(describing: relinked?.prefix_fix)) \(relinked?.candidates.count ?? -1)")
        if let fix = relinked?.prefix_fix { _ = doc.applyPrefixFix(fix) }
        await wait("relinked", timeout: 10) { doc.missingMedia.isEmpty && doc.mediaStatus("m-b") == "ok" }
        check("the prefix fix relinks every other item", doc.missingMedia.isEmpty && doc.project.mediaItem("m-music")?.paths.absolute?.contains("Volumes/Work") == true)
        doc.showRelinkSheet = false
        // Search Folder… after another move, including a renamed file.
        let again = scratch.appendingPathComponent("Archive/clips")
        try? fm.createDirectory(at: again.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? fm.copyItem(at: moved, to: again)
        try? fm.removeItem(at: moved)
        try? fm.moveItem(at: again.appendingPathComponent("c.mp4"), to: again.appendingPathComponent("c-final.mp4"))
        doc.resolveMedia()
        await wait("missing again") { doc.missingMedia.count >= 5 }
        let found = doc.searchFolder(scratch.appendingPathComponent("Archive"))
        await wait("search folder") { doc.missingMedia.isEmpty }
        check("Search Folder matches by name, then fingerprint (a renamed file)", found.count >= 5 && doc.missingMedia.isEmpty && doc.project.mediaItem("m-c")?.paths.absolute?.hasSuffix("c-final.mp4") == true, "\(found)")

        // MARK: Brand presets
        note("Brand presets")
        var acme: BrandPreset = (try? Core.run("brand.new", ["name": "Acme"])) ?? BrandPreset(id: "", name: "Acme")
        acme.rules = "Do: lead with the product.\nDon't: use stock music."
        acme.colors.primary = "#0A84FF"
        acme.logos.primary = try? Core.run("brand.asset", ["path": again.appendingPathComponent("logo.png").path])
        acme = (try? Core.run("brand.save", ["brand": Core.arg(acme)])) ?? acme
        var globex: BrandPreset = (try? Core.run("brand.new", ["name": "Globex"])) ?? BrandPreset(id: "", name: "Globex")
        globex = (try? Core.run("brand.save", ["brand": Core.arg(globex)])) ?? globex
        app.reloadBrands()
        check("presets are stored in the app library", app.brands.map(\.name) == ["Acme", "Globex"], "\(app.brands.map(\.name))")
        doc.setBrand(acme)
        check("choosing a preset stores its ID and a snapshot", doc.project.brand?.preset_id == acme.id && doc.project.brand?.snapshot.rules == acme.rules)
        doc.setBrand(globex)
        check("switching presets is one step", doc.project.brand?.snapshot.name == "Globex")
        doc.setBrand(acme)
        acme.rules += "\nDo: end on the logo."
        await sleep(1.1)
        _ = try? Core.raw("brand.save", ["brand": Core.arg(acme)])
        app.reloadBrands()
        let newer: Bool = (try? Core.run("brand.is_newer", ["brand": Core.arg(doc.project.brand!)])) ?? false
        check("a newer library copy is detected against the snapshot", newer)
        check("the project offers Update to Latest (warning button and brand menu)", doc.brandUpdateAvailable)
        doc.banner = .brandUpdate(name: "Acme")
        await snapshot(wc2.window, "brand-banner")
        doc.updateBrandToLatest()
        check("Update to Latest takes the library copy", doc.project.brand?.snapshot.rules.contains("end on the logo") == true && doc.banner == nil && !doc.brandUpdateAvailable)
        let exported = scratch.appendingPathComponent("acme.cutbrand")
        _ = try? Core.raw("brand.export", ["brand": Core.arg(acme), "destination": exported.path])
        check("a preset exports as .cutbrand with its assets", ((try? fm.attributesOfItem(atPath: exported.path))?[.size] as? Int ?? 0) > 200)
        delegate.showLibrary(tab: .brands, select: acme.id)
        await sleep(0.5)
        await snapshot(NSApp.windows.first { $0.title == "App Library" }, "library-brands")
        delegate.showLibrary(tab: .presets, select: nil)
        await sleep(0.4)
        await snapshot(NSApp.windows.first { $0.title == "App Library" }, "library-presets")
        NSApp.windows.first { $0.title == "App Library" }?.close()

        // MARK: Validation with blockers
        note("Validation")
        doc.update { p in
            p.output_configs = [OutputConfig(id: "out_shorts", preset: "youtube_shorts")]
            p.run_configs[0].name = "Shorts"
            p.run_configs[0].outputs = ["out_shorts"]
            p.run_configs[0].output_folder = scratch.appendingPathComponent("out").path
            p.run_configs[0].quality = "draft"
            p.run_configs[0].dry_run_first = false
            p.intent.target_length_s = 6
        }
        await sleep(0.3)
        var report = await doc.validate()
        check("the unknown media ID is a blocker", report?.issues.contains { $0.rule == "spec_unresolved_media" && $0.severity == "blocker" } == true, "\(report?.issues.map(\.rule) ?? [])")
        check("each issue links to where it can be fixed", report?.issues.first { $0.rule == "spec_unresolved_media" }?.fix?.line == 16, "\(String(describing: report?.issues.first { $0.rule == "spec_unresolved_media" }?.fix))")
        await snapshot(wc2.window, "validation-blockers")
        let findings = doc.assistant.validationContext()
        check("validation findings are ready to go to Claude with the next message", doc.assistant.includeValidation && doc.assistant.validationFindingCount == (report?.issues.count ?? -1) && findings.contains("[blocker]") && findings.contains("(spec.md line 16)"), String(findings.prefix(300)))
        if let issue = report?.issues.first(where: { $0.rule == "spec_unresolved_media" }) {
            doc.assistant.askToFix(issue: issue)
            check("Ask Claude to Fix writes the request for one finding", doc.inspectorTab == .claude && doc.assistant.draft.contains(issue.title), doc.assistant.draft)
            await sleep(0.4)
            await snapshot(wc2.window, "claude-validation-context")
            doc.assistant.draft = ""
            doc.inspectorTab = .validation
        }
        doc.runs.requestRun()
        await wait("blocked run") { !doc.runs.isPreparing }
        check("a run can't start with blockers", doc.runs.plan == nil && !doc.runs.isRunning && doc.inspectorTab == .validation)
        if let fix = report?.issues.first(where: { $0.rule == "spec_unresolved_media" })?.fix {
            doc.reveal(fix)
            check("the fix link jumps to the spec line", doc.editor.caretLine() == 15, "\(doc.editor.caretLine())")
        }
        ed2(doc).replaceAllText(doc.editor.text.replacingOccurrences(of: " and ![missing](media:m-nope)", with: ""), undoable: true)
        doc.textDidChange()
        report = await doc.validate()
        check("fixing the spec clears the blockers", report?.blockers == 0, "\(report?.issues.filter { $0.severity == "blocker" }.map(\.title) ?? [])")

        // MARK: A full run: the fake Claude drives the real cutline
        note("Run")
        let a = again.appendingPathComponent("a.mp4").path, b = again.appendingPathComponent("b.mp4").path, music = again.appendingPathComponent("music.wav").path
        let project5 = """
        {
          version: 1, name: "selftest", target: "shorts",
          assets: { a: {path: "\(a)"}, b: {path: "\(b)"}, music: {path: "\(music)", role: "music"} },
          main: [
            { id: "hook", clip: "a", in: 1.0, out: 3.0 },
            { transition: { type: "fade", duration: 0.3 } },
            { id: "step", clip: "b", in: 2.0, out: 4.0 },
          ],
          layers: [
            { type: "text", template: "hook_title", text: "Sharp in 60 seconds", start: 0, duration: 1.8, position: "lower_mid" },
            { type: "audio", asset: "music", start: 0, end: "end", gain_db: -8, fade_in: 0.3, fade_out: 0.5 },
          ],
          exports: [ {name: "youtube-shorts", target: "shorts", out: "outputs/youtube-shorts/youtube-shorts.mp4"} ],
        }
        """
        let runScript = writeScript("run.json", [
            ["say": "Reading the spec and the manifest."],
            ["tool": "Read", "input": ["file_path": "spec.md"]],
            ["tool": "Write", "input": ["file_path": "project.json5", "content": project5]],
            ["tool": "Bash", "input": ["command": "cutline validate --json project.json5"]],
            ["tool": "Bash", "input": ["command": "ls -la"]],
            ["tool": "AskUserQuestion", "input": ["questions": [["question": "Keep the music under the whole clip?", "multiSelect": false, "options": [["label": "Yes"], ["label": "No"]]]]]],
            ["tool": "Bash", "input": ["command": "cutline render project.json5 --draft"]],
            ["tool": "Write", "input": ["file_path": "summary.md", "content": "## What I made\nOne Shorts draft.\n\n## Assumptions\nNone.\n"]],
            ["result": ["text": "The draft is rendered.", "cost": 0.42]],
        ])
        doc.runs.testOptions = ["extra_env": ["FAKE_CLAUDE_SCRIPT": runScript, "FAKE_CLAUDE_TRACE": scratch.appendingPathComponent("run-trace.ndjson").path]]
        doc.runs.requestRun()
        await wait("confirmation sheet", timeout: 30) { doc.runs.plan != nil }
        check("Run validates, then shows the confirmation sheet", doc.runs.plan?.outputs.count == 1 && doc.runs.plan?.outputFolder == scratch.appendingPathComponent("out").path, "\(String(describing: doc.runs.plan?.outputFolder))")
        check("the sheet shows an approximate cost estimate", (doc.runs.plan?.report.estimates.claude_usd_high ?? 0) > 0)
        await sleep(0.6)
        await snapshot(wc2.window, "run-confirm")
        if let plan = doc.runs.plan { doc.runs.confirm(plan) }
        await wait("run question", timeout: 60) { doc.runs.question != nil }
        check("a question from Claude pauses the run and shows as a card", doc.runs.question?.questions.first?.question == "Keep the music under the whole clip?" && doc.runs.isRunning)
        check("stages follow Claude's tool calls", doc.runs.stages.first { $0.id == "validate" }?.state != .pending && doc.runs.stages.first { $0.id == "authoring" }?.state == .done, "\(doc.runs.stages.map { "\($0.id):\($0.state)" })")
        await snapshot(wc2.window, "run-question")
        doc.runs.answer(["Keep the music under the whole clip?": "Yes"])
        await wait("render stage", timeout: 30) { doc.runs.currentStage == "render" }
        await sleep(1.0)
        await snapshot(wc2.window, "run-live")
        await wait("run to finish", timeout: 240) { !doc.runs.isRunning }
        let outcome = doc.runs.lastOutcome
        check("the run succeeds", outcome?.status == "succeeded" || outcome?.status == "succeeded_with_warnings", "\(String(describing: outcome?.status)) \(outcome?.message ?? "") \(outcome?.warnings ?? [])")
        let out = outcome?.outputs.first
        check("the real cutline rendered the output into outputs/<output>/", out?.exists == true && (out?.bytes ?? 0) > 10_000 && out?.path.hasSuffix("outputs/youtube-shorts/youtube-shorts.mp4") == true, "\(String(describing: out))")
        check("the output has a QA verdict from cutline", ["pass", "warnings", "fail"].contains(out?.qa_verdict ?? ""), out?.qa_verdict ?? "")
        check("cost comes from Claude Code's result", outcome?.record.cost_usd == 0.42)
        let runTrace = (try? String(contentsOf: scratch.appendingPathComponent("run-trace.ndjson"), encoding: .utf8)) ?? ""
        check("`ls` was denied; cutline was allowed", runTrace.contains("\"command\":\"ls -la\"") && runTrace.components(separatedBy: "\n").filter { $0.contains("\"decision\":\"deny\"") }.count == 1 && runTrace.contains("Only cutline"))
        check("the answer given in the app reached Claude", runTrace.contains("\"Keep the music under the whole clip?\":\"Yes\""))
        let folder = URL(fileURLWithPath: outcome?.folder ?? "/nonexistent")
        for f in ["logs/run.json", "logs/events.ndjson", "logs/claude.jsonl", "logs/validation.json", "logs/summary.md", "logs/permissions.ndjson", "logs/cutline/001-validate.log", "logs/cutline/002-render.log", "media-manifest.json", "spec.md", "brand-preset.json", "brand/default.json"] {
            check("the run folder has \(f)", fm.fileExists(atPath: folder.appendingPathComponent(f).path))
        }
        check("the run is in the project's history", doc.project.runs.last?.status == outcome?.status && doc.project.runs.last?.folder == outcome?.folder)
        check("history is in the file on disk too", (try? Core.run("project.load", ["path": projectURL.path], as: Project.self))?.runs.count == doc.project.runs.count)
        doc.sidebarTab = .runs
        await sleep(2.0)
        await snapshot(wc2.window, "run-results")
        let report5: String = (try? Core.run("logs.issue_report", ["folder": folder.path])) ?? ""
        check("the issue report summarises the run", report5.contains("# cutline issue report") && report5.contains("## Blocked tool calls"))
        let zip = scratch.appendingPathComponent("logs.zip")
        let count: Int = (try? Core.run("logs.export", ["folder": folder.path, "destination": zip.path, "redact": true])) ?? 0
        check("logs export as a zip", count > 8 && fm.fileExists(atPath: zip.path), "\(count)")
        let logNames = Set(((try? Core.run("run.results", ["folder": folder.path], as: RunResults.self))?.logs ?? []).map(\.name))
        check("a run keeps the instructions Claude was given", logNames.contains("prompt-system.md") && logNames.contains("prompt.md"), "\(logNames.sorted())")
        let systemPrompt = (try? String(contentsOf: folder.appendingPathComponent("logs/prompt-system.md"), encoding: .utf8)) ?? ""
        check("a run is told how to finish the video", systemPrompt.contains("## Finish") && systemPrompt.contains("snap_to_beats") && systemPrompt.contains("smoothleft"), String(systemPrompt.prefix(80)))
        let unzip = Process()
        unzip.executableURL = URL(fileURLWithPath: "/usr/bin/unzip")
        unzip.arguments = ["-Z1", zip.path]
        let pipe = Pipe()
        unzip.standardOutput = pipe
        try? unzip.run()
        let listing = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        check("the export opens with an index and the issue report", listing.contains("README.md") && listing.contains("issue-report.md") && listing.contains("logs/prompt-system.md"), listing)

        // MARK: Copy and Markdown preview
        note("Copy and Markdown preview")
        let blocks = MarkdownBlocks.parse("# Title\n\nA *short* paragraph\nin two lines.\n\n- one\n  - nested\n1. first\n\n```\ncode here\n```\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n> quoted\n\n---\n")
        check("Markdown previews as headings, lists, code, tables, quotes and rules", blocks == [
            .heading(1, "Title"), .paragraph("A *short* paragraph in two lines."), .item(indent: 0, marker: "•", text: "one"), .item(indent: 1, marker: "•", text: "nested"),
            .item(indent: 0, marker: "1.", text: "first"), .code("code here"), .table([["A", "B"], ["1", "2"]]), .quote("quoted"), .rule,
        ], "\(blocks)")
        let clipboardBefore = NSPasteboard.general.string(forType: .string)
        Clipboard.copy("copied by the self test")
        check("one click puts text on the clipboard", NSPasteboard.general.string(forType: .string) == "copied by the self test")
        if let clipboardBefore { Clipboard.copy(clipboardBefore) }
        if let report = doc.report {
            let all = ValidationView.text(of: report)
            check("validation results copy as text", report.issues.isEmpty || all.contains(report.issues[0].title))
        }

        // MARK: Stop
        note("Stop")
        doc.runs.testOptions = ["extra_env": ["FAKE_CLAUDE_SCRIPT": writeScript("hang.json", [["say": "Working."], ["hang": true]])]]
        doc.runs.requestRun()
        await wait("confirmation sheet", timeout: 30) { doc.runs.plan != nil }
        if let plan = doc.runs.plan { doc.runs.confirm(plan) }
        await wait("the run to start", timeout: 30) { doc.runs.transcript.contains { $0.text == "Working." } }
        let t0 = Date()
        doc.runs.stop()
        await wait("the run to stop", timeout: 20) { !doc.runs.isRunning }
        check("Stop cancels the run within the grace period", doc.runs.lastOutcome?.status == "cancelled" && Date().timeIntervalSince(t0) < 8, "\(String(describing: doc.runs.lastOutcome?.status)) in \(Date().timeIntervalSince(t0)) s")
        check("a cancelled run keeps complete logs", fm.fileExists(atPath: URL(fileURLWithPath: doc.runs.lastOutcome?.folder ?? "/x").appendingPathComponent("logs/summary.md").path))

        // MARK: Settings
        delegate.showSettings(pane: "claude")
        await sleep(0.5)
        await snapshot(NSApp.windows.first { $0.title == "Settings" }, "settings-claude")
        delegate.showSettings(pane: "elevenlabs")
        await sleep(0.4)
        await snapshot(NSApp.windows.first { $0.title == "Settings" }, "settings-elevenlabs")
        delegate.showSettings(pane: "finish")
        await sleep(0.4)
        await snapshot(NSApp.windows.first { $0.title == "Settings" }, "settings-finish")
        check("the finish defaults are smooth and on the beat", app.settings.finish == Finish() && app.settings.finish.transitions == "smooth" && app.settings.finish.cut_on_beat)
        check("a project follows the Settings finish until it is given its own", doc.project.finish == nil)
        doc.update { $0.finish = app.settings.finish }
        doc.update { $0.finish?.transitions = "cuts" }
        doc.inspectorTab = .project
        await sleep(0.5)
        await snapshot(wc2.window, "project-finish")
        _ = doc.save()
        let savedFinish = (try? String(contentsOf: doc.fileURL, encoding: .utf8))?.contains("\"transitions\": \"cuts\"") == true
        check("a project's own finish is saved with it", savedFinish)
        doc.update { $0.finish = nil }
        doc.inspectorTab = .outputs
        await sleep(0.4)
        await snapshot(wc2.window, "outputs")
        doc.inspectorTab = .project
        await sleep(0.3)
        await snapshot(wc2.window, "project")
        // Light appearance (spec §17: full light and dark mode).
        NSApp.appearance = NSAppearance(named: .aqua)
        doc.inspectorTab = .validation
        await sleep(0.8)
        await snapshot(wc2.window, "light-validation")
        doc.showPreview = true
        await sleep(1.5)
        await snapshot(web: doc.preview.webView, "light-preview")
        doc.showPreview = false
        doc.inspectorTab = .run
        await sleep(0.8)
        await snapshot(wc2.window, "light-run")
        NSApp.appearance = nil
        doc.save()
    }

    private static func ed2(_ doc: ProjectDocument) -> EditorController { doc.editor }
}
#endif
