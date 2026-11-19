import AppKit
import Combine

struct ChatMessage: Identifiable, Codable, Equatable {
    enum Role: String, Codable { case user, assistant, tool, note, error, question, usage }
    enum QuestionState: String, Codable { case pending, answered, cancelled }
    var id = UUID()
    var role: Role
    var text: String
    var toolID: String?
    var detail: String?
    var failed = false
    var attachments: [String] = []
    var questionID: String?
    var questions: [ClaudeQuestion]?
    var answers: [String: String]?
    var questionState: QuestionState?
}

/// One conversation with the spec assistant. Its Claude session is resumed on every turn.
struct ChatSession: Identifiable, Codable, Equatable {
    var id = UUID().uuidString.lowercased()
    var name = "New Session"
    var claudeSession: String?
    var messages: [ChatMessage] = []
    var created = Date()
    var autoNamed = true
}

/// The Claude panel's state for one project: sessions, the composer, and the running turn
/// (spec §4.3). The Claude process and its permission broker live in the core.
@MainActor
final class AssistantModel: ObservableObject {
    unowned let document: ProjectDocument
    @Published var sessions: [ChatSession] = []
    @Published var currentID: String?
    @Published private(set) var isRunning = false
    @Published var draft = ""
    @Published var includeSelection = true
    /// Send what validation found with each message, so Claude can fix it without the findings
    /// being copied over by hand. On by default.
    @Published var includeValidation = true
    /// The next message already carries its own findings (the dry run's), so none are added.
    private var skipValidationOnce = false
    @Published var scrollTrigger = 0

    private var turn: AssistantTurn?
    private var listener: EventForwarder?
    private var runSessionID: String?
    private var streamingMessageID: UUID?
    private var stopRequested = false
    /// Extra environment for the Claude process (the self test's fake CLI).
    var extraEnv: [String: String] = [:]

    init(document: ProjectDocument) {
        self.document = document
        load()
    }

    var current: ChatSession? { sessions.first { $0.id == currentID } }

    // MARK: Persistence (transcripts live in the app library; the project keeps id, name, session)

    private var storeDir: URL {
        let dir = (try? Core.run("assistant.sessions_dir", ["project_id": document.project.id], as: String.self)) ?? NSTemporaryDirectory()
        return URL(fileURLWithPath: dir, isDirectory: true)
    }
    private var storeURL: URL { storeDir.appendingPathComponent("sessions.json") }
    private func scratch(_ id: String) -> URL { storeDir.appendingPathComponent(id, isDirectory: true) }

    private func load() {
        if let data = try? Data(contentsOf: storeURL), let list = try? JSONDecoder().decode([ChatSession].self, from: data) {
            sessions = list.map { s in
                var s = s
                // Questions left pending from a previous launch can't be answered any more.
                for i in s.messages.indices where s.messages[i].questionState == .pending { s.messages[i].questionState = .cancelled }
                return s
            }
        }
        // Sessions known to the project but not to this machine keep their Claude session.
        for ref in document.project.assistant_sessions where !sessions.contains(where: { $0.id == ref.id }) {
            sessions.append(ChatSession(id: ref.id, name: ref.name, claudeSession: ref.claude_session, autoNamed: false))
        }
        currentID = sessions.last?.id
        if sessions.isEmpty { newSession() }
    }

    func save() {
        try? FileManager.default.createDirectory(at: storeDir, withIntermediateDirectories: true)
        if let data = try? JSONEncoder().encode(sessions) { try? data.write(to: storeURL, options: .atomic) }
        let refs = sessions.filter { !$0.messages.isEmpty }.map { AssistantSessionRef(id: $0.id, name: $0.name, claude_session: $0.claudeSession) }
        if refs != document.project.assistant_sessions { document.update { $0.assistant_sessions = refs } }
    }

    func newSession() {
        if let empty = sessions.first(where: { $0.messages.isEmpty }) {
            currentID = empty.id
            return
        }
        let s = ChatSession()
        sessions.append(s)
        currentID = s.id
    }

    func rename(_ id: String, to name: String) {
        mutate(id) { $0.name = name; $0.autoNamed = false }
        save()
    }

    func delete(_ id: String) {
        guard !isRunning else { return }
        sessions.removeAll { $0.id == id }
        try? FileManager.default.removeItem(at: scratch(id))
        if currentID == id { currentID = sessions.last?.id }
        if sessions.isEmpty { newSession() }
        save()
    }

    private func mutate(_ id: String?, _ f: (inout ChatSession) -> Void) {
        guard let id, let i = sessions.firstIndex(where: { $0.id == id }) else { return }
        f(&sessions[i])
    }

    private func append(_ m: ChatMessage) {
        mutate(runSessionID ?? currentID) { $0.messages.append(m) }
        scrollTrigger += 1
    }

    private func update(_ id: UUID, _ f: (inout ChatMessage) -> Void) {
        for si in sessions.indices {
            if let mi = sessions[si].messages.firstIndex(where: { $0.id == id }) {
                f(&sessions[si].messages[mi])
                return
            }
        }
    }

    // MARK: Shared folders

    /// Lets Claude read a folder outside the project (briefs, docs, source material). Takes
    /// effect on the next message.
    func addFolders() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = true
        panel.prompt = "Share with Claude"
        panel.message = "Claude can read (never change) the folders you choose while helping with the spec."
        guard panel.runModal() == .OK else { return }
        let picked = panel.urls.map { $0.standardizedFileURL.path }
        document.update { p in
            for path in picked where !p.assistant_folders.contains(path) { p.assistant_folders.append(path) }
        }
    }

    func removeFolder(_ path: String) {
        document.update { $0.assistant_folders.removeAll { $0 == path } }
    }

    // MARK: Running

    /// The selection in the editor with its line range, if any.
    func currentSelection() -> (text: String, lines: ClosedRange<Int>)? {
        let ed = document.editor
        let sel = ed.textView.selectedRange()
        guard sel.length > 0, !document.showPreview else { return nil }
        let a = ed.scan.line(at: sel.location) + 1
        let b = ed.scan.line(at: max(sel.location, NSMaxRange(sel) - 1)) + 1
        return (ed.nsText.substring(with: sel), a...b)
    }

    private struct ValidationContext: Decodable {
        var context: String
        var request: String
    }

    /// How many findings would go with the next message: the last validation's, or the live
    /// spec notes when the project hasn't been validated yet.
    var validationFindingCount: Int {
        if let r = document.report { return r.issues.count }
        return document.lint.count
    }

    /// The findings as text for Claude. Empty when there are none.
    func validationContext() -> String {
        if let r = document.report {
            let c: ValidationContext? = try? Core.run("validation.assistant_context", ["report": Core.arg(r)])
            return c?.context ?? ""
        }
        guard !document.lint.isEmpty else { return "" }
        let lines = document.lint.map { "- [\($0.severity)] \($0.message) (spec.md line \($0.line))" }.joined(separator: "\n")
        return "Notes on the spec from the editor's live check (these can be fixed in spec.md):\n" + lines + "\n"
    }

    private func compose(_ text: String) -> (prompt: String, labels: [String]) {
        var prompt = text
        var labels: [String] = []
        var context: [String] = []
        if includeSelection, let sel = currentSelection() {
            context.append("Selected text in spec.md (lines \(sel.lines.lowerBound)–\(sel.lines.upperBound)):\n\n```markdown\n\(sel.text)\n```")
            labels.append("Selection \(sel.lines.lowerBound)–\(sel.lines.upperBound)")
        }
        if includeValidation, !skipValidationOnce {
            let findings = validationContext()
            if !findings.isEmpty {
                context.append(findings + "\nThis is background: act on it when my message asks for fixes or when a finding bears on what I asked; otherwise just keep it in mind.")
                let n = validationFindingCount
                labels.append("Validation: \(n) finding\(n == 1 ? "" : "s")")
            }
        }
        skipValidationOnce = false
        if !context.isEmpty {
            prompt += "\n\n---\nContext from the app:\n\n" + context.joined(separator: "\n\n")
        }
        return (prompt, labels)
    }

    /// Validation › Ask Claude to Fix: opens the Claude tab with the request written, for every
    /// finding or for one. The findings themselves go along as context when it is sent.
    func askToFix(issue: ValidationIssue? = nil) {
        guard !isRunning, let r = document.report else { return }
        var args: Core.Args = ["report": Core.arg(r)]
        if let issue { args["only"] = issue.id }
        let c: ValidationContext? = try? Core.run("validation.assistant_context", args)
        includeValidation = true
        skipValidationOnce = false
        document.inspectorTab = .claude
        draft = c?.request ?? "Fix the validation findings that can be fixed in the spec."
    }

    /// Sends a prompt in the current session. Returns false if the turn didn't start.
    @discardableResult
    func send(_ rawText: String, displayText: String? = nil) -> Bool {
        let text = rawText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !isRunning else { return false }
        guard AppState.shared.claudeAvailable || !extraEnv.isEmpty else {
            Prompt.error("Claude Code isn't available.", detail: "Install the Claude Code CLI, then choose Re-check in Settings › Claude.")
            return false
        }
        if document.review.pending != nil {
            Prompt.error("Review Claude's last change first.", detail: "Accept or reject the pending changes to the spec before sending another message.")
            return false
        }
        guard document.save() else { return false }
        if currentID == nil { newSession() }
        guard let session = current else { return false }
        let composed = compose(text)
        runSessionID = session.id
        if session.autoNamed && session.messages.isEmpty {
            let name = String((displayText ?? text).replacingOccurrences(of: "\n", with: " ").prefix(48))
            mutate(session.id) { $0.name = name; $0.autoNamed = false }
        }
        append(ChatMessage(role: .user, text: displayText ?? text, attachments: composed.labels))
        draft = ""

        let p = document.project
        // Read as plain JSON: `media` goes back to the core untouched (HEIC photos in it get
        // JPEG copies Claude can open).
        let summaryJSON = (try? Core.raw("project.summary", ["project": Core.arg(p), "path": document.fileURL.path, "run_config": document.selectedRunConfig?.id]))
            .flatMap { $0.data(using: .utf8) }
            .flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
        let summary = summaryJSON.flatMap { j in (j["summary"] as? String).map { SummaryResult(summary: $0, read_paths: j["read_paths"] as? [String] ?? []) } }
        var request: [String: Any] = [
            "session_dir": scratch(session.id).path,
            "spec": document.editor.text,
            "prompt": composed.prompt,
            "model": document.assistantModel,
            "web_access": p.models.web_access,
            "read_paths": summary?.read_paths ?? [],
            "reference_paths": p.assistant_folders,
            "project_summary": summary?.summary ?? "",
            "media": summaryJSON?["media"] as? [Any] ?? [],
            "extra_env": extraEnv,
        ]
        if let e = p.models.effort, !e.isEmpty { request["effort"] = e }
        if let s = session.claudeSession { request["resume"] = s }
        if Helper.exists { request["helper"] = Helper.path }
        let json = String(data: (try? JSONSerialization.data(withJSONObject: request)) ?? Data(), encoding: .utf8) ?? "{}"
        let forwarder = EventForwarder { [weak self] line in self?.handle(line) }
        do {
            turn = try startAssistant(requestJson: json, listener: forwarder)
        } catch {
            append(ChatMessage(role: .error, text: "Couldn't start Claude: \(error.localizedDescription)"))
            runSessionID = nil
            return false
        }
        listener = forwarder
        streamingMessageID = nil
        stopRequested = false
        isRunning = true
        save()
        return true
    }

    /// Asks Claude to interview the user about gaps in the spec and fold the answers in.
    func gatherMissingInfo() {
        let prompt = (try? Core.run("assistant.gather_info_prompt", as: String.self)) ?? "Interview me about what the spec is missing, then update it."
        send(prompt, displayText: "Gather missing info")
    }

    /// Opens a session pre-filled with the dry run's findings (spec §11.2).
    func askToFix(_ readiness: ReadinessReport) {
        guard !isRunning else { return }
        newSession()
        let prompt = (try? Core.run("validation.fix_prompt", ["readiness": Core.arg(readiness)], as: String.self)) ?? ""
        document.inspectorTab = .claude
        draft = prompt
        skipValidationOnce = true
    }

    func stop() {
        guard isRunning else { return }
        stopRequested = true
        turn?.stop()
        cancelPendingQuestions()
    }

    private func cancelPendingQuestions() {
        for si in sessions.indices {
            for mi in sessions[si].messages.indices where sessions[si].messages[mi].questionState == .pending {
                sessions[si].messages[mi].questionState = .cancelled
            }
        }
    }

    // MARK: Events

    func handle(_ line: String) {
        guard let e = Core.decode(AssistantEvent.self, from: line) else { return }
        switch e.event {
        case "started":
            mutate(runSessionID) { $0.claudeSession = e.session_id }
        case "text_delta":
            let t = e.text ?? ""
            if let id = streamingMessageID {
                update(id) { $0.text += t }
            } else {
                let m = ChatMessage(role: .assistant, text: t)
                streamingMessageID = m.id
                append(m)
            }
            scrollTrigger += 1
        case "text":
            if let id = streamingMessageID {
                update(id) { $0.text = e.text ?? "" }
                streamingMessageID = nil
            } else {
                append(ChatMessage(role: .assistant, text: e.text ?? ""))
            }
        case "tool":
            streamingMessageID = nil
            append(ChatMessage(role: .tool, text: e.summary ?? e.name ?? "Tool", toolID: e.id, detail: e.detail))
        case "tool_result":
            guard e.is_error == true, let id = e.id else { break }
            for si in sessions.indices {
                if let mi = sessions[si].messages.firstIndex(where: { $0.toolID == id && $0.role == .tool }) {
                    sessions[si].messages[mi].failed = true
                    sessions[si].messages[mi].detail = e.detail
                    if let text = e.text, !text.isEmpty { sessions[si].messages[mi].text = text }
                }
            }
        case "blocked":
            append(ChatMessage(role: .note, text: "Blocked: " + (e.reason ?? "")))
        case "question":
            var m = ChatMessage(role: .question, text: (e.questions ?? []).map(\.question).joined(separator: "\n"))
            m.questionID = e.id
            m.questions = e.questions
            m.questionState = .pending
            append(m)
            NSApp.requestUserAttention(.informationalRequest)
            if document.inspectorTab != .claude { document.inspectorTab = .claude }
        case "spec_changed":
            if let spec = e.spec { document.review.propose(spec) }
        case "usage":
            guard AppSettings.shared.showClaudeUsage else { break }
            var s = "\(tokens(e.input_tokens ?? 0)) in · \(tokens(e.output_tokens ?? 0)) out"
            if let c = e.cost_usd { s += String(format: " · $%.4f", c) }
            if let d = e.duration_ms { s += String(format: " · %.1fs", Double(d) / 1000) }
            append(ChatMessage(role: .usage, text: s))
        case "error":
            if !stopRequested { append(ChatMessage(role: .error, text: e.message ?? "Claude reported an error.")) }
        case "finished":
            if let s = e.session_id { mutate(runSessionID) { $0.claudeSession = s } }
            cancelPendingQuestions()
            if stopRequested { append(ChatMessage(role: .note, text: "Stopped.")) }
            isRunning = false
            turn = nil
            listener = nil
            streamingMessageID = nil
            runSessionID = nil
            save()
        default:
            break
        }
    }

    private func tokens(_ n: UInt64) -> String { n >= 1000 ? String(format: "%.1fk", Double(n) / 1000) : "\(n)" }

    /// Sends the answers for a question card: question text → answer.
    func answer(messageID: UUID, answers: [String: String]) {
        guard let msg = sessions.flatMap(\.messages).first(where: { $0.id == messageID }), msg.questionState == .pending, let qid = msg.questionID, let turn else { return }
        update(messageID) { $0.answers = answers; $0.questionState = .answered }
        let json = String(data: (try? JSONSerialization.data(withJSONObject: answers)) ?? Data(), encoding: .utf8) ?? "{}"
        try? turn.answer(questionId: qid, answersJson: json)
        save()
    }

    func dismissQuestion(messageID: UUID) {
        guard let msg = sessions.flatMap(\.messages).first(where: { $0.id == messageID }), msg.questionState == .pending, let qid = msg.questionID else { return }
        update(messageID) { $0.questionState = .cancelled }
        turn?.dismiss(questionId: qid)
    }
}
