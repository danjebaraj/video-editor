import AppKit
import MarkdownCore

/// Islands in the source editor: creating, renaming, unwrapping and deleting them, quick fixes for
/// broken markers, renaming pasted copies, and drawing the region tint and problem underlines.
extension EditorController {
    /// Rescans islands shortly after edits, or right away with `now`.
    func scheduleIslandScan(now: Bool = false) {
        islandWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            let fresh = Islands.scan(self.text)
            guard fresh != self.islandScan else { return }
            self.islandScan = fresh
            self.textView.needsDisplay = true
        }
        islandWork = work
        if now { work.perform() } else { DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: work) }
    }

    /// Names to avoid for a new island: this document and every other file in the open folder.
    func islandNamesInUse() -> Set<String> {
        var names = islandScan.names
        return names
    }

    /// The island whose markers or content include `idx` (fresh scan).
    func island(at idx: Int) -> Island? {
        scheduleIslandScan(now: true)
        return islandScan.island(at: idx)
    }

    func islandMarker(at idx: Int) -> IslandMarker? {
        islandScan.markers.first { NSLocationInRange(idx, $0.range) || idx == NSMaxRange($0.range) }
    }

    func islandProblem(at idx: Int) -> IslandProblem? {
        scheduleIslandScan(now: true)
        return islandScan.problems.first { NSLocationInRange(idx, $0.marker.range) || idx == NSMaxRange($0.marker.range) }
    }

    // MARK: Creating

    /// Inserts an empty island at the caret, or wraps the selected lines in one. One undo step.
    func insertIsland() {
        guard textView.isEditable else { NSSound.beep(); return }
        scheduleIslandScan(now: true)
        let name = IslandNames.generate(avoiding: islandNamesInUse())
        apply(Islands.insert(nsText, selection: textView.selectedRange(), name: name), actionName: "Insert Island")
        scheduleIslandScan(now: true)
    }

    // MARK: Actions

    func renameIsland(_ island: Island) {
        guard let new = Prompt.text(title: "Rename Island", message: "New name (lowercase letters, digits and dashes):", initial: island.name)?
            .trimmingCharacters(in: .whitespaces), new != island.name else { return }
        guard Islands.isValidName(new) else { Prompt.error("“\(new)” isn't a valid island name. Use lowercase letters, digits and single dashes, up to 40 characters."); return }
        guard !islandScan.names.contains(new) else { Prompt.error("This spec already has an island called “\(new)”."); return }
        apply(Islands.rename(nsText, island, to: new), actionName: "Rename Island")
        scheduleIslandScan(now: true)
        updateMentions(from: island.name, to: new)
    }

    /// Updates chips and, after asking, `@old` in the current assistant session's unsent message.
    func updateMentions(from old: String, to new: String) {
        let chat = document.assistant
        guard Islands.mentions(in: chat.draft).contains(old) else { return }
        if TestHooks.autoUpdateMentions ?? Prompt.confirm("Update @\(old) in your unsent message?", message: "Your message to the assistant mentions the island's old name.", ok: "Update") {
            chat.draft = Islands.replacingMentions(of: old, with: new, in: chat.draft)
        }
    }

    func unwrapIsland(_ island: Island) {
        apply(Islands.unwrap(nsText, island), actionName: "Unwrap Island")
        scheduleIslandScan(now: true)
    }

    func deleteIsland(_ island: Island) {
        apply(Islands.delete(nsText, island), actionName: "Delete Island")
        scheduleIslandScan(now: true)
    }

    func copyIslandReference(_ island: Island) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString("@\(island.name)", forType: .string)
        status.show("Copied @\(island.name).")
    }

    func askAboutIsland(_ island: Island) {
        // Mention the island in the Assistant panel's message, ready for the question.
        let chat = document.assistant
        if !Islands.mentions(in: chat.draft).contains(island.name) { chat.draft = "@\(island.name) " + chat.draft }
        document.inspectorTab = .claude
        document.windowController?.focusChat()
    }

    // MARK: Quick fixes

    struct IslandFix { var title: String; var run: () -> Void }

    func fixes(for problem: IslandProblem) -> [IslandFix] {
        let m = problem.marker
        switch problem.kind {
        case .unclosed:
            return [IslandFix(title: "Insert End Marker") { [weak self] in self?.applyFix(Islands.closeAfterNextBlock(self!.nsText, m), "Insert End Marker") },
                    IslandFix(title: "Remove Start Marker") { [weak self] in self?.applyFix(Islands.removeMarker(self!.nsText, m), "Remove Marker") }]
        case .unmatchedEnd:
            return [IslandFix(title: "Remove End Marker") { [weak self] in self?.applyFix(Islands.removeMarker(self!.nsText, m), "Remove Marker") }]
        case .duplicate, .invalidName:
            return [IslandFix(title: problem.kind == .duplicate ? "Rename This Island" : "Use a Generated Name") { [weak self] in
                guard let self else { return }
                let name = IslandNames.generate(avoiding: self.islandNamesInUse())
                if let island = self.islandScan.islands.first(where: { $0.start == m }) {
                    self.applyFix(Islands.rename(self.nsText, island, to: name), "Rename Island")
                } else {
                    self.applyFix(Islands.renameMarker(self.nsText, m, to: name), "Rename Island")
                }
            }]
        case .nested:
            return [IslandFix(title: "Close Outer Island First") { [weak self] in self?.closeOuterIsland(before: m) },
                    IslandFix(title: "Unwrap Inner Island") { [weak self] in self?.unwrapNested(m) }]
        }
    }

    private func applyFix(_ edit: TextEdit, _ name: String) {
        apply(edit, actionName: name)
        scheduleIslandScan(now: true)
    }

    /// Moves the outer island's end marker to just before the nested start marker.
    private func closeOuterIsland(before inner: IslandMarker) {
        guard let outer = islandScan.islands.first(where: { $0.start.line < inner.line && inner.line < $0.end.line }) else { return }
        let out = NSMutableString(string: text)
        out.replaceCharacters(in: Islands.lineRangeWithNewline(out, outer.end.range), with: "")
        out.insert(Islands.endMarker(outer.name) + "\n", at: inner.range.location)
        replaceText(with: out as String, actionName: "Close Outer Island")
        scheduleIslandScan(now: true)
    }

    /// Removes a nested start marker and the first matching end marker after it.
    private func unwrapNested(_ inner: IslandMarker) {
        let out = NSMutableString(string: text)
        if let end = islandScan.markers.first(where: { $0.isEnd && $0.name == inner.name && $0.line > inner.line }) {
            out.replaceCharacters(in: Islands.lineRangeWithNewline(out, end.range), with: "")
        }
        out.replaceCharacters(in: Islands.lineRangeWithNewline(out, inner.range), with: "")
        replaceText(with: out as String, actionName: "Unwrap Island")
        scheduleIslandScan(now: true)
    }

    // MARK: Paste

    /// Pastes text containing islands whose names this file already uses, renaming the copies.
    /// Returns false when nothing needs renaming (normal paste).
    func pasteRenamingIslands(_ string: String) -> Bool {
        guard string.range(of: "island", options: .caseInsensitive) != nil else { return false }
        let sel = textView.selectedRange()
        // Names left in the file once the selection is replaced (a cut-and-paste keeps its name).
        let remaining = Islands.scan(nsText.replacingCharacters(in: sel, with: "")).names
        let others = islandNamesInUse()
        let r = Islands.renamingConflicts(in: string, taken: remaining) { IslandNames.generate(avoiding: $0.union(others)) }
        guard !r.renamed.isEmpty else { return false }
        apply(TextEdit(range: sel, replacement: r.text, selection: NSRange(location: sel.location + (r.text as NSString).length, length: 0)), actionName: "Paste")
        scheduleIslandScan(now: true)
        status.show(r.renamed.count == 1 ? "Pasted island renamed to \(r.renamed[0].1)." : "Pasted islands renamed to \(r.renamed.map(\.1).joined(separator: ", ")).")
        return true
    }

    // MARK: Drawing

    /// The character range laid out in the visible area, for skipping islands far off screen.
    private func viewportRange() -> NSRange? {
        guard let tlm = textView.textLayoutManager, let tcm = tlm.textContentManager,
              let vr = tlm.textViewportLayoutController.viewportRange else { return nil }
        let start = tcm.offset(from: tcm.documentRange.location, to: vr.location)
        let end = tcm.offset(from: tcm.documentRange.location, to: vr.endLocation)
        return NSRange(location: start, length: max(0, end - start))
    }

    static let islandTint = NSColor(name: nil) { $0.bestMatch(from: [.darkAqua]) == .darkAqua ? NSColor.controlAccentColor.withAlphaComponent(0.09) : NSColor.controlAccentColor.withAlphaComponent(0.06) }

    /// Faint tint behind each island and a bar at the left edge (orange for an island with a problem).
    func drawIslands(in dirty: NSRect) {
        guard AppSettings.shared.highlightIslands, !islandScan.islands.isEmpty else { return }
        let len = nsText.length
        let visible = viewportRange()
        for island in islandScan.islands where NSMaxRange(island.fullRange) <= len {
            if let v = visible, NSIntersectionRange(v, island.fullRange).length == 0, !NSLocationInRange(island.fullRange.location, v) { continue }
            let top = rect(forCharacterAt: island.start.range.location).minY
            let bottom = rect(forCharacterAt: island.end.range.location).maxY
            let box = NSRect(x: 0, y: top, width: textView.bounds.width, height: max(0, bottom - top))
            guard box.intersects(dirty) else { continue }
            Self.islandTint.setFill()
            box.fill()
            (island.isUsable ? NSColor.controlAccentColor.withAlphaComponent(0.55) : NSColor.systemOrange.withAlphaComponent(0.7)).setFill()
            NSBezierPath(roundedRect: NSRect(x: 10, y: top + 1, width: 3, height: max(0, box.height - 2)), xRadius: 1.5, yRadius: 1.5).fill()
        }
    }

    /// Wavy underlines under markers with a problem.
    func drawIslandProblems(in dirty: NSRect) {
        guard !islandScan.problems.isEmpty else { return }
        let len = nsText.length
        let visible = viewportRange()
        NSColor.systemRed.withAlphaComponent(0.85).setStroke()
        for p in islandScan.problems where NSMaxRange(p.marker.range) <= len {
            if let v = visible, !NSLocationInRange(p.marker.range.location, NSRange(location: v.location, length: v.length + 1)) { continue }
            let a = rect(forCharacterAt: p.marker.range.location), b = rect(forCharacterAt: NSMaxRange(p.marker.range))
            let y = a.maxY - 1.5
            guard NSRect(x: a.minX, y: a.minY, width: max(0, b.minX - a.minX), height: a.height).intersects(dirty) else { continue }
            let path = NSBezierPath()
            path.lineWidth = 1
            var x = a.minX
            path.move(to: NSPoint(x: x, y: y))
            var up = true
            while x < b.minX {
                x += 2
                path.line(to: NSPoint(x: x, y: up ? y - 1.5 : y + 0.5))
                up.toggle()
            }
            path.stroke()
        }
    }
}

// MARK: Hover

/// The small panel shown when hovering an island marker: the name, any problem, and "Ask Assistant about this island".
@MainActor
enum IslandHover {
    static func view(island: Island?, marker: IslandMarker, problem: IslandProblem?, onAsk: @escaping () -> Void) -> NSView {
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        stack.edgeInsets = NSEdgeInsets(top: 10, left: 12, bottom: 10, right: 12)
        let title = NSTextField(labelWithString: "Island: \(marker.name)")
        title.font = .boldSystemFont(ofSize: 12)
        stack.addArrangedSubview(title)
        if let island {
            let lines = NSTextField(labelWithString: "Lines \(island.start.line + 1)–\(island.end.line + 1) · reference it as @\(island.name)")
            lines.font = .systemFont(ofSize: 11)
            lines.textColor = .secondaryLabelColor
            stack.addArrangedSubview(lines)
        }
        if let problem {
            let p = NSTextField(wrappingLabelWithString: problem.message + " Right-click the marker for a fix.")
            p.font = .systemFont(ofSize: 11)
            p.textColor = .systemOrange
            p.preferredMaxLayoutWidth = 280
            stack.addArrangedSubview(p)
        }
        if island?.isUsable == true, AppState.shared.assistantAvailable {
            let b = ClosureButton(title: "Ask Assistant about this island", action: onAsk)
            b.bezelStyle = .accessoryBarAction
            b.controlSize = .small
            stack.addArrangedSubview(b)
        }
        stack.setFrameSize(stack.fittingSize)
        return stack
    }
}

/// A button that runs a closure.
final class ClosureButton: NSButton {
    private var handler: () -> Void = {}
    convenience init(title: String, action: @escaping () -> Void) {
        self.init(frame: .zero)
        self.title = title
        handler = action
        target = self
        self.action = #selector(run)
    }
    @objc private func run() { handler() }
}
