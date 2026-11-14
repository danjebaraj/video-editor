import Foundation

// Islands: named regions marked by HTML comments, for targeting AI edits.
//
//     <!-- island: dashing-tornado -->
//     content
//     <!-- /island: dashing-tornado -->
//
// See "island support.md". Ranges are UTF-16 (NSString) offsets; lines are 0-based.

public struct IslandMarker: Equatable, Sendable {
    public var name: String
    public var isEnd: Bool
    public var line: Int
    /// The marker line, without its newline.
    public var range: NSRange
}

public struct Island: Equatable, Sendable {
    public var name: String
    public var start: IslandMarker
    public var end: IslandMarker
    /// Set when the island has a problem (duplicate or invalid name); it can't be referenced.
    public var problem: String?

    /// Lines strictly between the markers.
    public var contentLines: Range<Int> { (start.line + 1)..<end.line }
    /// From the start of the line after the start marker to the start of the end marker line.
    public var contentRange: NSRange {
        let s = NSMaxRange(start.range) + 1
        return NSRange(location: min(s, end.range.location), length: max(0, end.range.location - s))
    }
    /// Both markers and everything between them, without the final newline.
    public var fullRange: NSRange { NSRange(location: start.range.location, length: NSMaxRange(end.range) - start.range.location) }
    public var isUsable: Bool { problem == nil }
}

public struct IslandProblem: Equatable, Sendable {
    public enum Kind: Equatable, Sendable { case unclosed, unmatchedEnd, duplicate, nested, invalidName }
    public var kind: Kind
    public var marker: IslandMarker

    public var message: String {
        switch kind {
        case .unclosed: return "Island “\(marker.name)” has no end marker."
        case .unmatchedEnd: return "End marker for “\(marker.name)” has no matching start marker."
        case .duplicate: return "The island name “\(marker.name)” is already used in this file."
        case .nested: return "Island “\(marker.name)” starts inside another island. Nested islands aren't supported."
        case .invalidName: return "“\(marker.name)” isn't a valid island name (lowercase letters, digits and single dashes, up to 40 characters)."
        }
    }
}

public struct IslandScan: Equatable, Sendable {
    /// Paired islands in order, including ones with a problem (see `Island.problem`).
    public var islands: [Island] = []
    public var problems: [IslandProblem] = []
    /// Every marker outside code, in order.
    public var markers: [IslandMarker] = []

    public init() {}

    public func island(named name: String) -> Island? { islands.first { $0.name == name && $0.isUsable } }

    /// The island whose markers or content include `location`.
    public func island(at location: Int) -> Island? {
        islands.first { NSLocationInRange(location, $0.fullRange) || location == NSMaxRange($0.fullRange) }
    }

    public func island(atLine line: Int) -> Island? { islands.first { (it: Island) in it.start.line <= line && line <= it.end.line } }

    public var names: Set<String> { Set(markers.map(\.name)) }
}

public enum Islands {
    public static let maxNameLength = 40

    public static func isValidName(_ name: String) -> Bool {
        name.count <= maxNameLength && name.range(of: "^[a-z0-9]+(-[a-z0-9]+)*$", options: .regularExpression) != nil
    }

    private static let markerRx = try! NSRegularExpression(pattern: "^[ \\t]{0,3}<!--\\s*(/?)\\s*island\\s*:\\s*(.*?)\\s*-->[ \\t]*$", options: .caseInsensitive)

    /// Parses a whole line as a marker: `<!-- island: name -->` or `<!-- /island: name -->`
    /// (extra whitespace allowed, `island` in any case).
    public static func marker(inLine line: String) -> (name: String, isEnd: Bool)? {
        let ns = line as NSString
        guard let m = markerRx.firstMatch(in: line, range: NSRange(location: 0, length: ns.length)) else { return nil }
        return (ns.substring(with: m.range(at: 2)), m.range(at: 1).length > 0)
    }

    public static func startMarker(_ name: String) -> String { "<!-- island: \(name) -->" }
    public static func endMarker(_ name: String) -> String { "<!-- /island: \(name) -->" }

    /// Finds markers outside fenced and indented code (a marker must be a whole line, so markers in
    /// inline code are text), pairs them and reports problems.
    public static func scan(_ source: String) -> IslandScan {
        let ns = source as NSString
        var result = IslandScan()
        guard ns.range(of: "<!--").location != NSNotFound else { return result }
        var fence: (char: Character, count: Int)?
        var previousBlank = true
        var inIndentedCode = false
        var pos = 0, line = 0
        while pos <= ns.length {
            var lineEnd = 0, contentsEnd = 0
            if pos < ns.length {
                ns.getLineStart(nil, end: &lineEnd, contentsEnd: &contentsEnd, for: NSRange(location: pos, length: 0))
            } else {
                lineEnd = pos; contentsEnd = pos
            }
            let r = NSRange(location: pos, length: contentsEnd - pos)
            let text = ns.substring(with: r)
            let trimmed = text.drop { $0 == " " }
            let indent = text.count - trimmed.count
            let blank = trimmed.trimmingCharacters(in: .whitespaces).isEmpty
            if let f = fence {
                let run = trimmed.prefix { $0 == f.char }.count
                if indent < 4, run >= f.count, trimmed.dropFirst(run).allSatisfy({ $0 == " " || $0 == "\t" }) { fence = nil }
            } else if indent < 4, let c = trimmed.first, c == "`" || c == "~", trimmed.prefix(while: { $0 == c }).count >= 3,
                      !(c == "`" && trimmed.dropFirst(trimmed.prefix(while: { $0 == c }).count).contains("`")) {
                fence = (c, trimmed.prefix { $0 == c }.count)
            } else if (indent >= 4 || text.hasPrefix("\t")) && previousBlank && !blank {
                // Indented code block (after a blank line or more code): never a marker.
                inIndentedCode = true
            } else if let m = marker(inLine: text) {
                result.markers.append(IslandMarker(name: m.name, isEnd: m.isEnd, line: line, range: r))
            }
            if fence == nil && !(inIndentedCode && !blank && (indent >= 4 || text.hasPrefix("\t"))) { inIndentedCode = false }
            // An indented code line keeps the next indented line in code.
            previousBlank = blank || inIndentedCode
            if pos == ns.length { break }
            pos = lineEnd
            line += 1
        }
        pair(&result)
        return result
    }

    private static func pair(_ result: inout IslandScan) {
        var open: IslandMarker?
        var nestedOpen: [IslandMarker] = []
        var used = Set<String>()
        for m in result.markers {
            if !m.isEnd {
                if open != nil {
                    result.problems.append(IslandProblem(kind: .nested, marker: m))
                    nestedOpen.append(m)
                } else {
                    open = m
                }
                continue
            }
            if let i = nestedOpen.lastIndex(where: { $0.name == m.name }) {
                nestedOpen.remove(at: i)
            } else if let o = open, o.name == m.name {
                var island = Island(name: o.name, start: o, end: m)
                if !isValidName(o.name) {
                    island.problem = IslandProblem(kind: .invalidName, marker: o).message
                    result.problems.append(IslandProblem(kind: .invalidName, marker: o))
                } else if used.contains(o.name) {
                    island.problem = IslandProblem(kind: .duplicate, marker: o).message
                    result.problems.append(IslandProblem(kind: .duplicate, marker: o))
                }
                used.insert(o.name)
                result.islands.append(island)
                open = nil
            } else {
                result.problems.append(IslandProblem(kind: .unmatchedEnd, marker: m))
            }
        }
        if let o = open { result.problems.append(IslandProblem(kind: .unclosed, marker: o)) }
        for n in nestedOpen where !result.problems.contains(where: { $0.marker == n }) {
            result.problems.append(IslandProblem(kind: .unclosed, marker: n))
        }
        result.problems.sort { $0.marker.line < $1.marker.line }
    }

    // MARK: Editing

    /// Inserts an empty island at the caret (caret on the blank line between the markers), or wraps the
    /// selected lines. Markers get their own lines, with blank lines outside the island where needed.
    public static func insert(_ text: NSString, selection: NSRange, name: String) -> TextEdit {
        let start = startMarker(name), end = endMarker(name)
        if selection.length == 0 {
            let edit = MarkdownEditing.insertBlock(text, selection: NSRange(location: selection.location, length: 0), block: start + "\n\n" + end,
                                                   selectInBlock: NSRange(location: (start as NSString).length + 1, length: 0))
            return edit
        }
        let lines = text.lineRange(for: selection)
        var body = text.substring(with: lines)
        let hadNewline = body.hasSuffix("\n")
        if hadNewline { body.removeLast() }
        var prefix = "", suffix = hadNewline ? "\n" : ""
        if lines.location > 0 {
            let prev = text.lineRange(for: NSRange(location: lines.location - 1, length: 0))
            if !text.substring(with: prev).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { prefix = "\n" }
        }
        if NSMaxRange(lines) < text.length {
            let next = text.lineRange(for: NSRange(location: NSMaxRange(lines), length: 0))
            if !text.substring(with: next).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { suffix += "\n" }
        }
        let rep = prefix + start + "\n" + body + "\n" + end + suffix
        let caret = lines.location + (prefix + start + "\n" + body as NSString).length
        return TextEdit(range: lines, replacement: rep, selection: NSRange(location: caret, length: 0))
    }

    /// Removes both marker lines, keeping the content.
    public static func unwrap(_ text: NSString, _ island: Island) -> TextEdit {
        let content = text.substring(with: island.contentRange)
        let body = content.hasSuffix("\n") ? String(content.dropLast()) : content
        let range = lineRangeWithNewline(text, island.fullRange)
        let keepNewline = NSMaxRange(range) > NSMaxRange(island.fullRange) && !body.isEmpty
        let rep = body.isEmpty ? "" : body + (keepNewline ? "\n" : "")
        return TextEdit(range: range, replacement: rep, selection: NSRange(location: range.location, length: 0))
    }

    /// Removes the markers and the content.
    public static func delete(_ text: NSString, _ island: Island) -> TextEdit {
        let range = lineRangeWithNewline(text, island.fullRange)
        return TextEdit(range: range, replacement: "", selection: NSRange(location: range.location, length: 0))
    }

    /// Rewrites both markers with `newName`, in canonical form, keeping the caret's offset.
    public static func rename(_ text: NSString, _ island: Island, to newName: String) -> TextEdit {
        let s = startMarker(newName), e = endMarker(newName)
        let content = text.substring(with: NSRange(location: NSMaxRange(island.start.range), length: island.end.range.location - NSMaxRange(island.start.range)))
        let rep = s + content + e
        return TextEdit(range: island.fullRange, replacement: rep, selection: NSRange(location: island.fullRange.location, length: 0))
    }

    /// Removes one marker line.
    public static func removeMarker(_ text: NSString, _ marker: IslandMarker) -> TextEdit {
        let range = lineRangeWithNewline(text, marker.range)
        return TextEdit(range: range, replacement: "", selection: NSRange(location: range.location, length: 0))
    }

    /// Quick fix for a start marker without an end: adds the end marker after the next block
    /// (the lines up to the next blank line, or the end of the text).
    public static func closeAfterNextBlock(_ text: NSString, _ marker: IslandMarker) -> TextEdit {
        var pos = NSMaxRange(marker.range) < text.length ? NSMaxRange(marker.range) + 1 : text.length
        // Skip blank lines right after the marker, then the block itself.
        var sawContent = false
        while pos < text.length {
            let line = text.lineRange(for: NSRange(location: pos, length: 0))
            let blank = text.substring(with: line).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            if blank && sawContent { break }
            if let m = Islands.marker(inLine: text.substring(with: line).trimmingCharacters(in: .newlines)), m.name == marker.name || !m.isEnd { break }
            if !blank { sawContent = true }
            pos = NSMaxRange(line)
        }
        let atLineStart = pos == 0 || text.character(at: pos - 1) == 10
        let rep = (atLineStart ? "" : "\n") + endMarker(marker.name) + (pos < text.length ? "\n" : "")
        return TextEdit(range: NSRange(location: pos, length: 0), replacement: rep, selection: NSRange(location: pos + (rep as NSString).length, length: 0))
    }

    /// Replaces the name in one marker line.
    public static func renameMarker(_ text: NSString, _ marker: IslandMarker, to newName: String) -> TextEdit {
        let rep = marker.isEnd ? endMarker(newName) : startMarker(newName)
        return TextEdit(range: marker.range, replacement: rep, selection: NSRange(location: marker.range.location, length: 0))
    }

    public static func lineRangeWithNewline(_ text: NSString, _ range: NSRange) -> NSRange {
        var r = range
        if NSMaxRange(r) < text.length, text.character(at: NSMaxRange(r)) == 10 { r.length += 1 }
        else if r.location > 0, text.character(at: r.location - 1) == 10, NSMaxRange(r) >= text.length { r.location -= 1; r.length += 1 }
        return r
    }

    /// Renames islands in `pasted` whose names are already taken in the file, so a pasted copy never
    /// clashes. Returns the new text and the (old, new) pairs.
    public static func renamingConflicts(in pasted: String, taken: Set<String>, generate: (Set<String>) -> String) -> (text: String, renamed: [(String, String)]) {
        let scan = scan(pasted)
        var taken = taken
        var renamed: [(String, String)] = []
        let out = NSMutableString(string: pasted)
        for island in scan.islands.reversed() where taken.contains(island.name) {
            let new = generate(taken.union(scan.names))
            taken.insert(new)
            out.replaceCharacters(in: island.end.range, with: endMarker(new))
            out.replaceCharacters(in: island.start.range, with: startMarker(new))
            renamed.insert((island.name, new), at: 0)
        }
        return (out as String, renamed)
    }

    // MARK: Mentions

    private static let mentionRx = try! NSRegularExpression(pattern: "(?<![\\w@/])@([a-z0-9]+(?:-[a-z0-9]+)*)(?![\\w-])")

    /// `@name` tokens in a message, in order, without duplicates.
    public static func mentions(in message: String) -> [String] {
        let ns = message as NSString
        var out: [String] = []
        for m in mentionRx.matches(in: message, range: NSRange(location: 0, length: ns.length)) {
            let n = ns.substring(with: m.range(at: 1))
            if !out.contains(n) { out.append(n) }
        }
        return out
    }

    /// Replaces `@old` mentions with `@new`.
    public static func replacingMentions(of old: String, with new: String, in message: String) -> String {
        let rx = try! NSRegularExpression(pattern: "(?<![\\w@/])@\(NSRegularExpression.escapedPattern(for: old))(?![\\w-])")
        return rx.stringByReplacingMatches(in: message, range: NSRange(location: 0, length: (message as NSString).length), withTemplate: "@\(new)")
    }

    // MARK: Review guard rails

    /// Warnings for each hunk of a change to a file that referenced islands: a hunk that changes or
    /// removes a marker, or edits text outside every referenced island.
    public static func guardWarnings(original: String, hunks: [DiffHunk], referenced: Set<String>) -> [Int: [String]] {
        let scan = scan(original)
        let targets = scan.islands.filter { referenced.contains($0.name) }
        let markerLines = Set(scan.markers.map(\.line))
        var out: [Int: [String]] = [:]
        for h in hunks {
            var w: [String] = []
            let removed = h.originalStart..<(h.originalStart + h.removed.count)
            if removed.contains(where: markerLines.contains) || h.added.contains(where: { marker(inLine: $0) != nil }) {
                w.append("Changes an island marker.")
            }
            let inside = targets.contains { island in
                let content = island.contentLines
                if h.removed.isEmpty { return h.originalStart >= content.lowerBound && h.originalStart <= content.upperBound }
                return removed.lowerBound >= content.lowerBound && removed.upperBound <= content.upperBound
            }
            if !inside { w.append(targets.isEmpty ? "Edits a file with none of the referenced islands." : "Edits text outside the referenced islands.") }
            if !w.isEmpty { out[h.id] = w }
        }
        return out
    }
}

// MARK: Names

/// Friendly `adjective-noun` names from bundled word lists.
public enum IslandNames {
    public static let adjectives = [
        "amazing", "bold", "brave", "bright", "brisk", "calm", "clever", "cosmic", "crisp", "curious", "dapper", "daring", "dashing",
        "eager", "electric", "fancy", "fearless", "fierce", "fluffy", "fresh", "friendly", "gentle", "giant", "gleaming", "golden",
        "graceful", "happy", "hidden", "humble", "icy", "jolly", "keen", "kind", "lively", "lucky", "lunar", "magic", "mellow",
        "merry", "mighty", "misty", "modest", "nimble", "noble", "patient", "plucky", "polar", "proud", "quick", "quiet", "radiant",
        "rapid", "rustic", "serene", "shiny", "silent", "silver", "sleek", "smooth", "snowy", "solar", "spry", "steady", "stellar",
        "sturdy", "sunny", "swift", "tidy", "tiny", "tranquil", "valiant", "vivid", "warm", "wild", "wise", "witty", "zesty",
    ]
    public static let nouns = [
        "anchor", "badger", "beacon", "bison", "breeze", "brook", "canyon", "cedar", "comet", "coral", "crane", "dawn", "delta",
        "dog", "dolphin", "dune", "eagle", "ember", "falcon", "fern", "fjord", "forest", "fox", "galaxy", "glacier", "harbor",
        "hawk", "heron", "island", "lagoon", "lantern", "lark", "lynx", "maple", "meadow", "meteor", "moose", "nebula", "oak",
        "orbit", "otter", "owl", "panda", "pebble", "pine", "planet", "prairie", "quartz", "raven", "reef", "river", "robin",
        "sparrow", "spruce", "summit", "tiger", "tornado", "trail", "tundra", "valley", "violet", "voyage", "walrus", "willow",
        "wolf", "zephyr",
    ]

    /// A name not in `taken`: random pairs first, then the first pair with a numeric suffix.
    public static func generate<R: RandomNumberGenerator>(avoiding taken: Set<String>, using rng: inout R) -> String {
        for _ in 0..<50 {
            let n = "\(adjectives.randomElement(using: &rng)!)-\(nouns.randomElement(using: &rng)!)"
            if !taken.contains(n) { return n }
        }
        let base = "\(adjectives.randomElement(using: &rng)!)-\(nouns.randomElement(using: &rng)!)"
        var i = 2
        while taken.contains("\(base)-\(i)") { i += 1 }
        return "\(base)-\(i)"
    }

    public static func generate(avoiding taken: Set<String>) -> String {
        var rng = SystemRandomNumberGenerator()
        return generate(avoiding: taken, using: &rng)
    }
}
