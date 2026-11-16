import AppKit
import Combine
import UserNotifications

struct StageState: Identifiable, Equatable {
    enum State { case pending, active, done, failed }
    var id: String
    var label: String
    var state: State = .pending
    var seconds: Double = 0
}

struct TranscriptItem: Identifiable, Equatable {
    let id = UUID()
    var role: String
    var text: String
    var tool: String?
    var detail: String?
    /// A tool call that came back as an error; the run goes on.
    var failed = false
}

struct PendingRunQuestion: Equatable {
    var id: String
    var questions: [ClaudeQuestion]
    var timeout: Int?
}

/// What the confirmation sheet shows before a run starts (spec §12.1).
struct RunPlan: Equatable {
    var runConfig: RunConfig
    var outputs: [ResolvedOutput]
    var report: ValidationReport
    var outputFolder: String
    var model: String
    var dryRunFirst: Bool
    var revise: (runID: String, feedback: String)?

    static func == (a: RunPlan, b: RunPlan) -> Bool { a.runConfig == b.runConfig && a.model == b.model && a.dryRunFirst == b.dryRunFirst && a.revise?.runID == b.revise?.runID }
}

/// Drives runs for one project (spec §12): validate, confirm, optional dry run, production,
/// live progress, questions, cancellation, and the outcome.
@MainActor
final class RunController: ObservableObject {
    unowned let document: ProjectDocument

    @Published private(set) var isRunning = false
    /// `dry_run` or `production` while running.
    @Published private(set) var kind = "production"
    @Published private(set) var stages: [StageState] = RunController.freshStages()
    @Published private(set) var currentStage: String?
    @Published private(set) var fraction: Double?
    @Published private(set) var progressDetail = ""
    @Published private(set) var startedAt: Date?
    @Published private(set) var elapsed: Double = 0
    @Published private(set) var claudeUSD: Double?
    @Published private(set) var costIsEstimate = true
    @Published private(set) var inputTokens: UInt64 = 0
    @Published private(set) var outputTokens: UInt64 = 0
    @Published private(set) var elevenCredits: UInt64 = 0
    @Published private(set) var elevenCap: UInt64 = 0
    @Published private(set) var transcript: [TranscriptItem] = []
    @Published private(set) var streamingText = ""
    @Published private(set) var question: PendingRunQuestion?
    @Published private(set) var warnings: [String] = []
    @Published private(set) var liveFolder: String?
    @Published private(set) var liveModel = ""
    @Published private(set) var isStopping = false
    @Published private(set) var lastOutcome: RunOutcome?
    @Published private(set) var statusLine = ""
    /// Set while the confirmation sheet is up.
    @Published var plan: RunPlan?
    /// Set when a dry run found points to look at; the user chooses whether to go on.
    @Published var dryRunFindings: (plan: RunPlan, readiness: ReadinessReport)?
    @Published private(set) var isPreparing = false

    private var run: Run?
    private var listener: EventForwarder?
    private var timer: Timer?
    private var continuation: RunPlan?
    /// Extra environment and options for the core (the self test's fake CLI and server).
    var testOptions: [String: Any] = [:]

    init(document: ProjectDocument) { self.document = document }

    static func freshStages() -> [StageState] {
        [("prepare", "Prepare"), ("audio", "Audio"), ("capture", "Recording"), ("authoring", "Authoring"), ("validate", "Validate"), ("review", "Review"), ("render", "Render"), ("qa", "QA"), ("done", "Done")].map { StageState(id: $0.0, label: $0.1) }
    }

    // MARK: Starting

    /// Run (⌘R): validates, then shows the confirmation sheet.
    func requestRun(revise: (runID: String, feedback: String)? = nil) {
        guard !isRunning, !isPreparing else { return }
        guard let rc = document.selectedRunConfig else {
            Prompt.error("There is no run config.", detail: "Add one in the Outputs inspector.")
            return
        }
        guard AppState.shared.canStartRun(projectID: document.project.id) else {
            Prompt.error("Another run is in progress.", detail: "Wait for it to finish, or raise the parallel run limit in Settings › General.")
            return
        }
        isPreparing = true
        Task {
            defer { isPreparing = false }
            guard let report = await document.validate(kind: "production", show: false) else { return }
            if report.blockers > 0 {
                document.inspectorTab = .validation
                NSSound.beep()
                return
            }
            let outputs = document.resolvedOutputs.filter { rc.outputs.contains($0.id) }
            plan = RunPlan(runConfig: rc, outputs: outputs, report: report, outputFolder: document.outputRoot(for: rc), model: rc.production_model, dryRunFirst: rc.dry_run_first && revise == nil, revise: revise)
        }
    }

    /// The user confirmed the sheet. Warnings shown there count as acknowledged for this run.
    func confirm(_ plan: RunPlan) {
        self.plan = nil
        document.acknowledged.formUnion(plan.report.issues.filter { $0.severity == "warning" }.map(\.id))
        if plan.dryRunFirst {
            continuation = plan
            launch(plan, kind: "dry_run")
        } else {
            launch(plan, kind: "production")
        }
    }

    /// Dry run only (Validate › Dry Run).
    func requestDryRun() {
        guard !isRunning, !isPreparing, let rc = document.selectedRunConfig else { return }
        isPreparing = true
        Task {
            defer { isPreparing = false }
            guard let report = await document.validate(kind: "dry_run", show: false) else { return }
            if report.blockers > 0 {
                document.inspectorTab = .validation
                NSSound.beep()
                return
            }
            let outputs = document.resolvedOutputs.filter { rc.outputs.contains($0.id) }
            continuation = nil
            launch(RunPlan(runConfig: rc, outputs: outputs, report: report, outputFolder: "", model: document.dryRunModel, dryRunFirst: false, revise: nil), kind: "dry_run")
        }
    }

    /// Continue to production after reviewing dry run findings.
    func continueAfterDryRun() {
        guard let f = dryRunFindings else { return }
        dryRunFindings = nil
        launch(f.plan, kind: "production")
    }

    private func launch(_ plan: RunPlan, kind: String) {
        guard document.save() else { return }
        var request: [String: Any] = [
            "project_path": document.fileURL.path,
            "run_config": plan.runConfig.id,
            "kind": kind,
            "assume_answers": false,
            "validation": Core.arg(plan.report),
        ]
        if kind == "production", plan.model != plan.runConfig.production_model { request["model"] = plan.model }
        if let r = plan.revise, kind == "production" { request["revise"] = ["previous_run": r.runID, "feedback": r.feedback] }
        if let t = plan.runConfig.question_timeout_s, t > 0 { request["question_timeout_s"] = t }
        var options: [String: Any] = ["helper": Helper.path, "eleven_key_present": AppState.shared.keyStatus.present,
                                      "cartesia_key_present": AppState.shared.cartesiaKeyStatus.present]
        for (k, v) in testOptions { options[k] = v }
        let toJSON: ([String: Any]) -> String = { String(data: (try? JSONSerialization.data(withJSONObject: $0)) ?? Data(), encoding: .utf8) ?? "{}" }
        let forwarder = EventForwarder { [weak self] line in self?.handle(line) }

        reset(kind: kind, cap: plan.runConfig.eleven_credit_cap)
        statusLine = kind == "dry_run" ? "Starting the dry run…" : "Starting…"
        isRunning = true
        AppState.shared.activeRuns.insert(document.project.id)
        document.inspectorTab = .run
        let requestJSON = toJSON(request), optionsJSON = toJSON(options)
        Task {
            do {
                // Finding the tools and preparing the folder takes a moment; keep it off the main thread.
                let run = try await Task.detached { try startRun(requestJson: requestJSON, optionsJson: optionsJSON, listener: forwarder) }.value
                self.run = run
                self.listener = forwarder
                self.liveFolder = run.folder()
                self.document.runHistoryChanged()
                self.document.selectedRunID = run.runId()
                let outcomeJSON = await Task.detached { run.wait() }.value
                self.finished(Core.decode(RunOutcome.self, from: outcomeJSON), plan: plan)
            } catch {
                self.transcript.append(TranscriptItem(role: "error", text: error.localizedDescription))
                self.statusLine = "Couldn't start"
                self.endRun()
                self.continuation = nil
                Prompt.error("The run couldn't start.", detail: error.localizedDescription)
            }
        }
    }

    private func reset(kind: String, cap: UInt64) {
        self.kind = kind
        stages = Self.freshStages()
        currentStage = nil
        fraction = nil
        progressDetail = ""
        startedAt = Date()
        elapsed = 0
        claudeUSD = nil
        costIsEstimate = true
        inputTokens = 0
        outputTokens = 0
        elevenCredits = 0
        elevenCap = cap
        transcript = []
        streamingText = ""
        question = nil
        warnings = []
        liveFolder = nil
        isStopping = false
        lastOutcome = nil
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in
                guard let self, let s = self.startedAt, self.isRunning else { return }
                self.elapsed = Date().timeIntervalSince(s)
            }
        }
    }

    private func endRun() {
        isRunning = false
        isStopping = false
        run = nil
        listener = nil
        question = nil
        timer?.invalidate()
        timer = nil
        AppState.shared.activeRuns.remove(document.project.id)
        NSApp.dockTile.badgeLabel = AppState.shared.activeRuns.isEmpty ? nil : NSApp.dockTile.badgeLabel
    }

    // MARK: Events

    func handle(_ line: String) {
        guard let e = Core.decode(RunEvent.self, from: line) else { return }
        switch e.event {
        case "run_started":
            liveFolder = e.folder
            liveModel = e.model ?? ""
            statusLine = kind == "dry_run" ? "Dry run" : "Running"
        case "stage_started":
            guard let stage = e.stage else { break }
            for i in stages.indices where stages[i].state == .active { stages[i].state = .done }
            if let i = stages.firstIndex(where: { $0.id == stage }) {
                stages[i].state = stage == "done" ? .done : .active
                if let label = e.label, stage == "render" { stages[i].label = label }
            }
            currentStage = stage
            fraction = nil
            progressDetail = ""
            if stage != "done" { statusLine = e.label ?? stage.capitalized }
        case "stage_finished":
            if let i = stages.firstIndex(where: { $0.id == e.stage }) {
                stages[i].state = e.ok == false ? .failed : .done
                stages[i].seconds += e.seconds ?? 0
            }
        case "progress":
            fraction = e.fraction
            if let d = e.detail { progressDetail = d }
        case "text_delta":
            streamingText += e.text ?? ""
        case "message":
            streamingText = ""
            transcript.append(TranscriptItem(role: e.role ?? "note", text: e.text ?? "", tool: e.tool, detail: e.detail, failed: e.failed ?? false))
            if transcript.count > 600 { transcript.removeFirst(transcript.count - 600) }
        case "question":
            guard let id = e.id else { break }
            question = PendingRunQuestion(id: id, questions: e.questions ?? [], timeout: e.timeout_s)
            statusLine = "Waiting for your answer"
            document.inspectorTab = .run
            notify(title: "Claude has a question", body: (e.questions ?? []).first?.question ?? "The run is waiting for an answer.")
            NSApp.dockTile.badgeLabel = "?"
        case "question_answered":
            if question?.id == e.id { question = nil }
            if NSApp.dockTile.badgeLabel == "?" { NSApp.dockTile.badgeLabel = nil }
        case "warning":
            if let m = e.message { warnings.append(m) }
        case "cost":
            claudeUSD = e.claude_usd
            costIsEstimate = e.claude_estimated ?? true
            inputTokens = e.input_tokens ?? inputTokens
            outputTokens = e.output_tokens ?? outputTokens
            elevenCredits = e.eleven_credits ?? elevenCredits
            if let cap = e.eleven_cap { elevenCap = cap }
        case "run_finished":
            statusLine = RunStatus.label(e.status ?? "")
            if let c = e.cost_usd { claudeUSD = c; costIsEstimate = false }
            if let s = e.seconds { elapsed = s }
        default:
            break
        }
    }

    private func finished(_ outcome: RunOutcome?, plan: RunPlan) {
        let wasKind = kind
        endRun()
        lastOutcome = outcome
        document.runHistoryChanged()
        guard let outcome else { return }
        document.selectedRunID = outcome.run_id
        statusLine = RunStatus.label(outcome.status)
        let title = wasKind == "dry_run" ? "Dry run: \(RunStatus.label(outcome.status).lowercased())" : "\(plan.runConfig.name): \(RunStatus.label(outcome.status).lowercased())"
        notify(title: title, body: outcome.message ?? "\(document.displayName) — \(outcome.outputs.filter(\.exists).count) output(s)")
        if !NSApp.isActive { NSApp.dockTile.badgeLabel = outcome.status == "failed" ? "!" : "✓" }

        guard wasKind == "dry_run" else { return }
        let next = continuation
        continuation = nil
        if let r = outcome.readiness { document.addDryRunFindings(r) }
        guard outcome.status != "failed", outcome.status != "cancelled" else {
            document.inspectorTab = .run
            return
        }
        guard let next else {
            document.inspectorTab = .validation
            return
        }
        // A clean dry run goes straight on; findings wait for a decision.
        let r = outcome.readiness
        let clean = (r?.ambiguities.isEmpty ?? true) && (r?.missing_media.isEmpty ?? true) && (r?.risks.isEmpty ?? true) && (r?.ready ?? true)
        if clean {
            launch(next, kind: "production")
        } else if let r {
            document.inspectorTab = .validation
            dryRunFindings = (next, r)
        }
    }

    // MARK: Control

    /// Stop: an interrupt, then after five seconds the whole process tree is killed (spec §12.4).
    func stop() {
        guard isRunning, let run else { return }
        isStopping = true
        statusLine = "Stopping…"
        continuation = nil
        run.cancel()
    }

    func answer(_ answers: [String: String]) {
        guard let q = question, let run else { return }
        let json = String(data: (try? JSONSerialization.data(withJSONObject: answers)) ?? Data(), encoding: .utf8) ?? "{}"
        try? run.answer(questionId: q.id, answersJson: json)
        question = nil
        statusLine = kind == "dry_run" ? "Dry run" : "Running"
    }

    private func notify(title: String, body: String) {
        guard AppState.shared.settings.notifications, !NSApp.isActive else { return }
        NSApp.requestUserAttention(.informationalRequest)
        let center = UNUserNotificationCenter.current()
        center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
            guard granted else { return }
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = body
            content.sound = .default
            center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
        }
    }

    // MARK: Results

    /// Results of any run in the history, read from its folder.
    func results(for record: RunRecord) -> RunResults? {
        try? Core.run("run.results", ["folder": record.folder])
    }
}
