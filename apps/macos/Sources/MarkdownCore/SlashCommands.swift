import Foundation

/// A slash command shown in the `/` popup.
public struct SlashCommand: Identifiable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let aliases: [String]
    public let title: String
    public let symbol: String
    /// Only offered when the caret is on the first line of the document.
    public let firstLineOnly: Bool

    public init(_ id: String, _ title: String, aliases: [String] = [], symbol: String = "textformat", firstLineOnly: Bool = false) {
        self.id = id
        self.name = id
        self.aliases = aliases
        self.title = title
        self.symbol = symbol
        self.firstLineOnly = firstLineOnly
    }
}

public enum SlashCommandCatalog {
    public static let all: [SlashCommand] = [
        // Structured blocks of a video spec, first because they are what a spec is made of.
        SlashCommand("scene", "Scene", aliases: ["section"], symbol: "film"),
        SlashCommand("record", "Recorded app scene", aliases: ["recording", "capture", "screen"], symbol: "record.circle"),
        SlashCommand("vo", "Voice-over", aliases: ["voiceover", "narration", "script"], symbol: "waveform"),
        SlashCommand("shot", "Shot", aliases: ["clip"], symbol: "camera"),
        SlashCommand("music", "Music brief", aliases: ["soundtrack"], symbol: "music.note"),
        SlashCommand("cta", "Call to action", aliases: ["calltoaction"], symbol: "hand.point.up.left"),
        SlashCommand("h1", "Heading 1", aliases: ["heading"], symbol: "textformat.size.larger"),
        SlashCommand("h2", "Heading 2", aliases: ["heading"], symbol: "textformat.size"),
        SlashCommand("h3", "Heading 3", aliases: ["heading"], symbol: "textformat.size"),
        SlashCommand("h4", "Heading 4", aliases: ["heading"], symbol: "textformat.size.smaller"),
        SlashCommand("h5", "Heading 5", aliases: ["heading"], symbol: "textformat.size.smaller"),
        SlashCommand("h6", "Heading 6", aliases: ["heading"], symbol: "textformat.size.smaller"),
        SlashCommand("bold", "Bold", symbol: "bold"),
        SlashCommand("italic", "Italic", symbol: "italic"),
        SlashCommand("strike", "Strikethrough", aliases: ["strikethrough"], symbol: "strikethrough"),
        SlashCommand("code", "Inline code", symbol: "chevron.left.forwardslash.chevron.right"),
        SlashCommand("bullet", "Bullet list", aliases: ["ul", "list"], symbol: "list.bullet"),
        SlashCommand("numbered", "Numbered list", aliases: ["ol"], symbol: "list.number"),
        SlashCommand("todo", "Task list", aliases: ["task", "checkbox"], symbol: "checklist"),
        SlashCommand("quote", "Blockquote", aliases: ["blockquote"], symbol: "text.quote"),
        SlashCommand("codeblock", "Code block", aliases: ["fence"], symbol: "curlybraces"),
        SlashCommand("table", "Table", symbol: "tablecells"),
        SlashCommand("url", "Link", aliases: ["link"], symbol: "link"),
        SlashCommand("attach", "Add media", aliases: ["media", "upload", "picture", "photo", "video", "audio"], symbol: "paperclip"),
        SlashCommand("image", "Image link", aliases: ["img"], symbol: "photo"),
        SlashCommand("hr", "Horizontal rule", aliases: ["divider"], symbol: "minus"),
        SlashCommand("note", "Note alert", aliases: ["alert", "callout"], symbol: "info.circle"),
        SlashCommand("tip", "Tip alert", aliases: ["alert", "callout"], symbol: "lightbulb"),
        SlashCommand("important", "Important alert", aliases: ["alert", "callout"], symbol: "exclamationmark.bubble"),
        SlashCommand("warning", "Warning alert", aliases: ["alert", "callout"], symbol: "exclamationmark.triangle"),
        SlashCommand("caution", "Caution alert", aliases: ["alert", "callout"], symbol: "exclamationmark.octagon"),
        SlashCommand("mermaid", "Mermaid diagram", symbol: "flowchart"),
        SlashCommand("math", "Math block", symbol: "function"),
        SlashCommand("footnote", "Footnote", symbol: "textformat.superscript"),
        SlashCommand("frontmatter", "Front matter", symbol: "doc.text", firstLineOnly: true),
        SlashCommand("toc", "Table of contents", symbol: "list.bullet.indent"),
        SlashCommand("date", "Today's date", symbol: "calendar"),
        SlashCommand("time", "Current time", symbol: "clock"),
        SlashCommand("details", "Details block", symbol: "chevron.right.square"),
        SlashCommand("island", "Island (named region for the assistant)", aliases: ["fragment", "region"], symbol: "square.dashed.inset.filled"),
    ]

    /// Fuzzy score: higher is better; nil means no match. Matches subsequences of name, aliases and title.
    public static func score(_ query: String, _ candidate: String) -> Int? {
        let q = Array(query.lowercased()), c = Array(candidate.lowercased())
        if q.isEmpty { return 1 }
        if c == q { return 1000 }
        if c.starts(with: q) { return 800 - c.count }
        var qi = 0, score = 0, lastMatch = -2, firstMatch = -1
        for (i, ch) in c.enumerated() where qi < q.count && ch == q[qi] {
            if firstMatch < 0 { firstMatch = i }
            score += (i == lastMatch + 1) ? 10 : 3
            lastMatch = i
            qi += 1
        }
        guard qi == q.count else { return nil }
        return score - firstMatch
    }

    /// Filters and ranks commands for a query (text typed after `/`).
    public static func filter(_ query: String, onFirstLine: Bool = true) -> [SlashCommand] {
        let pool = all.filter { onFirstLine || !$0.firstLineOnly }
        let scored: [(SlashCommand, Int, Int)] = pool.enumerated().compactMap { idx, cmd in
            let candidates = [cmd.name] + cmd.aliases + [cmd.title.replacingOccurrences(of: " ", with: "")]
            let best = candidates.compactMap { score(query, $0) }.max()
            return best.map { (cmd, $0, idx) }
        }
        return scored.sorted { $0.1 != $1.1 ? $0.1 > $1.1 : $0.2 < $1.2 }.map(\.0)
    }
}
