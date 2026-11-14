import SwiftUI
import AppKit
import MarkdownCore

/// Find & Replace state for the current editor (match case, whole word, regex).
@MainActor
final class FindBarModel: ObservableObject {
    weak var windowController: ProjectWindowController?
    @Published var query = "" { didSet { if query != oldValue { refreshMatches(keepIndex: false) } } }
    @Published var replacement = ""
    @Published var matchCase = false { didSet { refreshMatches(keepIndex: false) } }
    @Published var wholeWord = false { didSet { refreshMatches(keepIndex: false) } }
    @Published var useRegex = false { didSet { refreshMatches(keepIndex: false) } }
    @Published private(set) var matchCount = 0
    @Published private(set) var currentIndex = 0
    @Published private(set) var regexError: String?
    @Published var focusRequested = 0

    private var editor: EditorController? { windowController?.currentEditor }

    private var findBarOpen: Bool { windowController?.doc.showFindBar ?? false }
    private var editWork: DispatchWorkItem?

    /// Recomputes matches shortly after the note's text changes.
    func textChanged() {
        guard !(editor?.findMatches.isEmpty ?? true) else { return }
        editWork?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.refreshMatches(keepIndex: true) }
        editWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3, execute: work)
    }

    func regex() -> NSRegularExpression? {
        guard !query.isEmpty else { return nil }
        var pattern = useRegex ? query : NSRegularExpression.escapedPattern(for: query)
        if wholeWord { pattern = "\\b(?:" + pattern + ")\\b" }
        do {
            let r = try NSRegularExpression(pattern: pattern, options: matchCase ? [.anchorsMatchLines] : [.caseInsensitive, .anchorsMatchLines])
            regexError = nil
            return r
        } catch {
            regexError = "Invalid pattern"
            return nil
        }
    }

    func useSelectionIfAny() {
        guard let ed = editor else { return }
        let sel = ed.textView.selectedRange()
        if sel.length > 0 && sel.length < 200 {
            let s = ed.nsText.substring(with: sel)
            if !s.contains("\n") { query = useRegex ? NSRegularExpression.escapedPattern(for: s) : s }
        }
        refreshMatches(keepIndex: false)
    }

    func refreshMatches(keepIndex: Bool, includeHiddenFind: Bool = false) {
        guard let ed = editor else { return }
        let text = ed.text
        let matches: [NSRange]
        if let rx = regex(), includeHiddenFind || findBarOpen {
            matches = rx.matches(in: text, range: NSRange(location: 0, length: (text as NSString).length)).map(\.range).filter { $0.length > 0 }
        } else {
            ed.setFindMatches([], current: 0)
            matchCount = 0
            currentIndex = 0
            return
        }
        var index = ed.findIndex
        if !keepIndex {
            let caret = ed.textView.selectedRange().location
            index = matches.firstIndex { $0.location >= caret } ?? 0
        }
        index = min(index, max(matches.count - 1, 0))
        ed.setFindMatches(matches, current: index)
        matchCount = matches.count
        currentIndex = matchCount == 0 ? 0 : index + 1
    }

    func close() {
        guard let ed = editor else { return }
        // Search matches come back once the Find bar's own matches go away.
        DispatchQueue.main.async { [weak self] in self?.refreshMatches(keepIndex: false) }
        ed.focus()
    }

    func select(_ index: Int) {
        guard let ed = editor, !ed.findMatches.isEmpty else { NSSound.beep(); return }
        let next = (index + ed.findMatches.count) % ed.findMatches.count
        ed.setFindMatches(ed.findMatches, current: next)
        let r = ed.findMatches[next]
        ed.textView.setSelectedRange(r)
        ed.textView.scrollRangeToVisible(r)
        ed.textView.showFindIndicator(for: r)
        currentIndex = next + 1
    }

    func next() {
        guard let ed = editor else { return }
        if ed.findMatches.isEmpty { refreshMatches(keepIndex: false, includeHiddenFind: true) }
        let caret = NSMaxRange(ed.textView.selectedRange())
        let i = ed.findMatches.firstIndex { $0.location >= caret && $0 != ed.textView.selectedRange() } ?? 0
        select(ed.findMatches.indices.contains(ed.findIndex) && ed.findMatches[ed.findIndex] == ed.textView.selectedRange() ? ed.findIndex + 1 : i)
    }

    func previous() {
        guard let ed = editor else { return }
        if ed.findMatches.isEmpty { refreshMatches(keepIndex: false, includeHiddenFind: true) }
        select(ed.findIndex - 1)
    }

    func replaceCurrent() {
        guard let ed = editor, let rx = regex(), ed.findMatches.indices.contains(ed.findIndex) else { return }
        let r = ed.findMatches[ed.findIndex]
        let found = ed.nsText.substring(with: r)
        let rep = useRegex ? rx.replacementString(for: rx.firstMatch(in: found, range: NSRange(location: 0, length: (found as NSString).length))!, in: found, offset: 0, template: replacement) : replacement
        ed.apply(TextEdit(range: r, replacement: rep, selection: NSRange(location: r.location + (rep as NSString).length, length: 0)), actionName: "Replace")
        refreshMatches(keepIndex: true)
        if matchCount > 0 { select(min(ed.findIndex, matchCount - 1)) }
    }

    func replaceAll() {
        guard let ed = editor, let rx = regex() else { return }
        let text = ed.text
        let full = NSRange(location: 0, length: (text as NSString).length)
        let template = useRegex ? replacement : NSRegularExpression.escapedTemplate(for: replacement)
        let out = rx.stringByReplacingMatches(in: text, range: full, withTemplate: template)
        guard out != text else { return }
        ed.apply(TextEdit(range: full, replacement: out, selection: NSRange(location: min(ed.textView.selectedRange().location, (out as NSString).length), length: 0)), actionName: "Replace All")
        refreshMatches(keepIndex: false)
    }
}

struct FindBarView: View {
    @ObservedObject var model: FindBarModel
    @ObservedObject var workspace: ProjectDocument
    @FocusState private var findFocused: Bool

    var body: some View {
        VStack(spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Find", text: $model.query)
                    .textFieldStyle(.roundedBorder)
                    .focused($findFocused)
                    .onSubmit { model.next() }
                    .frame(minWidth: 180)
                Toggle(isOn: $model.matchCase) { Text("Aa") }.toggleStyle(.button).help("Match case")
                Toggle(isOn: $model.wholeWord) { Text("W") }.toggleStyle(.button).help("Whole word")
                Toggle(isOn: $model.useRegex) { Text(".*") }.toggleStyle(.button).help("Regular expression")
                Text(model.regexError ?? (model.query.isEmpty ? "" : "\(model.currentIndex) of \(model.matchCount)"))
                    .font(.caption).foregroundStyle(model.regexError == nil ? Color.secondary : Color.red).monospacedDigit()
                    .frame(minWidth: 60)
                Button { model.previous() } label: { Image(systemName: "chevron.left") }.help("Previous (⇧⌘G)")
                Button { model.next() } label: { Image(systemName: "chevron.right") }.help("Next (⌘G)")
                Button { workspace.findShowsReplace.toggle() } label: { Image(systemName: "arrow.left.arrow.right") }.help("Replace")
                Button("Done") { workspace.showFindBar = false; model.close() }.keyboardShortcut(.cancelAction)
            }
            if workspace.findShowsReplace {
                HStack(spacing: 6) {
                    Image(systemName: "arrow.2.squarepath").foregroundStyle(.secondary)
                    TextField(model.useRegex ? "Replace ($1 for groups)" : "Replace", text: $model.replacement)
                        .textFieldStyle(.roundedBorder)
                        .onSubmit { model.replaceCurrent() }
                    Button("Replace") { model.replaceCurrent() }.disabled(model.matchCount == 0)
                    Button("Replace All") { model.replaceAll() }.disabled(model.matchCount == 0)
                }
            }
        }
        .controlSize(.small)
        .padding(.horizontal, 10).padding(.vertical, 6)
        .background(.bar)
        .onAppear { findFocused = true; model.refreshMatches(keepIndex: false) }
        .onChange(of: model.focusRequested) { findFocused = true }
        .onDisappear { model.close() }
    }
}
