import AppKit
import Combine

/// Review of a change Claude made to the spec (spec §4.3): the change is shown as hunks, and
/// only accepted hunks are written back. Rejecting restores the previous text.
@MainActor
final class SpecReview: ObservableObject {
    struct Pending: Equatable {
        /// The editor's text when the turn started.
        var base: String
        /// Claude's version of the whole spec.
        var proposed: String
        var hunks: [Hunk]
        /// Per hunk: accepted, rejected, or not decided yet.
        var decisions: [Bool?]

        var undecided: Int { decisions.filter { $0 == nil }.count }
        var accepted: Int { decisions.filter { $0 == true }.count }
    }

    unowned let document: ProjectDocument
    @Published private(set) var pending: Pending?

    init(document: ProjectDocument) { self.document = document }

    /// Claude changed `spec.md`: diff its text against what the editor had and wait for review.
    /// A later change in the same turn replaces the proposal; decisions on unchanged hunks stay.
    func propose(_ spec: String) {
        let base = pending?.base ?? document.editor.text
        let hunks: [Hunk] = (try? Core.run("spec.diff", ["old": base, "new": spec])) ?? []
        guard !hunks.isEmpty else {
            if pending != nil { finish(applying: []) }
            return
        }
        var decisions: [Bool?] = Array(repeating: nil, count: hunks.count)
        if let old = pending {
            for (i, h) in hunks.enumerated() {
                if let j = old.hunks.firstIndex(of: h) { decisions[i] = old.decisions[j] }
            }
        }
        pending = Pending(base: base, proposed: spec, hunks: hunks, decisions: decisions)
        document.editor.textView.isEditable = false
    }

    func decide(_ index: Int, accept: Bool) {
        guard var p = pending, p.decisions.indices.contains(index) else { return }
        p.decisions[index] = accept
        pending = p
        if p.undecided == 0 { finish() }
    }

    func acceptAll() {
        guard var p = pending else { return }
        p.decisions = p.decisions.map { _ in true }
        pending = p
        finish()
    }

    func rejectAll() {
        guard var p = pending else { return }
        p.decisions = p.decisions.map { _ in false }
        pending = p
        finish()
    }

    /// Ends the review: accepted hunks go into the spec, everything else keeps the old text.
    func finish(applying override: [Bool]? = nil) {
        guard let p = pending else { return }
        let accepted = override ?? p.decisions.map { $0 == true }
        pending = nil
        document.editor.textView.isEditable = true
        guard accepted.contains(true) else { return }
        guard let result: String = try? Core.run("spec.apply_hunks", ["old": p.base, "hunks": Core.arg(p.hunks), "accepted": accepted]) else { return }
        // One undoable edit, so Undo brings the previous spec back.
        document.editor.replaceAllText(result, undoable: true)
        document.textDidChange()
    }
}
