import AppKit

/// Formatting and insertion commands shared by the toolbar, Format menu, shortcuts and slash commands.
enum FormatCommand: String, CaseIterable {
    case bold, italic, strikethrough, inlineCode, link, image, imageLink
    case heading1, heading2, heading3, heading4, heading5, heading6, paragraph
    case bulletList, numberedList, taskList, toggleTask, blockquote, codeBlock, table, horizontalRule
    case formatTable, convertToTable
    case indent, outdent, moveLineUp, moveLineDown
    case alertNote, alertTip, alertImportant, alertWarning, alertCaution
    case mermaid, math, footnote, frontMatter, toc, date, time, details, island

    var title: String {
        switch self {
        case .bold: return "Bold"
        case .italic: return "Italic"
        case .strikethrough: return "Strikethrough"
        case .inlineCode: return "Inline Code"
        case .link: return "Link…"
        case .image: return "Attach Image…"
        case .imageLink: return "Image Link"
        case .heading1, .heading2, .heading3, .heading4, .heading5, .heading6: return "Heading \(headingLevel)"
        case .paragraph: return "Paragraph"
        case .bulletList: return "Bullet List"
        case .numberedList: return "Numbered List"
        case .taskList: return "Task List"
        case .toggleTask: return "Toggle Task Checkbox"
        case .blockquote: return "Blockquote"
        case .codeBlock: return "Code Block"
        case .table: return "Insert Table…"
        case .formatTable: return "Format Table"
        case .convertToTable: return "Convert Selection to Table"
        case .horizontalRule: return "Horizontal Rule"
        case .indent: return "Indent"
        case .outdent: return "Outdent"
        case .moveLineUp: return "Move Line Up"
        case .moveLineDown: return "Move Line Down"
        case .alertNote, .alertTip, .alertImportant, .alertWarning, .alertCaution: return alertKind!.capitalized + " Alert"
        case .mermaid: return "Mermaid Diagram"
        case .math: return "Math Block"
        case .footnote: return "Footnote"
        case .frontMatter: return "Front Matter"
        case .toc: return "Table of Contents"
        case .date: return "Today's Date"
        case .time: return "Current Time"
        case .details: return "Details Block"
        case .island: return "Insert Island"
        }
    }

    var toolTip: String? {
        switch self {
        case .image: return "Copy an image file into the image folder and link it"
        case .imageLink: return "Insert a link to a web image. Remote images aren't loaded in the preview; attach a file to embed one."
        case .island: return "Insert a named island for the assistant to target, or wrap the selected lines in one"
        default: return nil
        }
    }

    /// GitHub alert commands, in the order GitHub lists them.
    static let alerts: [FormatCommand] = [.alertNote, .alertTip, .alertImportant, .alertWarning, .alertCaution]

    /// The alert type as written in `> [!TYPE]`.
    var alertKind: String? {
        switch self {
        case .alertNote: return "NOTE"
        case .alertTip: return "TIP"
        case .alertImportant: return "IMPORTANT"
        case .alertWarning: return "WARNING"
        case .alertCaution: return "CAUTION"
        default: return nil
        }
    }

    var headingLevel: Int {
        switch self {
        case .heading1: return 1
        case .heading2: return 2
        case .heading3: return 3
        case .heading4: return 4
        case .heading5: return 5
        case .heading6: return 6
        default: return 0
        }
    }

    var symbol: String {
        switch self {
        case .bold: return "bold"
        case .italic: return "italic"
        case .strikethrough: return "strikethrough"
        case .inlineCode: return "chevron.left.forwardslash.chevron.right"
        case .link: return "link"
        case .image: return "photo"
        case .imageLink: return "photo.badge.arrow.down"
        case .bulletList: return "list.bullet"
        case .numberedList: return "list.number"
        case .taskList: return "checklist"
        case .blockquote: return "text.quote"
        case .codeBlock: return "curlybraces"
        case .table: return "tablecells"
        case .formatTable: return "wand.and.stars"
        case .convertToTable: return "tablecells.badge.ellipsis"
        case .horizontalRule: return "minus"
        case .alertNote: return "info.circle"
        case .alertTip: return "lightbulb"
        case .alertImportant: return "exclamationmark.bubble"
        case .alertWarning: return "exclamationmark.triangle"
        case .alertCaution: return "exclamationmark.octagon"
        case .island: return "square.dashed.inset.filled"
        default: return "textformat"
        }
    }

    /// Key equivalent (characters, modifiers) per spec section 6.2.
    var shortcut: (String, NSEvent.ModifierFlags)? {
        switch self {
        case .bold: return ("b", .command)
        case .italic: return ("i", .command)
        case .strikethrough: return ("x", [.command, .shift])
        case .inlineCode: return ("e", .command)
        case .link: return ("k", .command)
        case .image: return ("i", [.command, .shift])
        case .heading1: return ("1", .command)
        case .heading2: return ("2", .command)
        case .heading3: return ("3", .command)
        case .heading4: return ("4", .command)
        case .heading5: return ("5", .command)
        case .heading6: return ("6", .command)
        case .paragraph: return ("0", .command)
        case .bulletList: return ("8", [.command, .shift])
        case .numberedList: return ("7", [.command, .shift])
        case .taskList: return ("9", [.command, .shift])
        case .toggleTask: return ("\r", .command)
        case .blockquote: return (".", [.command, .shift])
        case .codeBlock: return ("c", [.command, .option])
        case .table: return ("t", [.command, .option, .control])
        case .formatTable: return ("t", [.command, .option])
        case .island: return ("i", [.command, .option])
        case .horizontalRule: return ("-", [.command, .shift])
        case .indent: return ("]", .command)
        case .outdent: return ("[", .command)
        case .moveLineUp: return (String(UnicodeScalar(NSUpArrowFunctionKey)!), [.command, .option])
        case .moveLineDown: return (String(UnicodeScalar(NSDownArrowFunctionKey)!), [.command, .option])
        default: return nil
        }
    }

    /// Maps a slash command id to a format command (nil for slash-only behaviours).
    static func fromSlash(_ id: String) -> FormatCommand? {
        switch id {
        case "h1": return .heading1
        case "h2": return .heading2
        case "h3": return .heading3
        case "h4": return .heading4
        case "h5": return .heading5
        case "h6": return .heading6
        case "bold": return .bold
        case "italic": return .italic
        case "strike": return .strikethrough
        case "code": return .inlineCode
        case "bullet": return .bulletList
        case "numbered": return .numberedList
        case "todo": return .taskList
        case "quote": return .blockquote
        case "codeblock": return .codeBlock
        case "url": return .link
        case "attach": return .image
        case "image": return .imageLink
        case "hr": return .horizontalRule
        case "note": return .alertNote
        case "tip": return .alertTip
        case "important": return .alertImportant
        case "warning": return .alertWarning
        case "caution": return .alertCaution
        case "mermaid": return .mermaid
        case "math": return .math
        case "footnote": return .footnote
        case "frontmatter": return .frontMatter
        case "toc": return .toc
        case "date": return .date
        case "time": return .time
        case "details": return .details
        case "island": return .island
        default: return nil
        }
    }

    func menuItem() -> NSMenuItem {
        let item = NSMenuItem(title: title, action: #selector(ProjectWindowController.performFormatCommand(_:)), keyEquivalent: "")
        item.representedObject = rawValue
        if alertKind != nil { item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil) }
        item.toolTip = toolTip
        if let (key, mods) = shortcut {
            item.keyEquivalent = key
            item.keyEquivalentModifierMask = mods
        }
        return item
    }
}
