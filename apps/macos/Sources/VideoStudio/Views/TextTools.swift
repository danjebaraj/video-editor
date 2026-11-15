import AppKit
import SwiftUI

// One-click copy and Markdown preview, used wherever the app shows text it didn't ask the user to type.

enum Clipboard {
    static func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

/// A small button that copies text and shows a checkmark for a moment.
struct CopyButton: View {
    var help: String = "Copy"
    var title: String?
    var text: () -> String
    @State private var copied = false

    init(_ help: String = "Copy", title: String? = nil, text: @escaping () -> String) {
        self.help = help
        self.title = title
        self.text = text
    }

    var body: some View {
        Button {
            Clipboard.copy(text())
            copied = true
            Task {
                try? await Task.sleep(nanoseconds: 1_200_000_000)
                copied = false
            }
        } label: {
            if let title {
                Label(copied ? "Copied" : title, systemImage: copied ? "checkmark" : "doc.on.doc")
            } else {
                Image(systemName: copied ? "checkmark" : "doc.on.doc").font(.system(size: 10.5)).frame(width: 16, height: 16).contentShape(Rectangle())
            }
        }
        .buttonStyle(.borderless)
        .foregroundStyle(copied ? Color.green : Color.secondary)
        .help(copied ? "Copied" : help)
        .accessibilityLabel(help)
    }
}

extension View {
    /// Puts a copy button at the view's top trailing corner, beside it rather than over it.
    func copyable(_ help: String = "Copy", _ text: @escaping () -> String) -> some View {
        HStack(alignment: .top, spacing: 4) {
            self
            CopyButton(help, text: text)
        }
    }
}

/// Switches a Markdown text between its rendered preview and its source.
struct PreviewToggle: View {
    @Binding var preview: Bool

    var body: some View {
        Button { preview.toggle() } label: {
            Image(systemName: preview ? "chevron.left.forwardslash.chevron.right" : "eye").font(.system(size: 10.5)).frame(width: 16, height: 16).contentShape(Rectangle())
        }
        .buttonStyle(.borderless)
        .foregroundStyle(.secondary)
        .help(preview ? "Show the Markdown source" : "Show the rendered preview")
        .accessibilityLabel(preview ? "Show Markdown source" : "Show rendered preview")
    }
}

/// Markdown text with a preview toggle and a copy button beside it. Starts as a preview.
struct MarkdownText: View {
    var text: String
    var sourceFont: Font = .system(size: 12, design: .monospaced)
    @State private var preview = true

    var body: some View {
        HStack(alignment: .top, spacing: 4) {
            Group {
                if preview {
                    MarkdownBlocks(text: text)
                } else {
                    Text(text).font(sourceFont).textSelection(.enabled)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            VStack(spacing: 2) {
                CopyButton("Copy the Markdown") { text }
                PreviewToggle(preview: $preview)
            }
        }
    }
}

/// Renders Markdown as native text: headings, paragraphs, lists, quotes, code blocks, tables and rules.
struct MarkdownBlocks: View {
    var text: String

    enum Block: Equatable {
        case heading(Int, String)
        case paragraph(String)
        case item(indent: Int, marker: String, text: String)
        case quote(String)
        case code(String)
        case table([[String]])
        case rule
    }

    var body: some View {
        let blocks = Self.parse(text)
        VStack(alignment: .leading, spacing: 6) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                view(block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder private func view(_ block: Block) -> some View {
        switch block {
        case .heading(let level, let s):
            Text(Self.inline(s))
                .font(level == 1 ? .title3.weight(.semibold) : level == 2 ? .headline : .subheadline.weight(.semibold))
                .padding(.top, level <= 2 ? 4 : 2)
                .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
        case .paragraph(let s):
            Text(Self.inline(s)).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
        case .item(let indent, let marker, let s):
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(marker).foregroundStyle(.secondary).monospacedDigit()
                Text(Self.inline(s)).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
            }
            .padding(.leading, CGFloat(indent) * 14)
        case .quote(let s):
            Text(Self.inline(s)).foregroundStyle(.secondary).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                .padding(.leading, 8)
                .overlay(alignment: .leading) { Rectangle().fill(Color.secondary.opacity(0.4)).frame(width: 2) }
        case .code(let s):
            HStack(alignment: .top, spacing: 4) {
                ScrollView(.horizontal, showsIndicators: false) {
                    Text(s).font(.system(size: 11, design: .monospaced)).textSelection(.enabled).fixedSize(horizontal: true, vertical: true)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                CopyButton("Copy the code") { s }
            }
            .padding(6)
            .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 5))
        case .table(let rows):
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 4) {
                ForEach(Array(rows.enumerated()), id: \.offset) { r, cells in
                    GridRow {
                        ForEach(Array(cells.enumerated()), id: \.offset) { _, cell in
                            Text(Self.inline(cell)).font(r == 0 ? .callout.weight(.semibold) : .callout)
                                .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    if r == 0 { Divider() }
                }
            }
        case .rule:
            Divider()
        }
    }

    static func inline(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(s)
    }

    static func parse(_ text: String) -> [Block] {
        var blocks: [Block] = []
        var paragraph: [String] = []
        var code: [String]?
        var table: [[String]] = []
        func flush() {
            if !paragraph.isEmpty { blocks.append(.paragraph(paragraph.joined(separator: " "))); paragraph = [] }
            if !table.isEmpty { blocks.append(.table(table)); table = [] }
        }
        for raw in text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n") {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if code != nil {
                if line.hasPrefix("```") || line.hasPrefix("~~~") { blocks.append(.code(code!.joined(separator: "\n"))); code = nil } else { code!.append(raw) }
                continue
            }
            if line.hasPrefix("```") || line.hasPrefix("~~~") { flush(); code = []; continue }
            if line.isEmpty { flush(); continue }
            if line.hasPrefix("|"), line.hasSuffix("|"), line.count > 1 {
                if !paragraph.isEmpty { flush() }
                let cells = line.dropFirst().dropLast().components(separatedBy: "|").map { $0.trimmingCharacters(in: .whitespaces) }
                let separator = cells.allSatisfy { !$0.isEmpty && $0.allSatisfy { "-: ".contains($0) } }
                if !separator { table.append(cells) }
                continue
            }
            if !table.isEmpty { flush() }
            if let level = headingLevel(line) {
                flush()
                blocks.append(.heading(level, String(line.dropFirst(level)).trimmingCharacters(in: .whitespaces)))
            } else if line.count >= 3, line.allSatisfy({ $0 == "-" || $0 == "*" || $0 == "_" || $0 == " " }), Set(line.filter { $0 != " " }).count == 1 {
                flush()
                blocks.append(.rule)
            } else if line.hasPrefix(">") {
                flush()
                blocks.append(.quote(String(line.dropFirst()).trimmingCharacters(in: .whitespaces)))
            } else if let (marker, rest) = listItem(line) {
                flush()
                let indent = raw.prefix { $0 == " " || $0 == "\t" }.reduce(0) { $0 + ($1 == "\t" ? 2 : 1) } / 2
                blocks.append(.item(indent: min(indent, 6), marker: marker, text: rest))
            } else {
                paragraph.append(line)
            }
        }
        if let code { blocks.append(.code(code.joined(separator: "\n"))) }
        flush()
        return blocks
    }

    private static func headingLevel(_ line: String) -> Int? {
        let hashes = line.prefix { $0 == "#" }.count
        guard (1...6).contains(hashes), line.dropFirst(hashes).first == " " else { return nil }
        return hashes
    }

    /// "- text", "* text", "+ text", "1. text" or "1) text", with task boxes shown as such.
    private static func listItem(_ line: String) -> (String, String)? {
        if let first = line.first, "-*+".contains(first), line.dropFirst().first == " " {
            let rest = String(line.dropFirst(2)).trimmingCharacters(in: .whitespaces)
            if rest.hasPrefix("[ ] ") { return ("☐", String(rest.dropFirst(4))) }
            if rest.lowercased().hasPrefix("[x] ") { return ("☑", String(rest.dropFirst(4))) }
            return ("•", rest)
        }
        let digits = line.prefix { $0.isNumber }
        guard !digits.isEmpty, digits.count <= 3 else { return nil }
        let after = line.dropFirst(digits.count)
        guard let dot = after.first, dot == "." || dot == ")", after.dropFirst().first == " " else { return nil }
        return ("\(digits).", String(after.dropFirst(2)).trimmingCharacters(in: .whitespaces))
    }
}
