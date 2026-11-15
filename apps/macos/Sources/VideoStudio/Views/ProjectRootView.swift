import SwiftUI
import AppKit
import WebKit

/// The project window's content: sidebar, the spec (editor, preview or review), inspector.
struct ProjectRootView: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject var findBar: FindBarModel
    @ObservedObject var chatFocus: FocusRequest
    @ObservedObject private var app = AppState.shared
    @ObservedObject private var runs: RunController

    init(document: ProjectDocument, findBar: FindBarModel, chatFocus: FocusRequest) {
        self.document = document
        self.findBar = findBar
        self.chatFocus = chatFocus
        self.runs = document.runs
    }

    var body: some View {
        HSplitView {
            SidebarView(document: document)
                .frame(minWidth: 210, idealWidth: 250, maxWidth: 380)
            CenterView(document: document, findBar: findBar)
                .frame(minWidth: 380)
                .layoutPriority(1)
            InspectorView(document: document, chatFocus: chatFocus)
                .frame(minWidth: 320, idealWidth: 380, maxWidth: 560)
        }
        .toolbar { toolbar }
        .sheet(isPresented: $document.showRelinkSheet) { RelinkSheet(document: document) }
        .sheet(item: planBinding) { plan in RunConfirmSheet(document: document, plan: plan.value) }
        .sheet(isPresented: findingsBinding) { DryRunFindingsSheet(document: document) }
        .overlay { if app.claudeMissing { ClaudeSetupOverlay() } }
    }

    private var planBinding: Binding<IdentifiedPlan?> {
        Binding(get: { runs.plan.map(IdentifiedPlan.init) }, set: { if $0 == nil { runs.plan = nil } })
    }

    private var findingsBinding: Binding<Bool> {
        Binding(get: { runs.dryRunFindings != nil }, set: { if !$0 { runs.dryRunFindings = nil } })
    }

    // MARK: Toolbar: [Brand ▾] [Run config ▾] [Validate] [Run ▶] [Stop ■]  status · cost

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .navigation) {
            Menu {
                Button("None") { document.setBrand(nil) }
                Divider()
                ForEach(app.brands) { b in
                    Button { document.setBrand(b) } label: {
                        if document.project.brand?.preset_id == b.id { Label(b.name, systemImage: "checkmark") } else { Text(b.name) }
                    }
                }
                if document.brandUpdateAvailable, let name = document.project.brand?.snapshot.name {
                    Divider()
                    Button("Update “\(name)” to Latest") { document.updateBrandToLatest() }
                }
                Divider()
                Button("Edit Brand Presets…") { AppDelegate.shared.showLibrary(tab: .brands, select: document.project.brand?.preset_id) }
            } label: {
                Label(document.project.brand?.snapshot.name ?? "No Brand", systemImage: "paintpalette")
                    .labelStyle(.titleAndIcon)
            }
            .help("Brand preset: logos, colors, fonts, voice and rules given to Claude")
            .accessibilityLabel("Brand preset")

            Menu {
                ForEach(document.project.run_configs) { rc in
                    Button { document.selectedRunConfigID = rc.id } label: {
                        if document.selectedRunConfig?.id == rc.id { Label(rc.name, systemImage: "checkmark") } else { Text(rc.name) }
                    }
                }
                Divider()
                Button("Edit Run Configs…") { document.inspectorTab = .outputs }
            } label: {
                Label(document.selectedRunConfig?.name ?? "No Run Config", systemImage: "rectangle.stack")
                    .labelStyle(.titleAndIcon)
            }
            .help("Run config: which outputs to make, with which model, into which folder")
            .accessibilityLabel("Run config")
        }
        ToolbarItemGroup(placement: .primaryAction) {
            RunStatusChip(document: document, runs: runs)
            Button {
                Task { await document.validate() }
            } label: {
                Label("Validate", systemImage: "checkmark.shield")
            }
            .help("Check the project before a run (⌘B)")
            .disabled(document.isValidating || runs.isRunning)

            if runs.isRunning {
                Button { runs.stop() } label: { Label("Stop", systemImage: "stop.fill") }
                    .help("Stop the run (⌘.)")
                    .disabled(runs.isStopping)
            } else {
                Button { runs.requestRun() } label: { Label("Run", systemImage: "play.fill") }
                    .help(app.claudeAvailable ? "Validate, then produce the outputs (⌘R)" : "Claude Code is required to run")
                    .disabled(!app.claudeAvailable || runs.isPreparing)
            }
            Button { document.showPreview.toggle() } label: {
                Label(document.showPreview ? "Source" : "Preview", systemImage: document.showPreview ? "pencil" : "eye")
            }
            .help(document.showPreview ? "Show the Markdown source (⌘/)" : "Show the rendered preview (⌘/)")
        }
    }
}

struct IdentifiedPlan: Identifiable {
    var value: RunPlan
    var id: String { value.runConfig.id + (value.revise?.runID ?? "") }
}

/// Run state and cost, shown in the toolbar.
struct RunStatusChip: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject var runs: RunController

    var body: some View {
        if runs.isRunning || runs.isPreparing {
            Button { document.inspectorTab = .run } label: {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text(runs.isPreparing ? "Validating…" : runs.statusLine).lineLimit(1)
                    if runs.isRunning {
                        Text(Fmt.elapsed(runs.elapsed)).monospacedDigit().foregroundStyle(.secondary)
                        if let c = runs.claudeUSD { Text(Fmt.money(c)).monospacedDigit().foregroundStyle(.secondary).help(runs.costIsEstimate ? "Estimated from token counts while the run is in progress" : "Reported by Claude Code") }
                    }
                }
                .font(.callout)
            }
            .help("Show the run")
            .accessibilityLabel("Run status: \(runs.statusLine)")
        } else if let r = document.project.runs.last {
            Button { document.selectedRunID = r.id; document.inspectorTab = .run } label: {
                HStack(spacing: 5) {
                    Image(systemName: RunStatus.symbol(r.status)).foregroundStyle(StatusColor.run(r.status))
                    Text(RunStatus.label(r.status)).lineLimit(1)
                    if let c = r.cost_usd { Text(Fmt.money(c)).monospacedDigit().foregroundStyle(.secondary) }
                }
                .font(.callout)
            }
            .help("Last run: \(Fmt.when(r.started))")
        }
    }
}

enum StatusColor {
    static func run(_ status: String) -> Color {
        switch status {
        case "succeeded": return .green
        case "succeeded_with_warnings": return .orange
        case "failed": return .red
        case "cancelled": return .secondary
        default: return .accentColor
        }
    }
    static func media(_ status: String) -> Color {
        switch status {
        case "missing", "unknown": return .red
        case "changed": return .orange
        default: return .secondary
        }
    }
    static func severity(_ s: String) -> Color {
        switch s {
        case "blocker": return .red
        case "warning": return .orange
        default: return .secondary
        }
    }
}

// MARK: Centre: banner, find bar, editor / preview / review, status bar

struct CenterView: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject var findBar: FindBarModel
    @ObservedObject private var review: SpecReview
    @ObservedObject private var status: EditorStatus

    init(document: ProjectDocument, findBar: FindBarModel) {
        self.document = document
        self.findBar = findBar
        self.review = document.review
        self.status = document.editor.status
    }

    var body: some View {
        VStack(spacing: 0) {
            if let banner = document.banner { BannerView(document: document, banner: banner) }
            if document.showFindBar && !document.showPreview && review.pending == nil {
                FindBarView(model: findBar, workspace: document)
                Divider()
            }
            ZStack {
                EditorHost(editor: document.editor)
                    .opacity(document.showPreview || review.pending != nil ? 0 : 1)
                    .accessibilityHidden(document.showPreview || review.pending != nil)
                if document.showPreview && review.pending == nil { PreviewHost(preview: document.preview) }
                if let pending = review.pending { ReviewView(review: review, pending: pending) }
            }
            Divider()
            statusBar
        }
        .background(Color(nsColor: .textBackgroundColor))
    }

    private var statusBar: some View {
        HStack(spacing: 12) {
            if let message = status.message {
                Text(message).lineLimit(1)
            } else if !document.lint.isEmpty {
                let warnings = document.lint.filter { $0.severity == "warning" }
                Menu {
                    ForEach(document.lint) { issue in
                        Button("Line \(issue.line): \(issue.message)") { document.jump(toLine: issue.line) }
                    }
                } label: {
                    Label("\(document.lint.count) spec \(document.lint.count == 1 ? "note" : "notes")", systemImage: warnings.isEmpty ? "info.circle" : "exclamationmark.triangle")
                        .foregroundStyle(warnings.isEmpty ? Color.secondary : Color.orange)
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
                .help("Live checks on the spec. They don't block anything.")
            }
            Spacer()
            if status.busyCount > 0 { ProgressView().controlSize(.mini) }
            Text("\(status.words) words · \(Fmt.duration(Double(status.words) / Double(max(1, AppState.shared.settings.words_per_minute)) * 60))").lineLimit(1).fixedSize()
                .help(status.isSelection ? "The selection" : "The whole spec, read aloud at \(AppState.shared.settings.words_per_minute) words per minute")
            Text("Ln \(status.line), Col \(status.column)").monospacedDigit()
            Text(document.isDirty ? "Edited" : "Saved").foregroundStyle(document.isDirty ? Color.orange : Color.secondary)
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .frame(height: 24)
        .background(.bar)
    }
}

struct BannerView: View {
    @ObservedObject var document: ProjectDocument
    var banner: Banner

    var body: some View {
        HStack(spacing: 10) {
            switch banner {
            case .externalChange:
                Image(systemName: "arrow.triangle.2.circlepath").foregroundStyle(.orange)
                Text("This project changed on disk while you have unsaved edits.")
                Spacer()
                Button("Compare") { document.compareWithDisk() }
                Button("Reload") { document.reloadFromDisk() }
                Button("Keep Mine") { document.keepMine() }
            case .brandUpdate(let name):
                Image(systemName: "paintpalette").foregroundStyle(Color.accentColor)
                Text("The brand preset “\(name)” has been updated in the library. This project uses its snapshot.")
                Spacer()
                Button("Update to Latest") { document.updateBrandToLatest() }
                Button("Keep Snapshot") { document.banner = nil }
            case .error(let message):
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.red)
                Text(message).lineLimit(2)
                Spacer()
                Button("Dismiss") { document.banner = nil }
            }
        }
        .font(.callout)
        .controlSize(.small)
        .padding(.horizontal, 12).padding(.vertical, 7)
        .background(.bar)
        .overlay(alignment: .bottom) { Divider() }
    }
}

/// The TextKit 2 source editor.
struct EditorHost: NSViewRepresentable {
    let editor: EditorController

    func makeNSView(context: Context) -> NSScrollView { editor.scrollView }

    func updateNSView(_ view: NSScrollView, context: Context) {
        DispatchQueue.main.async { editor.fitToContainer() }
    }
}

struct PreviewHost: NSViewRepresentable {
    let preview: PreviewController
    func makeNSView(context: Context) -> WKWebView { preview.webView }
    func updateNSView(_ view: WKWebView, context: Context) {}
}

// MARK: Review of Claude's edits (spec §4.3)

struct ReviewView: View {
    @ObservedObject var review: SpecReview
    var pending: SpecReview.Pending

    var body: some View {
        VStack(spacing: 0) {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 10) {
                    reviewTitle
                    Spacer()
                    reviewButtons
                }
                VStack(alignment: .leading, spacing: 8) {
                    reviewTitle
                    HStack(spacing: 8) { reviewButtons }
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.bar)
            Divider()
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    ForEach(Array(pending.hunks.enumerated()), id: \.offset) { index, hunk in
                        HunkCard(index: index, hunk: hunk, decision: pending.decisions[index], base: pending.base, review: review)
                    }
                }
                .padding(16)
            }
        }
        .background(Color(nsColor: .textBackgroundColor))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Review Claude's changes")
    }

    private var reviewTitle: some View {
        HStack(spacing: 10) {
            Image(systemName: "sparkles").foregroundStyle(Color.accentColor)
            VStack(alignment: .leading, spacing: 1) {
                Text("Claude changed the spec").font(.headline).lineLimit(1)
                Text("\(pending.hunks.count) change\(pending.hunks.count == 1 ? "" : "s") · \(pending.accepted) accepted · \(pending.undecided) to review")
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            .fixedSize()
        }
    }

    @ViewBuilder private var reviewButtons: some View {
        Button("Reject All") { review.rejectAll() }
        Button("Accept All") { review.acceptAll() }.keyboardShortcut(.defaultAction)
        Button("Done") { review.finish() }.help("Apply the accepted changes and keep the old text for the rest")
    }
}

struct HunkCard: View {
    var index: Int
    var hunk: Hunk
    var decision: Bool?
    var base: String
    var review: SpecReview

    private var context: (before: [String], after: [String]) {
        let lines = base.components(separatedBy: "\n")
        let start = max(0, hunk.old_start - 2)
        let end = min(lines.count, hunk.old_start + hunk.old_lines.count + 2)
        let afterStart = min(lines.count, hunk.old_start + hunk.old_lines.count)
        return (Array(lines[start..<min(hunk.old_start, lines.count)]), Array(lines[afterStart..<max(afterStart, end)]))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Change \(index + 1) · line \(hunk.old_start + 1)").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                Spacer()
                if let decision {
                    Label(decision ? "Accepted" : "Rejected", systemImage: decision ? "checkmark.circle.fill" : "xmark.circle.fill")
                        .font(.caption).foregroundStyle(decision ? Color.green : Color.secondary)
                }
                Button("Reject") { review.decide(index, accept: false) }.disabled(decision == false)
                Button("Accept") { review.decide(index, accept: true) }.disabled(decision == true)
            }
            .controlSize(.small)
            .padding(.horizontal, 10).padding(.vertical, 6)
            .background(Color(nsColor: .controlBackgroundColor))
            Divider()
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(context.before.enumerated()), id: \.offset) { _, l in line(" ", l, nil) }
                ForEach(Array(hunk.old_lines.enumerated()), id: \.offset) { _, l in line("−", l, Color.red) }
                ForEach(Array(hunk.new_lines.enumerated()), id: \.offset) { _, l in line("+", l, Color.green) }
                ForEach(Array(context.after.enumerated()), id: \.offset) { _, l in line(" ", l, nil) }
            }
            .padding(.vertical, 4)
        }
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color(nsColor: .separatorColor)))
        .opacity(decision == false ? 0.55 : 1)
    }

    private func line(_ mark: String, _ text: String, _ tint: Color?) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Text(mark).frame(width: 12).foregroundStyle(tint ?? Color.secondary)
            Text(text.isEmpty ? " " : text).frame(maxWidth: .infinity, alignment: .leading).textSelection(.enabled)
        }
        .font(.system(size: 12.5, design: .monospaced))
        .padding(.horizontal, 10).padding(.vertical, 1.5)
        .background((tint ?? Color.clear).opacity(0.13))
        .foregroundStyle(tint == nil ? Color.secondary : Color.primary)
        .accessibilityLabel((mark == "+" ? "Added: " : mark == "−" ? "Removed: " : "") + text)
    }
}

// MARK: Claude Code missing (spec §4.1)

/// Shown over the window when Claude Code isn't installed. The spec can still be edited after
/// dismissing it; every Claude action, model-based validation step and run stays disabled.
struct ClaudeSetupOverlay: View {
    @ObservedObject private var app = AppState.shared
    @State private var dismissed = false

    var body: some View {
        if !dismissed {
            ZStack {
                Color.black.opacity(0.35).ignoresSafeArea()
                VStack(spacing: 14) {
                    Image(systemName: "terminal").font(.system(size: 40)).foregroundStyle(Color.accentColor)
                    Text("Claude Code is required").font(.title2.weight(.semibold))
                    Text("Syncfusion Video Studio uses the Claude Code command-line tool to help write specs and to produce videos. It wasn't found on this Mac.")
                        .multilineTextAlignment(.center).foregroundStyle(.secondary).frame(maxWidth: 420)
                    VStack(alignment: .leading, spacing: 6) {
                        Text("1. Install it in Terminal:").font(.callout)
                        Text("curl -fsSL https://claude.ai/install.sh | bash").font(.system(.callout, design: .monospaced)).textSelection(.enabled)
                            .padding(8).frame(maxWidth: .infinity, alignment: .leading).background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
                        Text("2. Run `claude` once and sign in. The app never asks for your Anthropic credentials.").font(.callout)
                        Text("3. Come back and choose Re-check. If Claude Code is installed somewhere unusual, set its path in Settings › Claude.").font(.callout)
                    }
                    .frame(maxWidth: 440, alignment: .leading)
                    HStack {
                        Button("Edit the Spec Anyway") { dismissed = true }
                        Button("Settings…") { AppDelegate.shared.showSettings(pane: "claude") }
                        Button {
                            app.detect(force: true)
                        } label: {
                            if app.isDetecting { ProgressView().controlSize(.small) } else { Text("Re-check") }
                        }
                        .keyboardShortcut(.defaultAction)
                    }
                    .padding(.top, 4)
                }
                .padding(28)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
                .shadow(radius: 24)
            }
        }
    }
}
