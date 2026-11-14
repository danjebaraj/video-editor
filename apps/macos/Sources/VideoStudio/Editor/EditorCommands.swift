import AppKit
import MarkdownCore

/// Formatting commands and slash-command execution.
extension EditorController {
    func perform(_ cmd: FormatCommand) {
        guard textView.isEditable else { NSSound.beep(); return }
        let ns = nsText
        let sel = textView.selectedRange()
        switch cmd {
        case .bold: apply(MarkdownEditing.toggleInline(ns, selection: sel, marker: "**", placeholder: "bold text"), actionName: "Bold")
        case .italic: apply(MarkdownEditing.toggleInline(ns, selection: sel, marker: "*", placeholder: "italic text"), actionName: "Italic")
        case .strikethrough: apply(MarkdownEditing.toggleInline(ns, selection: sel, marker: "~~", placeholder: "strikethrough"), actionName: "Strikethrough")
        case .inlineCode: apply(MarkdownEditing.toggleInline(ns, selection: sel, marker: "`", placeholder: "code"), actionName: "Inline Code")
        case .link: showLinkDialog()
        case .image: document.windowController?.pickMedia()
        case .imageLink: insertImageLink()
        case .island: insertIsland()
        case .heading1, .heading2, .heading3, .heading4, .heading5, .heading6:
            apply(MarkdownEditing.setHeading(ns, selection: sel, level: cmd.headingLevel), actionName: cmd.title)
        case .paragraph: apply(MarkdownEditing.setHeading(ns, selection: sel, level: 0), actionName: "Paragraph")
        case .bulletList: apply(MarkdownEditing.toggleLineStyle(ns, selection: sel, style: .bullet), actionName: "Bullet List")
        case .numberedList: apply(MarkdownEditing.toggleLineStyle(ns, selection: sel, style: .numbered), actionName: "Numbered List")
        case .taskList: apply(MarkdownEditing.toggleLineStyle(ns, selection: sel, style: .task), actionName: "Task List")
        case .toggleTask: apply(MarkdownEditing.toggleTask(ns, caret: sel.location), actionName: "Toggle Task")
        case .blockquote: apply(MarkdownEditing.toggleLineStyle(ns, selection: sel, style: .quote), actionName: "Blockquote")
        case .codeBlock: apply(MarkdownEditing.codeBlock(ns, selection: sel), actionName: "Code Block")
        case .table: showTableGrid()
        case .formatTable: performTable(.format)
        case .convertToTable: convertSelectionToTable()
        case .horizontalRule: apply(MarkdownEditing.insertBlock(ns, selection: NSRange(location: sel.location, length: 0), block: "---"), actionName: "Horizontal Rule")
        case .indent: apply(MarkdownEditing.shiftLines(ns, selection: sel, outdent: false, unit: AppSettings.shared.indentUnit), actionName: "Indent")
        case .outdent: apply(MarkdownEditing.shiftLines(ns, selection: sel, outdent: true, unit: AppSettings.shared.indentUnit), actionName: "Outdent")
        case .moveLineUp: if let e = MarkdownEditing.moveLines(ns, selection: sel, up: true) { apply(e, actionName: "Move Line Up") }
        case .moveLineDown: if let e = MarkdownEditing.moveLines(ns, selection: sel, up: false) { apply(e, actionName: "Move Line Down") }
        case .alertNote, .alertTip, .alertImportant, .alertWarning, .alertCaution:
            let kind = cmd.alertKind!
            // On an existing alert's first line, change its type instead of inserting another.
            let line = ns.lineRange(for: NSRange(location: sel.location, length: 0))
            let header = try! NSRegularExpression(pattern: "^(\\s*>\\s*\\[!)(NOTE|TIP|IMPORTANT|WARNING|CAUTION)(\\])", options: .caseInsensitive)
            if sel.length == 0, let m = header.firstMatch(in: ns as String, range: line) {
                apply(TextEdit(range: m.range(at: 2), replacement: kind, selection: sel), actionName: cmd.title)
                return
            }
            let body = sel.length > 0 ? ns.substring(with: sel).components(separatedBy: "\n").map { "> " + $0 }.joined(separator: "\n") : "> Text"
            let block = "> [!\(kind)]\n" + body
            let selectIn = sel.length > 0 ? nil : NSRange(location: (block as NSString).length - 4, length: 4)
            apply(MarkdownEditing.insertBlock(ns, selection: sel, block: block, selectInBlock: selectIn), actionName: cmd.title)
        case .mermaid:
            let block = "```mermaid\nflowchart LR\n  A[Start] --> B{Decision}\n  B -->|Yes| C[Done]\n  B -->|No| A\n```"
            apply(MarkdownEditing.insertBlock(ns, selection: sel, block: block, selectInBlock: NSRange(location: 11, length: 12)), actionName: "Mermaid")
        case .math:
            apply(MarkdownEditing.insertBlock(ns, selection: sel, block: "$$\n\n$$", selectInBlock: NSRange(location: 3, length: 0)), actionName: "Math")
        case .footnote:
            let f = MarkdownEditing.footnote(ns, selection: sel)
            // Insert definition at end first so the reference location stays valid.
            let end = ns.length
            let defText = f.definitionAppend
            textView.undoManager?.beginUndoGrouping()
            apply(TextEdit(range: NSRange(location: end, length: 0), replacement: defText, selection: sel))
            apply(f.reference)
            textView.undoManager?.endUndoGrouping()
            textView.undoManager?.setActionName("Footnote")
            let defEnd = nsText.length
            textView.setSelectedRange(NSRange(location: defEnd, length: 0))
            textView.scrollRangeToVisible(NSRange(location: defEnd, length: 0))
        case .frontMatter:
            guard scan.frontMatter == nil else { NSSound.beep(); return }
            let block = "---\ntitle: \ndate: \(isoDate())\ntags: []\n---\n"
            apply(TextEdit(range: NSRange(location: 0, length: sel.location == 0 && sel.length > 0 ? 0 : 0), replacement: block, selection: NSRange(location: 11, length: 0)), actionName: "Front Matter")
        case .toc:
            let toc = MarkdownEditing.tableOfContents(for: text)
            apply(MarkdownEditing.insertBlock(ns, selection: sel, block: toc.isEmpty ? "<!-- No headings yet -->" : toc), actionName: "Table of Contents")
        case .date: insert(isoDate())
        case .time:
            let f = DateFormatter(); f.dateFormat = "HH:mm"
            insert(f.string(from: Date()))
        case .details:
            let block = "<details>\n<summary>Summary</summary>\n\nContent\n\n</details>"
            apply(MarkdownEditing.insertBlock(ns, selection: sel, block: block, selectInBlock: NSRange(location: 19, length: 7)), actionName: "Details")
        }
    }

    func isoDate() -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd"
        return f.string(from: Date())
    }

    /// Runs a slash command after its `/query` text has been removed.
    func runSlashCommand(_ cmd: SlashCommand, typed: String) {
        if cmd.id == "codeblock" {
            completion.showLanguages(in: self, anchor: textView.selectedRange().location)
            return
        }
        if cmd.id == "table" {
            insertTable(rows: 1, columns: 2)
            return
        }
        if ["scene", "vo", "shot", "music", "cta"].contains(cmd.id) {
            insertSpecBlock(cmd.id)
            return
        }
        guard let fc = FormatCommand.fromSlash(cmd.id) else { return }
        perform(fc)
    }

    /// Inserts one of the structured spec blocks (spec §5) and selects its first placeholder.
    func insertSpecBlock(_ id: String) {
        guard textView.isEditable else { NSSound.beep(); return }
        let blocks: [SlashBlock] = (try? Core.run("spec.slash_blocks")) ?? []
        guard let block = blocks.first(where: { $0.command == id }) else { return }
        let text = block.snippet.trimmingCharacters(in: .newlines)
        var select: NSRange?
        let ns = text as NSString
        let open = ns.range(of: "{{")
        if open.location != NSNotFound {
            let close = ns.range(of: "}}", range: NSRange(location: open.location, length: ns.length - open.location))
            if close.location != NSNotFound { select = NSRange(location: open.location, length: NSMaxRange(close) - open.location) }
        }
        apply(MarkdownEditing.insertBlock(nsText, selection: textView.selectedRange(), block: text, selectInBlock: select), actionName: "Insert \(block.title)")
    }

    func insertCodeFence(language: String) {
        apply(MarkdownEditing.codeBlock(nsText, selection: textView.selectedRange(), language: language), actionName: "Code Block")
        // Place caret on the empty line inside the fence.
        let sel = textView.selectedRange()
        let lineEnd = nsText.lineRange(for: NSRange(location: sel.location, length: 0))
        textView.setSelectedRange(NSRange(location: min(NSMaxRange(lineEnd), nsText.length), length: 0))
    }

    // MARK: Dialogs

    func showLinkDialog() {
        let sel = textView.selectedRange()
        let selected = nsText.substring(with: sel)
        let clip = NSPasteboard.general.string(forType: .string)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let clipURL = clip.range(of: "^(https?://|mailto:|#|\\./|\\.\\./)\\S+$", options: .regularExpression) != nil ? clip : ""
        let alert = NSAlert()
        alert.messageText = "Insert Link"
        alert.addButton(withTitle: "Insert")
        alert.addButton(withTitle: "Cancel")
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        let textField = NSTextField(string: selected)
        textField.placeholderString = "Link text"
        let urlField = NSTextField(string: clipURL)
        urlField.placeholderString = "https://example.com"
        for (label, field) in [("Text", textField), ("URL", urlField)] {
            let l = NSTextField(labelWithString: label)
            field.translatesAutoresizingMaskIntoConstraints = false
            field.widthAnchor.constraint(equalToConstant: 300).isActive = true
            stack.addArrangedSubview(l)
            stack.addArrangedSubview(field)
        }
        stack.frame = NSRect(x: 0, y: 0, width: 300, height: 110)
        alert.accessoryView = stack
        alert.window.initialFirstResponder = selected.isEmpty ? textField : urlField
        guard let window = textView.window else { return }
        alert.beginSheetModal(for: window) { [weak self] resp in
            guard let self, resp == .alertFirstButtonReturn else { return }
            let label = textField.stringValue.isEmpty ? (urlField.stringValue.isEmpty ? "link text" : urlField.stringValue) : textField.stringValue
            let url = urlField.stringValue.trimmingCharacters(in: .whitespaces)
            let rep = "[\(label)](\(url.isEmpty ? "url" : url))"
            let selection = url.isEmpty ? NSRange(location: sel.location + (label as NSString).length + 3, length: 3)
                                        : NSRange(location: sel.location + (rep as NSString).length, length: 0)
            self.apply(TextEdit(range: sel, replacement: rep, selection: selection), actionName: "Link")
            self.focus()
        }
    }

    func caretRect() -> NSRect {
        rect(forCharacterAt: textView.selectedRange().location)
    }

    /// Rect of the line fragment containing a character, in text view coordinates (TextKit 2).
    func rect(forCharacterAt loc: Int) -> NSRect {
        guard let tlm = textView.textLayoutManager, let tcm = tlm.textContentManager,
              let tl = tcm.location(tcm.documentRange.location, offsetBy: min(loc, nsText.length)) else { return .zero }
        tlm.ensureLayout(for: NSTextRange(location: tl))
        guard let frag = tlm.textLayoutFragment(for: tl) else { return .zero }
        let origin = textView.textContainerOrigin
        let fragOffset = tcm.offset(from: tcm.documentRange.location, to: frag.rangeInElement.location)
        var lineRect = frag.layoutFragmentFrame
        for line in frag.textLineFragments {
            let r = line.characterRange
            if loc - fragOffset >= r.location && loc - fragOffset <= NSMaxRange(r) {
                let x = line.locationForCharacter(at: loc - fragOffset).x
                lineRect = NSRect(x: frag.layoutFragmentFrame.minX + line.typographicBounds.minX + x,
                                  y: frag.layoutFragmentFrame.minY + line.typographicBounds.minY,
                                  width: 1, height: line.typographicBounds.height)
                break
            }
        }
        return lineRect.offsetBy(dx: origin.x, dy: origin.y)
    }

    func showTableGrid() {
        AnchoredPanel.show(TableGridPicker { [weak self] rows, cols in
            AnchoredPanel.closeCurrent()
            self?.insertTable(rows: rows, columns: cols)
            self?.focus()
        }, below: caretRect(), in: textView)
    }
}
