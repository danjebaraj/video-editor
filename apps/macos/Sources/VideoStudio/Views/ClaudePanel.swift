import SwiftUI
import AppKit

/// Model picker (spec §4.2): family, then version — Latest passes the CLI alias, a version passes
/// the full model ID — plus a Custom ID field. The list comes from the core's models.json.
struct ModelPicker: View {
    var title: String
    @Binding var model: String
    @ObservedObject private var app = AppState.shared
    @State private var customDraft = ""
    @State private var showCustom = false

    private var isListed: Bool {
        app.models.families.contains { $0.alias == model || $0.versions.contains { $0.id == model } }
    }

    var body: some View {
        Field(title) {
            Menu {
                ForEach(app.models.families) { family in
                    Menu(family.name) {
                        Button { model = family.alias } label: { check("Latest", model == family.alias) }
                        Divider()
                        ForEach(family.versions) { v in
                            Button { model = v.id } label: { check(v.name, model == v.id) }
                        }
                    }
                }
                Divider()
                Button("Custom ID…") {
                    customDraft = isListed ? "" : model
                    showCustom = true
                }
            } label: {
                Text(app.models.displayName(model)).lineLimit(1)
            }
            .fixedSize()
            .accessibilityLabel(title)
            .accessibilityValue(app.models.displayName(model))
        }
        .popover(isPresented: $showCustom) {
            VStack(alignment: .leading, spacing: 8) {
                Text("Custom model ID").font(.headline)
                TextField("claude-…", text: $customDraft).textFieldStyle(.roundedBorder).frame(width: 260)
                    .onSubmit { apply() }
                Text("Passed to Claude Code as it is.").font(.caption).foregroundStyle(.secondary)
                HStack {
                    Spacer()
                    Button("Cancel") { showCustom = false }
                    Button("Use") { apply() }.keyboardShortcut(.defaultAction).disabled(customDraft.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
            .padding(14)
        }
    }

    private func apply() {
        let v = customDraft.trimmingCharacters(in: .whitespaces)
        if !v.isEmpty { model = v }
        showCustom = false
    }

    @ViewBuilder private func check(_ title: String, _ on: Bool) -> some View {
        if on { Label(title, systemImage: "checkmark") } else { Text(title) }
    }
}

/// Effort level, shown only when `claude --help` lists `--effort` (spec §4.2).
struct EffortPicker: View {
    @Binding var effort: String?
    @ObservedObject private var app = AppState.shared

    var body: some View {
        if !app.effortLevels.isEmpty {
            Picker("Effort", selection: Binding(get: { effort ?? "" }, set: { effort = $0.isEmpty ? nil : $0 })) {
                Text("Default").tag("")
                ForEach(app.effortLevels, id: \.self) { Text($0.capitalized).tag($0) }
            }
        }
    }
}

// MARK: Claude panel (spec §4.3)

struct ClaudePanel: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject var chat: AssistantModel
    @ObservedObject var chatFocus: FocusRequest
    @ObservedObject private var app = AppState.shared
    @FocusState private var composerFocused: Bool

    init(document: ProjectDocument, chatFocus: FocusRequest) {
        self.document = document
        self.chat = document.assistant
        self.chatFocus = chatFocus
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if !app.claudeAvailable && app.environment != nil {
                EmptyState(symbol: "terminal", title: "Claude Code isn't available", message: app.environment?.claude == nil ? "Install the Claude Code CLI to get help with the spec." : "This version of Claude Code is too old. Update it and re-check.",
                           action: ("Open Settings…", { AppDelegate.shared.showSettings(pane: "claude") }))
            } else if let session = chat.current, !session.messages.isEmpty {
                transcript(session)
            } else {
                starters
            }
            Divider()
            composer
        }
        .onChange(of: chatFocus.count) { composerFocused = true }
    }

    private var header: some View {
        HStack(spacing: 6) {
            Menu {
                ForEach(chat.sessions.reversed()) { s in
                    Button { chat.currentID = s.id } label: {
                        if s.id == chat.currentID { Label(s.name, systemImage: "checkmark") } else { Text(s.name) }
                    }
                }
                Divider()
                Button("New Session") { chat.newSession() }.disabled(chat.isRunning)
                if let s = chat.current {
                    Button("Rename…") {
                        if let name = Prompt.text(title: "Rename Session", message: "", initial: s.name) { chat.rename(s.id, to: name) }
                    }
                    Button("Delete Session", role: .destructive) {
                        if Prompt.confirm("Delete “\(s.name)”?", message: "The conversation is removed permanently.", ok: "Delete", destructive: true) { chat.delete(s.id) }
                    }
                    .disabled(chat.isRunning)
                }
            } label: {
                Text(chat.current?.name ?? "Session").lineLimit(1)
            }
            .menuStyle(.borderlessButton)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityLabel("Claude session")
            Spacer()
            Text(app.models.displayName(document.project.models.assistant)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                .help("The spec assistant model. Change it in the Project inspector.")
            Button { chat.newSession() } label: { Image(systemName: "square.and.pencil") }
                .buttonStyle(.borderless).help("New session (⇧⌘N)").disabled(chat.isRunning)
                .accessibilityLabel("New session")
        }
        .padding(.horizontal, 10).frame(height: 32)
    }

    private var starters: some View {
        VStack(spacing: 10) {
            Image(systemName: "sparkles").font(.system(size: 26)).foregroundStyle(.tertiary)
            Text("Improve the spec with Claude").font(.headline)
            Text("Claude edits the spec; you review each change before it is applied. It can also ask you about what's missing.")
                .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
            VStack(spacing: 6) {
                starter("Gather missing info", "questionmark.bubble") { chat.gatherMissingInfo() }
                starter("Tighten the script to the target length", "timer") { chat.send("Tighten the voice-over and scenes so the video fits the target length. Keep the key message and the call to action.") }
                starter("Check the spec against the attached media", "photo.on.rectangle") { chat.send("Compare the spec with the attached media. Point out scenes with no suitable media, media that isn't used, and anything the footage can't support. Then fix what you can in the spec.") }
            }
            .padding(.top, 4)
        }
        .padding(18)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func starter(_ title: String, _ symbol: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: symbol).frame(maxWidth: .infinity, alignment: .leading)
        }
        .controlSize(.large)
        .disabled(chat.isRunning || !app.claudeAvailable)
    }

    private func transcript(_ session: ChatSession) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    ForEach(session.messages) { m in
                        MessageView(message: m, chat: chat).id(m.id)
                    }
                    if chat.isRunning {
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.small)
                            Text("Claude is working…").font(.caption).foregroundStyle(.secondary)
                        }
                        .id("working")
                    }
                    Color.clear.frame(height: 1).id("end")
                }
                .padding(10)
            }
            .onChange(of: chat.scrollTrigger) { proxy.scrollTo("end", anchor: .bottom) }
            .onChange(of: chat.currentID) { proxy.scrollTo("end", anchor: .bottom) }
            .onAppear { proxy.scrollTo("end", anchor: .bottom) }
        }
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let sel = chat.currentSelection() {
                Toggle(isOn: $chat.includeSelection) {
                    Label("Selection, lines \(sel.lines.lowerBound)–\(sel.lines.upperBound)", systemImage: "text.cursor").font(.caption)
                }
                .toggleStyle(.checkbox).controlSize(.small)
                .help("Send the selected text as context")
            }
            if document.review.pending != nil {
                Label("Review Claude's changes in the editor to continue.", systemImage: "arrow.left.circle").font(.caption).foregroundStyle(.orange)
            }
            HStack(alignment: .bottom, spacing: 6) {
                TextField("Ask Claude to improve the spec…", text: $chat.draft, axis: .vertical)
                    .lineLimit(1...8)
                    .textFieldStyle(.roundedBorder)
                    .focused($composerFocused)
                    .onSubmit { chat.send(chat.draft) }
                    .disabled(!app.claudeAvailable)
                    .accessibilityLabel("Message to Claude")
                if chat.isRunning {
                    Button { chat.stop() } label: { Image(systemName: "stop.circle.fill").font(.title2) }
                        .buttonStyle(.borderless).help("Stop (⌘.)").accessibilityLabel("Stop")
                } else {
                    Button { chat.send(chat.draft) } label: { Image(systemName: "arrow.up.circle.fill").font(.title2) }
                        .buttonStyle(.borderless)
                        .disabled(chat.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !app.claudeAvailable || document.review.pending != nil)
                        .help("Send (Return)").accessibilityLabel("Send")
                }
            }
            HStack(spacing: 10) {
                Button { chat.gatherMissingInfo() } label: { Label("Gather missing info", systemImage: "questionmark.bubble") }
                    .buttonStyle(.borderless).controlSize(.small)
                    .disabled(chat.isRunning || !app.claudeAvailable || document.review.pending != nil)
                    .help("Claude interviews you about gaps in the spec and folds your answers in (⌃⌘G)")
                Spacer()
                Toggle(isOn: Binding(get: { document.project.models.web_access }, set: { v in document.update { $0.models.web_access = v } })) {
                    Label("Web", systemImage: "globe").font(.caption)
                }
                .toggleStyle(.checkbox).controlSize(.small)
                .help("Let Claude search and fetch web pages in this project's assistant sessions")
            }
        }
        .padding(10)
    }
}

struct MessageView: View {
    var message: ChatMessage
    @ObservedObject var chat: AssistantModel
    @State private var showDetail = false

    var body: some View {
        switch message.role {
        case .user:
            VStack(alignment: .trailing, spacing: 3) {
                Text(message.text).textSelection(.enabled)
                    .padding(.horizontal, 10).padding(.vertical, 7)
                    .background(Color.accentColor.opacity(0.16), in: RoundedRectangle(cornerRadius: 10))
                if !message.attachments.isEmpty {
                    Text(message.attachments.joined(separator: " · ")).font(.caption2).foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .accessibilityLabel("You: \(message.text)")
        case .assistant:
            Text(markdown(message.text)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityLabel("Claude: \(message.text)")
        case .tool:
            VStack(alignment: .leading, spacing: 3) {
                Button { showDetail.toggle() } label: {
                    HStack(spacing: 5) {
                        Image(systemName: message.failed ? "xmark.circle" : "wrench.and.screwdriver").font(.caption2)
                        Text(markdown(message.text)).font(.caption).lineLimit(1)
                        if message.detail != nil { Image(systemName: showDetail ? "chevron.down" : "chevron.right").font(.system(size: 8)) }
                    }
                    .foregroundStyle(message.failed ? Color.red : Color.secondary)
                }
                .buttonStyle(.plain)
                if showDetail, let d = message.detail {
                    Text(d).font(.system(size: 10.5, design: .monospaced)).foregroundStyle(.secondary).textSelection(.enabled)
                        .padding(6).frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 5))
                }
            }
        case .note, .usage:
            Text(message.text).font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: message.role == .usage ? .trailing : .leading)
        case .error:
            Label(message.text, systemImage: "exclamationmark.triangle.fill").font(.callout).foregroundStyle(.red).textSelection(.enabled)
                .padding(8).frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.red.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
        case .question:
            QuestionCard(questions: message.questions ?? [], state: message.questionState ?? .cancelled, answers: message.answers ?? [:],
                         onAnswer: { chat.answer(messageID: message.id, answers: $0) }, onDismiss: { chat.dismissQuestion(messageID: message.id) })
        }
    }

    private func markdown(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(s)
    }
}

/// A question from Claude (AskUserQuestion): options to pick, or a free answer.
struct QuestionCard: View {
    var questions: [ClaudeQuestion]
    var state: ChatMessage.QuestionState
    var answers: [String: String]
    var onAnswer: ([String: String]) -> Void
    var onDismiss: (() -> Void)?
    @State private var picked: [String: Set<String>] = [:]
    @State private var other: [String: String] = [:]

    private func answer(for q: ClaudeQuestion) -> String {
        var parts = q.options.map(\.label).filter { picked[q.question]?.contains($0) == true }
        let free = (other[q.question] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !free.isEmpty { parts.append(free) }
        return parts.joined(separator: ", ")
    }

    private var complete: Bool { questions.allSatisfy { !answer(for: $0).isEmpty } }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("Claude has a question", systemImage: "questionmark.bubble.fill").font(.caption.weight(.semibold)).foregroundStyle(Color.accentColor)
            ForEach(Array(questions.enumerated()), id: \.offset) { _, q in
                VStack(alignment: .leading, spacing: 5) {
                    if let h = q.header, !h.isEmpty { Text(h.uppercased()).font(.caption2.weight(.semibold)).foregroundStyle(.secondary) }
                    Text(q.question).font(.callout.weight(.medium)).fixedSize(horizontal: false, vertical: true)
                    if state == .pending {
                        ForEach(q.options, id: \.self) { o in
                            let on = picked[q.question]?.contains(o.label) == true
                            Button {
                                var set = picked[q.question] ?? []
                                if q.multi_select == true {
                                    if on { set.remove(o.label) } else { set.insert(o.label) }
                                } else {
                                    set = on ? [] : [o.label]
                                }
                                picked[q.question] = set
                            } label: {
                                HStack(alignment: .top, spacing: 6) {
                                    Image(systemName: q.multi_select == true ? (on ? "checkmark.square.fill" : "square") : (on ? "largecircle.fill.circle" : "circle"))
                                        .foregroundStyle(on ? Color.accentColor : Color.secondary)
                                    VStack(alignment: .leading, spacing: 1) {
                                        Text(o.label)
                                        if let d = o.description, !d.isEmpty { Text(d).font(.caption).foregroundStyle(.secondary) }
                                    }
                                    Spacer(minLength: 0)
                                }
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel(o.label + (on ? ", selected" : ""))
                        }
                        TextField(q.options.isEmpty ? "Your answer" : "Something else…", text: Binding(get: { other[q.question] ?? "" }, set: { other[q.question] = $0 }))
                            .textFieldStyle(.roundedBorder).controlSize(.small)
                    } else if state == .answered {
                        Label(answers[q.question] ?? "", systemImage: "checkmark").font(.callout).foregroundStyle(.secondary)
                    } else {
                        Text("Not answered").font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
            if state == .pending {
                HStack {
                    if let onDismiss { Button("Skip") { onDismiss() }.help("Let Claude continue with its best judgement") }
                    Spacer()
                    Button("Answer") {
                        var out: [String: String] = [:]
                        for q in questions { out[q.question] = answer(for: q) }
                        onAnswer(out)
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!complete)
                }
                .controlSize(.small)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.accentColor.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Color.accentColor.opacity(0.35)))
    }
}
