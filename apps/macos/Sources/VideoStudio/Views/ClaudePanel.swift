import SwiftUI
import AppKit

/// Model picker (spec §4.2): family, then version — Latest passes the CLI alias, a version passes
/// the full model ID — plus a Custom ID field. The list comes from the core's models.json.
struct ModelPicker: View {
    var title: String
    @Binding var model: String
    /// When set, the picker offers "Settings default": an empty model, which follows this one.
    var settingsDefault: String?
    @ObservedObject private var app = AppState.shared
    @State private var customDraft = ""
    @State private var showCustom = false

    private var isListed: Bool {
        app.models.families.contains { $0.alias == model || $0.versions.contains { $0.id == model } }
    }

    var body: some View {
        Field(title) {
            Menu {
                if let settingsDefault {
                    Button { model = "" } label: { check("Settings default (\(app.models.displayName(settingsDefault)))", model.isEmpty) }
                    Divider()
                }
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
                Text(shown).lineLimit(1)
            }
            .fixedSize()
            .accessibilityLabel(title)
            .accessibilityValue(shown)
        }
        .popover(isPresented: $showCustom) {
            VStack(alignment: .leading, spacing: 8) {
                Text("Custom model ID").font(.headline)
                TextField("claude-…", text: $customDraft).textFieldStyle(.roundedBorder).frame(width: 260)
                    .onSubmit { apply() }
                Text("Passed to the provider as it is.").font(.caption).foregroundStyle(.secondary)
                HStack {
                    Spacer()
                    Button("Cancel") { showCustom = false }
                    Button("Use") { apply() }.keyboardShortcut(.defaultAction).disabled(customDraft.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
            .padding(14)
        }
    }

    private var shown: String {
        if model.isEmpty, let settingsDefault { return "Settings default (\(app.models.displayName(settingsDefault)))" }
        return app.models.displayName(model)
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

/// Codex's model picker (BUG-fond-crane-62ZP): an explicit empty choice — Codex default, or the
/// model in Settings › Codex when one is set there — then the models Codex reports, plus a custom
/// ID, which is offered even when Codex hasn't answered with a list. The label is the effective
/// model, the same one the core sends.
struct CodexModelPicker: View {
    var title: String
    /// Empty follows Settings › Codex, then Codex's own default.
    @Binding var model: String
    @ObservedObject private var app = AppState.shared
    @State private var customDraft = ""
    @State private var showCustom = false

    private var choices: [(id: String, name: String)] { app.codexModelChoices }
    private var isListed: Bool { choices.contains { $0.id == model } }
    private var emptyLabel: String { ModelResolution.codexEmptyChoiceLabel(settingsCodexModel: app.settings.codex_model, codexModels: choices) }

    var body: some View {
        Field(title) {
            Menu {
                Button { model = "" } label: { check(emptyLabel, model.trimmingCharacters(in: .whitespaces).isEmpty) }
                if !choices.isEmpty { Divider() }
                ForEach(app.codexStatus?.models ?? []) { m in
                    Button { model = m.id } label: { check(m.name + (m.is_default ? " (default)" : ""), model == m.id) }
                }
                if !model.isEmpty && !isListed {
                    Button {} label: { check(model, true) }
                }
                Divider()
                Button("Custom ID…") {
                    customDraft = isListed ? "" : model
                    showCustom = true
                }
            } label: {
                Text(shown).lineLimit(1)
            }
            .fixedSize()
            .accessibilityLabel(title.isEmpty ? "Codex model" : title)
            .accessibilityValue(shown)
        }
        .popover(isPresented: $showCustom) {
            VStack(alignment: .leading, spacing: 8) {
                Text("Custom Codex model ID").font(.headline)
                TextField("gpt-…", text: $customDraft).textFieldStyle(.roundedBorder).frame(width: 260)
                    .onSubmit { apply() }
                Text("Passed to Codex as it is.").font(.caption).foregroundStyle(.secondary)
                HStack {
                    Spacer()
                    Button("Cancel") { showCustom = false }
                    Button("Use") { apply() }.keyboardShortcut(.defaultAction).disabled(customDraft.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
            .padding(14)
        }
    }

    private var shown: String {
        let effective = ModelResolution.effective(codex: true, own: model, settingsDefault: app.settings.codex_model, claudeFallback: "")
        return ModelResolution.label(effective: effective, codex: true, codexModels: choices, claudeName: { $0 })
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

/// A model choice that follows the selected provider (BUG-fond-crane-62ZP): Claude's menu bound
/// to Claude's field, or Codex's bound to the Codex field. Each provider's choice is kept while
/// the other is selected.
struct ProviderModelPicker: View {
    var title: String
    @Binding var claude: String
    @Binding var codex: String
    /// Claude's "Settings default" choice, as `ModelPicker.settingsDefault`.
    var claudeSettingsDefault: String?
    @ObservedObject private var app = AppState.shared

    var body: some View {
        if app.usesCodex {
            CodexModelPicker(title: title, model: $codex)
        } else {
            ModelPicker(title: title, model: $claude, settingsDefault: claudeSettingsDefault)
        }
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

// MARK: Assistant panel (spec §4.3; the type keeps its old name)

struct ClaudePanel: View {
    @ObservedObject var document: ProjectDocument
    @ObservedObject var chat: AssistantModel
    @ObservedObject var chatFocus: FocusRequest
    @ObservedObject private var app = AppState.shared
    @FocusState private var composerFocused: Bool
    /// Height of the message box; the handle above the composer changes it.
    @AppStorage("claudeComposerHeight") private var composerHeight = 96.0
    @State private var dragStartHeight: Double?
    private static let minComposerHeight = 54.0

    init(document: ProjectDocument, chatFocus: FocusRequest) {
        self.document = document
        self.chat = document.assistant
        self.chatFocus = chatFocus
    }

    var body: some View {
        GeometryReader { geo in
            panel(maxComposer: max(Self.minComposerHeight, geo.size.height - 220))
        }
    }

    private func panel(maxComposer: Double) -> some View {
        VStack(spacing: 0) {
            header
            Divider()
            if !app.assistantAvailable && app.assistantChecked {
                // The provider selected in Settings › Assistant, named with the executable looked for.
                EmptyState(symbol: "terminal", title: app.assistantSetupProblem.title, message: app.assistantSetupProblem.detail,
                           action: ("Open Settings…", { AppDelegate.shared.showSettings(pane: app.usesCodex ? "codex" : "claude") }))
            } else if let session = chat.current, !session.messages.isEmpty {
                transcript(session)
            } else {
                starters
            }
            resizeHandle(maxComposer: maxComposer)
            composer(height: min(composerHeight, maxComposer))
        }
        .onChange(of: chatFocus.count) { composerFocused = true }
    }

    /// The splitter between the conversation and the message box.
    private func resizeHandle(maxComposer: Double) -> some View {
        ZStack {
            Divider()
            Capsule().fill(Color.secondary.opacity(0.45)).frame(width: 36, height: 4)
        }
        .frame(maxWidth: .infinity).frame(height: 10)
        .contentShape(Rectangle())
        .onHover { inside in if inside { NSCursor.resizeUpDown.push() } else { NSCursor.pop() } }
        .gesture(DragGesture(minimumDistance: 1, coordinateSpace: .global)
            .onChanged { v in
                let start = dragStartHeight ?? min(composerHeight, maxComposer)
                dragStartHeight = start
                composerHeight = min(max(start - v.translation.height, Self.minComposerHeight), maxComposer)
            }
            .onEnded { _ in dragStartHeight = nil })
        .help("Drag to resize the message box")
        .accessibilityElement()
        .accessibilityLabel("Message box height")
        .accessibilityValue("\(Int(composerHeight)) points")
        .accessibilityAdjustableAction { direction in
            let step = direction == .increment ? 40.0 : -40.0
            composerHeight = min(max(composerHeight + step, Self.minComposerHeight), maxComposer)
        }
    }

    private func actionRow(gatherTitle: Bool) -> some View {
        HStack(spacing: 10) {
            Button { chat.gatherMissingInfo() } label: {
                if gatherTitle {
                    Label("Gather missing info", systemImage: "questionmark.bubble").lineLimit(1)
                } else {
                    Image(systemName: "questionmark.bubble")
                }
            }
            .buttonStyle(.borderless).controlSize(.small).fixedSize()
            .disabled(chat.isRunning || !app.assistantAvailable || document.review.pending != nil)
            .help("Gather missing info: the Assistant interviews you about gaps in the spec and folds your answers in (⌃⌘G)")
            .accessibilityLabel("Gather missing info")
            Spacer(minLength: 4)
            foldersMenu
            Toggle(isOn: Binding(get: { document.project.models.web_access }, set: { v in document.update { $0.models.web_access = v } })) {
                Label("Web", systemImage: "globe").font(.caption).lineLimit(1)
            }
            .toggleStyle(.checkbox).controlSize(.small).fixedSize()
            .help("Let the Assistant search and fetch web pages in this project's assistant sessions")
        }
    }

    private var canSend: Bool {
        !chat.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && app.assistantAvailable && document.review.pending == nil
    }

    private var foldersMenu: some View {
        let folders = document.project.assistant_folders
        return Menu {
            if folders.isEmpty {
                Text("The Assistant can read only the spec and the media folders")
            } else {
                Section("The Assistant can read") {
                    ForEach(folders, id: \.self) { path in
                        Menu((path as NSString).abbreviatingWithTildeInPath) {
                            Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)]) }
                            Button("Remove") { chat.removeFolder(path) }
                        }
                    }
                }
            }
            Divider()
            Button("Add Folder…") { chat.addFolders() }
        } label: {
            Label(folders.isEmpty ? "Folders" : "Folders (\(folders.count))", systemImage: "folder.badge.plus").font(.caption)
        }
        .menuStyle(.borderlessButton).controlSize(.small).fixedSize()
        .help("Folders the Assistant can read while helping with the spec, such as briefs, docs or source material. Changes apply from the next message.")
        .accessibilityLabel("Folders shared with the Assistant")
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
            .accessibilityLabel("Assistant session")
            Spacer()
            // The model the next turn uses under the selected provider (BUG-fond-crane-62ZP).
            Text(document.effectiveModelLabel(.assistant)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                .help("The spec assistant model for \(app.providerName). Change it in the Project inspector.")
            Button { chat.newSession() } label: { Image(systemName: "square.and.pencil") }
                .buttonStyle(.borderless).help("New session (⇧⌘N)").disabled(chat.isRunning)
                .accessibilityLabel("New session")
        }
        .padding(.horizontal, 10).frame(height: 32)
    }

    private var starters: some View {
        VStack(spacing: 10) {
            Image(systemName: "sparkles").font(.system(size: 26)).foregroundStyle(.tertiary)
            Text("Improve the spec with the Assistant").font(.headline)
            Text("The Assistant edits the spec; you review each change before it is applied. It can also ask you about what's missing.")
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
        .disabled(chat.isRunning || !app.assistantAvailable)
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
                            Text("The Assistant is working…").font(.caption).foregroundStyle(.secondary)
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

    private func composer(height: Double) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            if let sel = chat.currentSelection() {
                Toggle(isOn: $chat.includeSelection) {
                    Label("Selection, lines \(sel.lines.lowerBound)–\(sel.lines.upperBound)", systemImage: "text.cursor").font(.caption)
                }
                .toggleStyle(.checkbox).controlSize(.small)
                .help("Send the selected text as context")
            }
            if chat.validationFindingCount > 0 {
                Toggle(isOn: $chat.includeValidation) {
                    Label("Validation findings (\(chat.validationFindingCount))", systemImage: "checkmark.shield").font(.caption)
                }
                .toggleStyle(.checkbox).controlSize(.small)
                .help("Send what validation found with your message, so the Assistant can fix it. Findings outside the spec are marked as such.")
            }
            if document.review.pending != nil {
                Label("Review the Assistant's changes in the editor to continue.", systemImage: "arrow.left.circle").font(.caption).foregroundStyle(.orange)
            }
            HStack(alignment: .bottom, spacing: 6) {
                // Return adds a line; ⌘Return sends (the Send button's shortcut).
                TextEditor(text: $chat.draft)
                    .font(.body)
                    .scrollContentBackground(.hidden)
                    .padding(.horizontal, 2).padding(.vertical, 5)
                    .frame(height: height)
                    .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(composerFocused ? Color.accentColor.opacity(0.7) : Color(nsColor: .separatorColor)))
                    .overlay(alignment: .topLeading) {
                        if chat.draft.isEmpty {
                            Text("Ask the Assistant to improve the spec…  ⌘Return to send").foregroundStyle(.tertiary)
                                .padding(.horizontal, 7).padding(.vertical, 5).allowsHitTesting(false)
                        }
                    }
                    .focused($composerFocused)
                    .disabled(!app.assistantAvailable)
                    .accessibilityLabel("Message to the Assistant")
                if chat.isRunning {
                    Button { chat.stop() } label: { Image(systemName: "stop.circle.fill").font(.title2) }
                        .buttonStyle(.borderless).help("Stop (⌘.)").accessibilityLabel("Stop")
                } else {
                    Button { chat.send(chat.draft) } label: { Image(systemName: "arrow.up.circle.fill").font(.title2) }
                        .buttonStyle(.borderless)
                        .keyboardShortcut(.return, modifiers: .command)
                        .disabled(!canSend)
                        .help("Send (⌘Return)").accessibilityLabel("Send")
                }
            }
            // Full labels when the panel is wide enough; otherwise Gather shrinks to its icon.
            ViewThatFits(in: .horizontal) {
                actionRow(gatherTitle: true)
                actionRow(gatherTitle: false)
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
                HStack(alignment: .top, spacing: 4) {
                    CopyButton("Copy your message") { message.text }
                    Text(message.text).textSelection(.enabled)
                        .padding(.horizontal, 10).padding(.vertical, 7)
                        .background(Color.accentColor.opacity(0.16), in: RoundedRectangle(cornerRadius: 10))
                }
                if !message.attachments.isEmpty {
                    Text(message.attachments.joined(separator: " · ")).font(.caption2).foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .accessibilityLabel("You: \(message.text)")
        case .assistant:
            MarkdownText(text: message.text)
                .accessibilityElement(children: .contain)
                .accessibilityLabel("Assistant: \(message.text)")
        case .tool:
            VStack(alignment: .leading, spacing: 3) {
                Button { showDetail.toggle() } label: {
                    HStack(spacing: 5) {
                        Image(systemName: message.failed ? "exclamationmark.circle" : "wrench.and.screwdriver").font(.caption2)
                        Text(markdown(message.text)).font(.caption).lineLimit(message.failed ? 3 : 1).multilineTextAlignment(.leading)
                        if message.detail != nil { Image(systemName: showDetail ? "chevron.down" : "chevron.right").font(.system(size: 8)) }
                    }
                    .foregroundStyle(message.failed ? Color.orange : Color.secondary)
                }
                .buttonStyle(.plain)
                .help(message.failed ? "This step didn't work. The Assistant sees the same message and usually tries again." : "")
                if showDetail, let d = message.detail {
                    Text(d).font(.system(size: 10.5, design: .monospaced)).foregroundStyle(.secondary).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .copyable("Copy the details") { d }
                        .padding(6)
                        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 5))
                }
            }
        case .note, .usage:
            Text(message.text).font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: message.role == .usage ? .trailing : .leading)
        case .error:
            Label(message.text, systemImage: "exclamationmark.triangle.fill").font(.callout).foregroundStyle(.red).textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .copyable("Copy the error") { message.text }
                .padding(8)
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

/// A question from the Assistant (AskUserQuestion): options to pick, or a free answer.
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
            Label("The Assistant has a question", systemImage: "questionmark.bubble.fill").font(.caption.weight(.semibold)).foregroundStyle(Color.accentColor)
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
                    if let onDismiss { Button("Skip") { onDismiss() }.help("Let the Assistant continue with its best judgement") }
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
