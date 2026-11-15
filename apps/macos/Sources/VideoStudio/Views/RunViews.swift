import SwiftUI
import AppKit
import AVKit

// MARK: Validation report (spec §11)

struct ValidationView: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject private var app = AppState.shared

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                if let r = document.report {
                    summaryChip("\(r.blockers)", "Blockers", r.blockers > 0 ? .red : .secondary)
                    summaryChip("\(r.warnings)", "Warnings", r.warnings > 0 ? .orange : .secondary)
                    summaryChip("\(r.infos)", "Notes", .secondary)
                } else {
                    Text("Not validated yet").foregroundStyle(.secondary)
                }
                Spacer()
                if document.isValidating { ProgressView().controlSize(.small) }
                Button("Validate") { Task { await document.validate() } }.disabled(document.isValidating).help("Run the static checks (⌘B)")
                Menu {
                    Button("Dry Run…") { document.runs.requestDryRun() }.disabled(!app.claudeAvailable || document.runs.isRunning)
                } label: { Image(systemName: "ellipsis.circle") }
                .menuStyle(.borderlessButton).fixedSize()
                .help("A dry run has a cheaper model write the edit and run cutline's checks, without rendering finals or generating audio")
                .accessibilityLabel("More validation actions")
            }
            .controlSize(.small)
            .padding(.horizontal, 12).padding(.vertical, 8)
            Divider()
            if let r = document.report {
                if r.issues.isEmpty {
                    EmptyState(symbol: "checkmark.seal", title: "Ready to run", message: "No problems were found.")
                } else {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 8) {
                            estimates(r)
                            if let dry = r.dry_run, !(dry.ambiguities.isEmpty && dry.risks.isEmpty && dry.missing_media.isEmpty && dry.assumptions.isEmpty) {
                                Button { document.assistant.askToFix(dry) } label: {
                                    Label("Ask Claude to fix the spec", systemImage: "sparkles").frame(maxWidth: .infinity)
                                }
                                .help("Opens a Claude session with the dry run's findings")
                                .disabled(!app.claudeAvailable)
                            }
                            ForEach(r.issues) { issue in IssueRow(document: document, issue: issue) }
                        }
                        .padding(12)
                    }
                }
            } else {
                EmptyState(symbol: "checkmark.shield", title: "Validate before you run", message: "Validation checks the tools, media, spec, brand, outputs and budget, so problems are caught before an expensive run.",
                           action: ("Validate", { Task { await document.validate() } }))
            }
        }
        // Back from System Settings: check again, so a permission that was just allowed clears.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            guard document.report?.issues.contains(where: { $0.rule == "capture_permission" }) == true, !document.runs.isRunning else { return }
            Task { await document.validate() }
        }
    }

    private func summaryChip(_ n: String, _ label: String, _ color: Color) -> some View {
        HStack(spacing: 4) {
            Text(n).font(.callout.weight(.semibold).monospacedDigit()).foregroundStyle(color)
            Text(label).font(.caption).foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder private func estimates(_ r: ValidationReport) -> some View {
        let e = r.estimates
        VStack(alignment: .leading, spacing: 3) {
            if e.claude_usd_high > 0 {
                Text("Claude: about \(Fmt.money(e.claude_usd_low))–\(Fmt.money(e.claude_usd_high)) (approximate)")
            }
            if e.eleven_credits > 0 {
                Text("ElevenLabs: about \(e.eleven_credits) credits of a \(e.eleven_cap) cap" + (e.eleven_remaining.map { " · \($0) left on the account" } ?? ""))
            }
            if e.disk_bytes > 0 {
                Text("Disk: about \(Fmt.bytes(e.disk_bytes)) needed" + (e.free_bytes.map { " · \(Fmt.bytes($0)) free" } ?? ""))
            }
            if let d = e.duration_s { Text("Planned length: \(Fmt.duration(d))" + (e.voice_over_words > 0 ? " · \(e.voice_over_words) words of voice-over" : "")) }
        }
        .font(.caption).foregroundStyle(.secondary)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(8)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
    }
}

struct IssueRow: View {
    @ObservedObject var document: ProjectDocument
    var issue: ValidationIssue

    private var symbol: String {
        switch issue.severity {
        case "blocker": return "xmark.octagon.fill"
        case "warning": return "exclamationmark.triangle.fill"
        default: return "info.circle"
        }
    }

    private var fixLabel: String {
        guard let f = issue.fix else { return "" }
        switch f.kind {
        case "spec": return f.line.map { "Line \($0)" } ?? "Spec"
        case "media": return "Media"
        case "brand": return "Brand"
        case "outputs", "run_config": return "Outputs"
        case "project": return "Project"
        case "settings": return "Settings"
        default: return "Show"
        }
    }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: symbol).foregroundStyle(StatusColor.severity(issue.severity)).frame(width: 16)
            VStack(alignment: .leading, spacing: 3) {
                Text(issue.title).font(.callout.weight(.medium)).fixedSize(horizontal: false, vertical: true)
                if let d = issue.detail { Text(d).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true).textSelection(.enabled) }
                HStack(spacing: 8) {
                    if let permission = issue.fix.flatMap({ $0.kind == "permission" ? CapturePermission(rawValue: $0.target ?? "") : nil }) {
                        // One click: the app is added to the list and System Settings opens on it.
                        Button("Allow \(permission.title)…") { permission.allow() }
                            .controlSize(.small)
                            .help("Opens System Settings with Syncfusion Video Studio in the \(permission.title) list. Turn its switch on there.")
                    } else if issue.fix != nil {
                        Button(fixLabel + " →") { document.reveal(issue.fix) }.buttonStyle(.link).font(.caption)
                            .accessibilityLabel("Go to \(fixLabel)")
                    }
                    if issue.severity == "warning" {
                        Toggle("Acknowledge", isOn: Binding(get: { document.acknowledged.contains(issue.id) }, set: { on in
                            if on { document.acknowledged.insert(issue.id) } else { document.acknowledged.remove(issue.id) }
                        }))
                        .toggleStyle(.checkbox).font(.caption).controlSize(.small)
                        .help("Accept this warning for the next run")
                    }
                }
            }
            Spacer(minLength: 0)
        }
        .padding(8)
        .background(StatusColor.severity(issue.severity).opacity(issue.severity == "info" ? 0.05 : 0.08), in: RoundedRectangle(cornerRadius: 6))
        .opacity(document.acknowledged.contains(issue.id) ? 0.6 : 1)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(issue.severity): \(issue.title)")
    }
}

// MARK: Run confirmation (spec §12.1)

struct RunConfirmSheet: View {
    @ObservedObject var document: ProjectDocument
    @State var plan: RunPlan
    @ObservedObject private var app = AppState.shared
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let warnings = plan.report.issues.filter { $0.severity == "warning" }
        VStack(alignment: .leading, spacing: 14) {
            Text(plan.revise == nil ? "Run “\(plan.runConfig.name)”" : "Revise with “\(plan.runConfig.name)”").font(.title3.weight(.semibold))
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 8) {
                GridRow {
                    Text("Outputs").foregroundStyle(.secondary)
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(plan.outputs) { o in
                            Text("\(o.name ?? o.id)  ").font(.callout) + Text("\(o.width ?? 0)×\(o.height ?? 0)").font(.caption).foregroundColor(.secondary)
                        }
                    }
                }
                GridRow {
                    Text("Model").foregroundStyle(.secondary)
                    ModelPicker(title: "", model: $plan.model).labelsHidden()
                }
                GridRow {
                    Text("Quality").foregroundStyle(.secondary)
                    Text(plan.runConfig.quality == "draft" ? "Draft" : "Final")
                }
                GridRow {
                    Text("Estimated cost").foregroundStyle(.secondary)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Claude: about \(Fmt.money(plan.report.estimates.claude_usd_low))–\(Fmt.money(plan.report.estimates.claude_usd_high)) (approximate)")
                        if plan.report.estimates.eleven_credits > 0 { Text("ElevenLabs: about \(plan.report.estimates.eleven_credits) credits (cap \(plan.runConfig.eleven_credit_cap))") }
                    }
                    .font(.callout)
                }
                GridRow {
                    Text("Output folder").foregroundStyle(.secondary)
                    Text(plan.outputFolder).font(.callout).lineLimit(2).truncationMode(.middle).textSelection(.enabled)
                }
                if plan.revise == nil {
                    GridRow {
                        Text("Dry run").foregroundStyle(.secondary)
                        Toggle("Do a dry run first with \(app.models.displayName(document.project.models.dry_run))", isOn: $plan.dryRunFirst)
                    }
                }
            }
            if let r = plan.revise {
                Text("Feedback").font(.caption).foregroundStyle(.secondary)
                Text(r.feedback).font(.callout).padding(8).frame(maxWidth: .infinity, alignment: .leading).background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
            }
            if !warnings.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Label("\(warnings.count) warning\(warnings.count == 1 ? "" : "s") will be accepted for this run", systemImage: "exclamationmark.triangle.fill").foregroundStyle(.orange).font(.callout.weight(.medium))
                    ForEach(warnings.prefix(5)) { w in Text("• " + w.title).font(.caption).foregroundStyle(.secondary).lineLimit(2) }
                    if warnings.count > 5 { Text("…and \(warnings.count - 5) more in the Validation tab").font(.caption).foregroundStyle(.secondary) }
                }
            }
            HStack {
                Button("Show Validation") {
                    document.runs.plan = nil
                    document.inspectorTab = .validation
                }
                Spacer()
                Button("Cancel") { document.runs.plan = nil }.keyboardShortcut(.cancelAction)
                Button(plan.dryRunFirst ? "Start Dry Run" : "Run") { document.runs.confirm(plan) }.keyboardShortcut(.defaultAction)
            }
        }
        .padding(22)
        .frame(width: 520)
    }
}

/// After a dry run that found something: continue to production, or stop and fix the spec.
struct DryRunFindingsSheet: View {
    @ObservedObject var document: ProjectDocument

    var body: some View {
        let findings = document.runs.dryRunFindings
        let r = findings?.readiness
        VStack(alignment: .leading, spacing: 12) {
            Label("The dry run found points to look at", systemImage: "exclamationmark.triangle.fill").font(.title3.weight(.semibold)).foregroundStyle(.orange)
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    section("Unclear in the spec", r?.ambiguities ?? [])
                    section("Missing media", r?.missing_media ?? [])
                    section("Risks", r?.risks ?? [])
                    section("Assumptions the dry run made", r?.assumptions ?? [])
                    section("Made during the production run (nothing to do)", r?.planned ?? [])
                    if let n = r?.notes, !n.isEmpty { Text(n).font(.callout).foregroundStyle(.secondary) }
                }
            }
            .frame(maxHeight: 300)
            HStack {
                Button("Ask Claude to Fix the Spec") {
                    if let r { document.assistant.askToFix(r) }
                    document.runs.dryRunFindings = nil
                }
                Spacer()
                Button("Stop Here") { document.runs.dryRunFindings = nil }.keyboardShortcut(.cancelAction)
                Button("Run Anyway") { document.runs.continueAfterDryRun() }
            }
        }
        .padding(22)
        .frame(width: 540)
    }

    @ViewBuilder private func section(_ title: String, _ items: [ReadinessFinding]) -> some View {
        if !items.isEmpty {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            ForEach(Array(items.enumerated()), id: \.offset) { _, f in
                HStack(alignment: .top, spacing: 6) {
                    Text("•")
                    VStack(alignment: .leading, spacing: 1) {
                        Text(f.text).fixedSize(horizontal: false, vertical: true)
                        if let d = f.detail { Text(d).font(.caption).foregroundStyle(.secondary) }
                    }
                    Spacer(minLength: 0)
                    if let line = f.spec_line {
                        Button("Line \(line)") {
                            document.runs.dryRunFindings = nil
                            document.jump(toLine: line)
                        }
                        .buttonStyle(.link).font(.caption)
                    }
                }
                .font(.callout)
            }
        }
    }
}

// MARK: Run inspector: live progress, then results and logs (spec §12.3, §12.5, §15)

struct RunInspector: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject private var runs: RunController

    init(document: ProjectDocument) {
        self.document = document
        self.runs = document.runs
    }

    var body: some View {
        if runs.isRunning {
            LiveRunView(document: document, runs: runs)
        } else if let id = document.selectedRunID ?? document.project.runs.last?.id, let record = document.project.runs.first(where: { $0.id == id }) {
            RunResultView(document: document, record: record).id(record.id + record.status)
        } else {
            EmptyState(symbol: "play.circle", title: "No run yet", message: "Run validates the project, then Claude Code drives cutline to produce the outputs. Progress, results and logs appear here.",
                       action: ("Run", { runs.requestRun() }))
        }
    }
}

struct LiveRunView: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject var runs: RunController
    @State private var showTranscript = true

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    HStack(alignment: .firstTextBaseline) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(runs.kind == "dry_run" ? "Dry run" : "Production run").font(.headline)
                            Text(runs.liveModel).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Text(Fmt.elapsed(runs.elapsed)).font(.title3.monospacedDigit())
                            .accessibilityLabel("Elapsed \(Fmt.elapsed(runs.elapsed))")
                    }
                    if let q = runs.question {
                        QuestionCard(questions: q.questions, state: .pending, answers: [:], onAnswer: { runs.answer($0) }, onDismiss: nil)
                        if let t = q.timeout { Text("With no answer in \(t) s, Claude proceeds on its stated assumption.").font(.caption).foregroundStyle(.secondary) }
                    }
                    stageList
                    costBox
                    if !runs.warnings.isEmpty {
                        VStack(alignment: .leading, spacing: 3) {
                            ForEach(Array(runs.warnings.enumerated()), id: \.offset) { _, w in
                                Label(w, systemImage: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(.orange)
                            }
                        }
                    }
                    DisclosureGroup("Transcript", isExpanded: $showTranscript) {
                        TranscriptView(items: runs.transcript, streaming: runs.streamingText)
                    }
                    .font(.callout)
                }
                .padding(14)
            }
            Divider()
            HStack {
                if let f = runs.liveFolder {
                    Button("Reveal Folder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: f)]) }
                }
                Spacer()
                Button(role: .destructive) { runs.stop() } label: { Label(runs.isStopping ? "Stopping…" : "Stop", systemImage: "stop.fill") }
                    .disabled(runs.isStopping)
                    .help("Interrupts Claude, then after 5 seconds stops everything it started. Partial outputs and logs are kept.")
            }
            .controlSize(.small)
            .padding(10)
        }
    }

    private var stageList: some View {
        VStack(alignment: .leading, spacing: 5) {
            ForEach(runs.stages.filter { runs.kind != "dry_run" || !["audio", "render", "qa"].contains($0.id) }) { s in
                HStack(spacing: 8) {
                    Group {
                        switch s.state {
                        case .pending: Image(systemName: "circle").foregroundStyle(.tertiary)
                        case .active: ProgressView().controlSize(.small)
                        case .done: Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                        case .failed: Image(systemName: "exclamationmark.circle.fill").foregroundStyle(.orange)
                        }
                    }
                    .frame(width: 18)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(s.label).foregroundStyle(s.state == .pending ? Color.secondary : Color.primary)
                        if s.state == .active {
                            if let f = runs.fraction { ProgressView(value: f).progressViewStyle(.linear) }
                            if !runs.progressDetail.isEmpty { Text(runs.progressDetail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle) }
                        }
                    }
                    Spacer(minLength: 0)
                    if s.seconds > 0 { Text(Fmt.elapsed(s.seconds)).font(.caption.monospacedDigit()).foregroundStyle(.secondary) }
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(s.label): \(s.state == .active ? "in progress" : s.state == .done ? "done" : s.state == .failed ? "had a failure" : "waiting")")
            }
        }
    }

    private var costBox: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text("Claude").foregroundStyle(.secondary)
                Spacer()
                Text(runs.claudeUSD.map { (runs.costIsEstimate ? "about " : "") + Fmt.money($0) } ?? "—").monospacedDigit()
            }
            .help(runs.costIsEstimate ? "Estimated from token counts while the run is in progress; the final figure comes from Claude Code." : "Reported by Claude Code")
            if runs.kind != "dry_run" {
                HStack {
                    Text("ElevenLabs credits").foregroundStyle(.secondary)
                    Spacer()
                    Text("\(runs.elevenCredits) of \(runs.elevenCap)").monospacedDigit()
                }
                ProgressView(value: min(1, Double(runs.elevenCredits) / Double(max(1, runs.elevenCap)))).progressViewStyle(.linear).tint(.orange)
            }
        }
        .font(.callout)
        .padding(10)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
    }
}

struct TranscriptView: View {
    var items: [TranscriptItem]
    var streaming: String

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            ForEach(items.suffix(200)) { item in
                switch item.role {
                case "assistant":
                    Text(item.text).font(.callout).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                case "tool":
                    Label { Text(Self.inline(item.text)) } icon: { Image(systemName: item.text.hasSuffix("failed") ? "xmark.circle" : "wrench.and.screwdriver") }.font(.caption)
                        .foregroundStyle(item.text.hasSuffix("failed") ? Color.red : Color.secondary).lineLimit(2).help(item.detail ?? "")
                case "error":
                    Label(item.text, systemImage: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(.red).textSelection(.enabled)
                default:
                    Text(item.text).font(.caption).foregroundStyle(.secondary)
                }
            }
            if !streaming.isEmpty { Text(streaming).font(.callout).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading) }
        }
        .padding(.top, 6)
    }

    /// Inline Markdown (code spans in tool summaries).
    static func inline(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(s)
    }
}

enum RunResultTab: String, CaseIterable { case results = "Results", summary = "Summary", logs = "Logs" }

struct RunResultView: View {
    @ObservedObject var document: ProjectDocument
    var record: RunRecord
    @State private var results: RunResults?
    @State private var tab: RunResultTab = .results
    @State private var showRevise = false
    @State private var feedback = ""

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 6) {
                    Image(systemName: RunStatus.symbol(record.status)).foregroundStyle(StatusColor.run(record.status))
                    Text(RunStatus.label(record.status)).font(.headline)
                    Spacer()
                    if record.kind == "dry_run" { Text("Dry run").font(.caption).padding(.horizontal, 6).background(Color(nsColor: .quaternaryLabelColor), in: Capsule()) }
                }
                Text([record.run_config_name ?? "", Fmt.when(record.started), record.model.map { AppState.shared.models.displayName($0) } ?? "", record.cost_usd.map { Fmt.money($0) } ?? "",
                      (record.eleven_credits ?? 0) > 0 ? "\(record.eleven_credits!) credits" : ""].filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(.caption).foregroundStyle(.secondary)
                if let m = results?.run?["message"]?.stringValue { Text(m).font(.callout).foregroundStyle(record.status == "failed" ? Color.red : Color.secondary).fixedSize(horizontal: false, vertical: true).textSelection(.enabled) }
                Picker("", selection: $tab) { ForEach(RunResultTab.allCases, id: \.self) { Text($0.rawValue).tag($0) } }
                    .pickerStyle(.segmented).labelsHidden().accessibilityLabel("Run detail")
            }
            .padding(12)
            Divider()
            if results?.exists == false {
                EmptyState(symbol: "questionmark.folder", title: "The run folder has moved", message: record.folder, action: ("Locate…", locate))
            } else {
                switch tab {
                case .results: resultsTab
                case .summary: summaryTab
                case .logs: LogsView(folder: URL(fileURLWithPath: record.folder), logs: results?.logs ?? [])
                }
            }
            Divider()
            HStack {
                Button("Reveal") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: record.folder)]) }
                Spacer()
                if record.kind == "production" {
                    Button("Revise…") { showRevise = true }.disabled(document.runs.isRunning || record.claude_session == nil || !AppState.shared.claudeAvailable)
                        .help("Give feedback and continue this run's Claude session in a new run folder")
                    Button("Re-run") {
                        if document.project.runConfig(record.run_config) != nil { document.selectedRunConfigID = record.run_config }
                        document.runs.requestRun()
                    }
                    .disabled(document.runs.isRunning || !AppState.shared.claudeAvailable)
                    .help("Run this run config again")
                }
            }
            .controlSize(.small)
            .padding(10)
        }
        .task(id: record.id + record.status) { load() }
        .sheet(isPresented: $showRevise) {
            VStack(alignment: .leading, spacing: 12) {
                Text("Revise this run").font(.title3.weight(.semibold))
                Text("Say what to change. Claude continues from this run's edit and audio, and writes a new run folder linked to this one.").font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                TextEditor(text: $feedback).font(.body).frame(height: 120).overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color(nsColor: .separatorColor)))
                    .accessibilityLabel("Feedback")
                HStack {
                    Spacer()
                    Button("Cancel") { showRevise = false }.keyboardShortcut(.cancelAction)
                    Button("Continue…") {
                        showRevise = false
                        if document.project.runConfig(record.run_config) != nil { document.selectedRunConfigID = record.run_config }
                        document.runs.requestRun(revise: (record.id, feedback))
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(feedback.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
            .padding(20).frame(width: 460)
        }
    }

    private func load() {
        let folder = record.folder
        Task {
            let r: RunResults? = try? await Core.async("run.results", ["folder": folder])
            results = r
            if record.kind == "dry_run" { tab = .summary }
        }
    }

    private func locate() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.message = "Choose the run folder (it contains a logs folder)."
        if panel.runModal() == .OK, let url = panel.url { document.relocateRun(record.id, to: url) }
    }

    private var resultsTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let outputs = results?.outputs, !outputs.isEmpty {
                    ForEach(outputs) { o in OutputCard(file: o) }
                } else if record.kind == "dry_run" {
                    Text("A dry run renders no outputs. Its findings are in Summary and in the Validation tab.").font(.callout).foregroundStyle(.secondary)
                    if let r = results?.readiness {
                        Button("Show Findings in Validation") { document.addDryRunFindings(r) }
                    }
                } else if results != nil {
                    Text("This run produced no outputs.").font(.callout).foregroundStyle(.secondary)
                } else {
                    ProgressView().frame(maxWidth: .infinity)
                }
                if let p = results?.project_json5 {
                    Button { NSWorkspace.shared.open(URL(fileURLWithPath: p)) } label: { Label("View project.json5", systemImage: "doc.text") }
                        .buttonStyle(.link).help("The cutline project Claude wrote for this run (read-only artifact)")
                }
            }
            .padding(12)
        }
    }

    private var summaryTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                if let r = results?.readiness {
                    Label(r.ready ? "Claude judged the spec ready for production." : "Claude judged the spec not ready yet.", systemImage: r.ready ? "checkmark.seal" : "exclamationmark.triangle")
                        .foregroundStyle(r.ready ? Color.green : Color.orange).font(.callout)
                    if !r.estimated_durations.isEmpty {
                        Text("Estimated length: " + r.estimated_durations.sorted { $0.key < $1.key }.map { "\($0.key) \(Fmt.duration($0.value))" }.joined(separator: ", ")).font(.caption).foregroundStyle(.secondary)
                    }
                    Button("Show Findings in Validation") { document.addDryRunFindings(r) }
                }
                Text(results?.summary ?? "No summary was written.").font(.system(size: 12)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(12)
        }
    }
}

/// One rendered output: player, duration, size, QA verdict, Reveal, Open With, Copy path.
struct OutputCard: View {
    var file: OutputFile
    @State private var player: AVPlayer?
    @State private var duration: Double?

    private var qaColor: Color {
        switch file.qa_verdict {
        case "pass": return .green
        case "warnings": return .orange
        case "fail": return .red
        default: return .secondary
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                VStack(alignment: .leading, spacing: 1) {
                    Text(file.output_name).font(.callout.weight(.semibold))
                    Text(file.platform + " · \(file.width)×\(file.height)").font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                if file.incomplete { Text("Incomplete").font(.caption2.weight(.semibold)).padding(.horizontal, 6).padding(.vertical, 1).background(Color.orange, in: Capsule()).foregroundStyle(.white) }
                Label("QA " + file.qa_verdict, systemImage: file.qa_verdict == "pass" ? "checkmark.seal.fill" : file.qa_verdict == "none" ? "questionmark.circle" : "exclamationmark.triangle.fill")
                    .font(.caption).foregroundStyle(qaColor)
            }
            if file.exists {
                if let player {
                    VideoPlayer(player: player).aspectRatio(CGFloat(file.width) / CGFloat(max(1, file.height)), contentMode: .fit).frame(maxWidth: .infinity, maxHeight: file.height > file.width ? 240 : 200).clipShape(RoundedRectangle(cornerRadius: 6))
                        .accessibilityLabel("Player for \(file.output_name)")
                }
                Text([duration.map(Fmt.duration) ?? "", Fmt.bytes(file.bytes), (file.path as NSString).lastPathComponent].filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                if let findings = file.qa_findings, !findings.isEmpty {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(findings, id: \.self) { f in
                            Text("• \(f.check): \(f.message)").font(.caption).foregroundStyle(f.severity.hasPrefix("err") ? Color.red : (f.severity.hasPrefix("warn") ? Color.orange : Color.secondary)).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                HStack {
                    Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: file.path)]) }
                    Menu("Open With") {
                        Button("Default App") { NSWorkspace.shared.open(URL(fileURLWithPath: file.path)) }
                        ForEach(NSWorkspace.shared.urlsForApplications(toOpen: URL(fileURLWithPath: file.path)).prefix(8), id: \.self) { app in
                            Button(app.deletingPathExtension().lastPathComponent) {
                                NSWorkspace.shared.open([URL(fileURLWithPath: file.path)], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
                            }
                        }
                    }
                    .fixedSize()
                    Button("Copy Path") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(file.path, forType: .string)
                    }
                    if let srt = file.captions_srt { Button("Captions") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: srt)]) } }
                }
                .controlSize(.small)
            } else {
                Label("This file wasn't produced.", systemImage: "xmark.circle").font(.callout).foregroundStyle(.red)
            }
        }
        .padding(10)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
        .task(id: file.path) {
            guard file.exists else { return }
            let url = URL(fileURLWithPath: file.path)
            player = AVPlayer(url: url)
            if let d = try? await AVURLAsset(url: url).load(.duration), d.seconds.isFinite, d.seconds > 0 { duration = d.seconds } else { duration = file.duration_s }
        }
        .onDisappear { player?.pause() }
    }
}

// MARK: Logs (spec §15)

struct LogsView: View {
    var folder: URL
    var logs: [LogFile]
    @State private var selected: String?
    @State private var text = ""
    @State private var search = ""
    @State private var source = "all"
    @State private var severity = "all"

    private var filtered: [LogFile] { logs.filter { source == "all" || $0.source == source } }

    private var visibleLines: [String] {
        var lines = text.components(separatedBy: "\n")
        if severity != "all" {
            let needles = severity == "error" ? ["error", "failed", "\"is_error\":true", "E_", "exit: 1", "deny"] : ["warn", "W_", "error", "failed", "E_", "deny"]
            lines = lines.filter { l in needles.contains { l.localizedCaseInsensitiveContains($0) } }
        }
        if !search.isEmpty { lines = lines.filter { $0.localizedCaseInsensitiveContains(search) } }
        return Array(lines.prefix(4000))
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Picker("Source", selection: $source) {
                    Text("All sources").tag("all")
                    ForEach(["app", "claude", "cutline", "eleven", "validation", "qa", "summary"], id: \.self) { Text($0.capitalized).tag($0) }
                }
                .labelsHidden().fixedSize()
                Picker("Severity", selection: $severity) {
                    Text("All lines").tag("all")
                    Text("Warnings and errors").tag("warning")
                    Text("Errors").tag("error")
                }
                .labelsHidden().fixedSize()
                Spacer(minLength: 0)
            }
            .controlSize(.small).padding(.horizontal, 10).padding(.top, 8)
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search this log", text: $search).textFieldStyle(.roundedBorder)
            }
            .controlSize(.small).padding(.horizontal, 10).padding(.vertical, 6)
            List(filtered, selection: $selected) { f in
                HStack {
                    Text(f.name).font(.system(size: 11.5, design: .monospaced)).lineLimit(1).truncationMode(.middle)
                    Spacer()
                    Text(Fmt.bytes(f.bytes)).font(.caption2).foregroundStyle(.secondary)
                }
                .tag(f.path as String?)
            }
            .frame(height: 130)
            Divider()
            ScrollView([.vertical, .horizontal]) {
                Text(visibleLines.joined(separator: "\n")).font(.system(size: 11, design: .monospaced)).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .topLeading).padding(8)
            }
            .background(Color(nsColor: .textBackgroundColor))
            Divider()
            HStack {
                Button("Reveal") {
                    NSWorkspace.shared.activateFileViewerSelecting([selected.map { URL(fileURLWithPath: $0) } ?? folder.appendingPathComponent("logs")])
                }
                Button("Export Logs…") { RunActions.exportLogs(folder: folder) }
                    .help("A zip of the log folder and project.json5, with optional redaction of media paths")
                Button("Copy Issue Report") { RunActions.copyIssueReport(folder: folder) }
                    .help("A Markdown summary of cutline errors, warnings and failed QA checks with log excerpts")
                Spacer(minLength: 0)
            }
            .controlSize(.small).padding(8)
        }
        .onChange(of: selected) { load() }
        .onAppear {
            if selected == nil { selected = logs.first { $0.name == "summary.md" }?.path ?? logs.first?.path }
            load()
        }
    }

    private func load() {
        guard let selected else { text = ""; return }
        let path = selected
        Task {
            let t = await Task.detached { () -> String in
                guard let h = FileHandle(forReadingAtPath: path) else { return "" }
                defer { try? h.close() }
                // Large protocol logs are shown from their start; Reveal opens the whole file.
                let data = (try? h.read(upToCount: 1_500_000)) ?? Data()
                return String(decoding: data, as: UTF8.self)
            }.value
            text = t
        }
    }
}
