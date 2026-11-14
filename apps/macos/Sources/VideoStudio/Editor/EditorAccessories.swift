import AppKit
import SwiftUI
import MarkdownCore

/// Line numbers in the editor's vertical ruler (TextKit 2).
final class LineNumberRulerView: NSRulerView {
    weak var controller: EditorController?

    init(controller: EditorController) {
        self.controller = controller
        super.init(scrollView: controller.scrollView, orientation: .verticalRuler)
        ruleThickness = 44
        clientView = controller.textView
    }

    required init(coder: NSCoder) { fatalError() }

    override func drawHashMarksAndLabels(in rect: NSRect) {
        guard let c = controller, let tlm = c.textView.textLayoutManager, let tcm = tlm.textContentManager else { return }
        NSColor.textBackgroundColor.setFill()
        bounds.fill()
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedDigitSystemFont(ofSize: max(9, c.highlighter.baseFont.pointSize - 3), weight: .regular),
                                                    .foregroundColor: NSColor.tertiaryLabelColor]
        let visible = c.textView.visibleRect
        let origin = c.textView.textContainerOrigin
        let startLoc = tlm.textViewportLayoutController.viewportRange?.location ?? tcm.documentRange.location
        var lastLine = -1
        tlm.enumerateTextLayoutFragments(from: startLoc, options: [.ensuresLayout]) { frag in
            let frame = frag.layoutFragmentFrame
            if frame.minY + origin.y > visible.maxY { return false }
            let offset = tcm.offset(from: tcm.documentRange.location, to: frag.rangeInElement.location)
            let line = c.scan.line(at: offset)
            if line != lastLine {
                lastLine = line
                let s = "\(line + 1)" as NSString
                let size = s.size(withAttributes: attrs)
                let y = frame.minY + origin.y - visible.minY + (frag.textLineFragments.first?.typographicBounds.height ?? size.height) / 2 - size.height / 2
                s.draw(at: NSPoint(x: ruleThickness - size.width - 8, y: y), withAttributes: attrs)
            }
            return true
        }
    }
}

/// Hover grid for choosing table dimensions (up to 10 × 10).
struct TableGridPicker: View {
    var onPick: (Int, Int) -> Void
    @State private var hover = (rows: 2, cols: 3)

    var body: some View {
        VStack(spacing: 8) {
            Text("\(hover.rows) × \(hover.cols) table").font(.callout).monospacedDigit()
            VStack(spacing: 3) {
                ForEach(0..<10, id: \.self) { r in
                    HStack(spacing: 3) {
                        ForEach(0..<10, id: \.self) { c in
                            RoundedRectangle(cornerRadius: 2)
                                .fill(r < hover.rows && c < hover.cols ? Color.accentColor.opacity(0.7) : Color.secondary.opacity(0.18))
                                .frame(width: 16, height: 16)
                                .onHover { inside in if inside { hover = (r + 1, c + 1) } }
                                .onTapGesture { onPick(r + 1, c + 1) }
                        }
                    }
                }
            }
            Text("Rows are body rows; a header row is added.").font(.caption).foregroundStyle(.secondary)
        }
        .padding(12)
    }
}

/// Spreadsheet-like table editor sheet ("Edit Table…").
struct TableEditorView: View {
    @State var table: MarkdownTable
    var onSave: (MarkdownTable) -> Void
    var onCancel: () -> Void
    @State private var draggedRow: Int?
    @State private var draggedCol: Int?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Edit Table").font(.headline)
            ScrollView([.horizontal, .vertical]) {
                Grid(alignment: .leading, horizontalSpacing: 4, verticalSpacing: 4) {
                    GridRow {
                        Color.clear.frame(width: 22, height: 1)
                        ForEach(0..<table.columnCount, id: \.self) { c in
                            HStack(spacing: 2) {
                                Image(systemName: "line.3.horizontal").foregroundStyle(.secondary).font(.caption)
                                Picker("", selection: Binding(get: { table.alignments[c] }, set: { table.alignments[c] = $0 })) {
                                    Text("—").tag(TableAlignment.none)
                                    Image(systemName: "text.alignleft").tag(TableAlignment.left)
                                    Image(systemName: "text.aligncenter").tag(TableAlignment.center)
                                    Image(systemName: "text.alignright").tag(TableAlignment.right)
                                }
                                .labelsHidden().frame(width: 60)
                                Button { table.deleteColumn(at: c) } label: { Image(systemName: "minus.circle") }.buttonStyle(.borderless).disabled(table.columnCount <= 1)
                            }
                            .onDrag { draggedCol = c; return NSItemProvider(object: "col\(c)" as NSString) }
                            .onDrop(of: [.text], isTargeted: nil) { _ in
                                if let from = draggedCol, from != c { moveColumn(from, c) }
                                draggedCol = nil
                                return true
                            }
                        }
                    }
                    GridRow {
                        Text("H").font(.caption).foregroundStyle(.secondary).frame(width: 22)
                        ForEach(0..<table.columnCount, id: \.self) { c in
                            TextField("Header", text: Binding(get: { table.header[c] }, set: { table.header[c] = $0 }))
                                .textFieldStyle(.roundedBorder).fontWeight(.semibold).frame(minWidth: 120)
                        }
                    }
                    ForEach(0..<table.rows.count, id: \.self) { r in
                        GridRow {
                            Image(systemName: "line.3.horizontal").foregroundStyle(.secondary).frame(width: 22)
                                .onDrag { draggedRow = r; return NSItemProvider(object: "row\(r)" as NSString) }
                            ForEach(0..<table.columnCount, id: \.self) { c in
                                TextField("", text: Binding(get: { r < table.rows.count ? table.rows[r][c] : "" }, set: { if r < table.rows.count { table.rows[r][c] = $0 } }))
                                    .textFieldStyle(.roundedBorder).frame(minWidth: 120)
                            }
                            Button { table.deleteRow(at: r) } label: { Image(systemName: "minus.circle") }.buttonStyle(.borderless)
                        }
                        .onDrop(of: [.text], isTargeted: nil) { _ in
                            if let from = draggedRow, from != r {
                                let row = table.rows.remove(at: from)
                                table.rows.insert(row, at: r)
                            }
                            draggedRow = nil
                            return true
                        }
                    }
                }
                .padding(4)
            }
            .frame(minWidth: 520, minHeight: 260)
            HStack {
                Button("Add Row") { table.insertRow(at: table.rows.count) }
                Button("Add Column") { table.insertColumn(at: table.columnCount) }
                Spacer()
                Button("Cancel", role: .cancel) { onCancel() }.keyboardShortcut(.cancelAction)
                Button("Save") { onSave(table) }.keyboardShortcut(.defaultAction)
            }
        }
        .padding(16)
    }

    private func moveColumn(_ from: Int, _ to: Int) {
        let header = table.header.remove(at: from); table.header.insert(header, at: to)
        let align = table.alignments.remove(at: from); table.alignments.insert(align, at: to)
        for r in table.rows.indices { let v = table.rows[r].remove(at: from); table.rows[r].insert(v, at: to) }
    }
}

enum TableEditorSheet {
    @MainActor
    static func present(table: MarkdownTable, from window: NSWindow?, onSave: @escaping (MarkdownTable) -> Void) {
        guard let window else { return }
        var sheet: NSWindow!
        let view = TableEditorView(table: table, onSave: { t in
            window.endSheet(sheet)
            onSave(t)
        }, onCancel: { window.endSheet(sheet) })
        sheet = NSWindow(contentViewController: NSHostingController(rootView: view))
        sheet.setContentSize(NSSize(width: 720, height: 420))
        window.beginSheet(sheet)
    }
}

/// A small floating panel anchored under a point in an editor (used instead of NSPopover, whose placement
/// is unreliable inside the SwiftUI/split-view hierarchy). Closes on Esc, on losing key, or via `close()`.
@MainActor
final class AnchoredPanel: NSPanel {
    private static var current: AnchoredPanel?

    override var canBecomeKey: Bool { true }

    override func cancelOperation(_ sender: Any?) { close() }

    override func resignKey() {
        super.resignKey()
        // Close when the user clicks elsewhere (but not while a menu from inside the panel is open).
        DispatchQueue.main.async { [weak self] in
            guard let self, !self.isKeyWindow, NSApp.keyWindow?.parent !== self else { return }
            if NSApp.keyWindow != nil { self.close() }
        }
    }

    override func close() {
        parent?.removeChildWindow(self)
        super.close()
        if AnchoredPanel.current === self { AnchoredPanel.current = nil }
    }

    static func closeCurrent() { current?.close() }

    /// Shows `content` with its top-left just below `rect` (in `view` coordinates).
    @discardableResult
    static func show<V: View>(_ content: V, below rect: NSRect, in view: NSView) -> AnchoredPanel? {
        current?.close()
        guard let window = view.window else { return nil }
        let host = NSHostingController(rootView: content)
        let size = host.view.fittingSize
        let panel = AnchoredPanel(contentRect: NSRect(origin: .zero, size: size), styleMask: [.titled, .fullSizeContentView], backing: .buffered, defer: false)
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.standardWindowButton(.closeButton)?.isHidden = true
        panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
        panel.standardWindowButton(.zoomButton)?.isHidden = true
        panel.isMovableByWindowBackground = true
        panel.contentViewController = host
        panel.isReleasedWhenClosed = false
        let screenRect = window.convertToScreen(view.convert(rect, to: nil))
        var origin = NSPoint(x: screenRect.minX - 12, y: screenRect.minY - size.height - 6)
        if let vf = window.screen?.visibleFrame {
            origin.x = min(max(origin.x, vf.minX + 8), vf.maxX - size.width - 8)
            if origin.y < vf.minY { origin.y = screenRect.maxY + 6 }
        }
        panel.setFrameOrigin(origin)
        window.addChildWindow(panel, ordered: .above)
        panel.makeKeyAndOrderFront(nil)
        current = panel
        return panel
    }
}
