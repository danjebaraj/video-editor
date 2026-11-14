import AppKit
import MarkdownCore

// Just Notes' editor has todo shortcuts and note extraction woven into typing. A video spec has
// no use for them, so the hooks the shared editor code calls are small here: task-list boxes
// still toggle on click, and nothing else happens.
extension EditorController {
    func convertTodoShortcut(line: Int, beforeMoreEdits: Bool = false) -> Bool { false }

    func todoSelectionChanged() {}

    /// Ends the undo group of earlier edits in the same event, so the next edit is its own undo step.
    func startOwnUndoStep() {
        if let um = textView.undoManager, um.groupsByEvent, um.groupingLevel == 1 { um.endUndoGrouping(); um.beginUndoGrouping() }
    }

    func todoRaw(line: Int) -> String? { nil }

    /// A click on `[ ]` or `[x]` of a task item toggles it.
    func handleTodoClick(at idx: Int) -> Bool {
        guard textView.isEditable, idx >= 0, idx < nsText.length, !isInCodeBlock(idx) else { return false }
        let line = scan.line(at: idx)
        let range = lineRangeChars(line...line)
        let text = nsText.substring(with: range) as NSString
        let rx = try! NSRegularExpression(pattern: "^\\s*(?:[-*+]|\\d+[.)])\\s+\\[([ xX])\\]")
        guard let m = rx.firstMatch(in: text as String, range: NSRange(location: 0, length: text.length)) else { return false }
        let box = m.range(at: 1)
        let boxStart = range.location + box.location - 1
        guard idx >= boxStart, idx <= boxStart + 2 else { return false }
        let checked = text.substring(with: box) != " "
        let sel = textView.selectedRange()
        apply(TextEdit(range: NSRange(location: range.location + box.location, length: 1), replacement: checked ? " " : "x", selection: sel), actionName: "Toggle Task")
        return true
    }

    /// Puts the selection's context in the Claude panel's composer.
    func askAboutSelection() {
        document.inspectorTab = .claude
        document.assistant.includeSelection = true
        document.windowController?.focusChat()
    }
}
