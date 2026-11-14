import SwiftUI
import AppKit
import UniformTypeIdentifiers

/// Sidebar: Outline (spec headings and scenes), Media (by kind, with status), Runs (history).
struct SidebarView: View {
    @ObservedObject var document: ProjectDocument

    var body: some View {
        VStack(spacing: 0) {
            Picker("", selection: $document.sidebarTab) {
                ForEach(SidebarTab.allCases) { tab in
                    Label(tab.title, systemImage: tab.symbol).tag(tab)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(8)
            .accessibilityLabel("Sidebar section")
            Divider()
            switch document.sidebarTab {
            case .outline: OutlineList(document: document)
            case .media: MediaList(document: document)
            case .runs: RunsList(document: document)
            }
        }
        .background(.background)
    }
}

/// An empty state with a symbol, a title and one line of help.
struct EmptyState: View {
    var symbol: String
    var title: String
    var message: String
    var action: (title: String, run: () -> Void)?

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: symbol).font(.system(size: 28)).foregroundStyle(.tertiary)
            Text(title).font(.headline)
            Text(message).font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
            if let action { Button(action.title, action: action.run).padding(.top, 4) }
        }
        .padding(20)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

// MARK: Outline

struct OutlineList: View {
    @ObservedObject var document: ProjectDocument

    var body: some View {
        if document.outline.isEmpty {
            EmptyState(symbol: "list.bullet.indent", title: "No headings yet", message: "Headings and scenes in the spec appear here. Type /scene in the editor to add a scene.")
        } else {
            List(document.outline) { item in
                Button {
                    document.jump(toLine: item.line)
                    if document.showPreview { document.preview.scrollToLine(item.line - 1) }
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: item.kind == "scene" ? "film" : "number")
                            .font(.caption).foregroundStyle(item.kind == "scene" ? Color.accentColor : Color.secondary).frame(width: 14)
                        Text(item.title).lineLimit(1).font(item.level <= 1 ? .body.weight(.semibold) : .body)
                        Spacer(minLength: 0)
                        if document.lint.contains(where: { $0.line == item.line && $0.severity == "warning" }) {
                            Image(systemName: "exclamationmark.triangle.fill").font(.caption2).foregroundStyle(.orange)
                        }
                    }
                    .padding(.leading, CGFloat(max(0, item.level - 1)) * 12)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(item.kind == "scene" ? "Scene" : "Heading level \(item.level)"): \(item.title)")
            }
            .listStyle(.sidebar)
        }
    }
}

// MARK: Media

/// A small picture of a media file, made off the main thread.
struct MediaThumbnail: View {
    var url: URL?
    var kind: String
    var hash: String
    @State private var image: NSImage?

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 4).fill(Color(nsColor: .quaternaryLabelColor))
            if let image {
                Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
            } else {
                Image(systemName: MediaItem.symbol(kind)).foregroundStyle(.secondary)
            }
        }
        .frame(width: 44, height: 30)
        .clipShape(RoundedRectangle(cornerRadius: 4))
        .task(id: (url?.path ?? "") + hash) {
            guard let url, kind == "video" || kind == "image" else { image = nil; return }
            let k = kind
            let data = await Task.detached(priority: .utility) { MediaSchemeHandler.picture(of: url, kind: k, duration: 0, index: 0) }.value
            image = data.flatMap(NSImage.init(data:))
        }
        .accessibilityHidden(true)
    }
}

struct MediaList: View {
    @ObservedObject var document: ProjectDocument
    @State private var dropTargeted = false

    private var groups: [(kind: String, label: String, items: [MediaItem])] {
        MediaItem.kinds.compactMap { k in
            let items = document.project.media.filter { $0.kind == k.key }
            return items.isEmpty ? nil : (k.key, k.label, items)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            if !document.missingMedia.isEmpty {
                Button { document.showRelinkSheet = true } label: {
                    Label("\(document.missingMedia.count) missing — Relink…", systemImage: "exclamationmark.triangle.fill")
                        .frame(maxWidth: .infinity)
                }
                .tint(.red)
                .controlSize(.small)
                .padding(8)
            }
            if document.project.media.isEmpty {
                EmptyState(symbol: "photo.on.rectangle", title: "No media yet", message: "Drop files or folders here. Media is used where it is; nothing is copied.",
                           action: ("Add Media…", { document.windowController?.addMedia(nil) }))
            } else {
                List(selection: $document.selectedMediaID) {
                    ForEach(groups, id: \.kind) { group in
                        Section(group.label) {
                            ForEach(group.items) { item in MediaRow(document: document, item: item).tag(item.id as String?) }
                        }
                    }
                }
                .listStyle(.sidebar)
                if let id = document.selectedMediaID, let item = document.project.mediaItem(id) {
                    Divider()
                    MediaDetail(document: document, item: item)
                }
            }
            Divider()
            HStack {
                Button { document.windowController?.addMedia(nil) } label: { Image(systemName: "plus") }.help("Add media files or folders (⇧⌘I)")
                    .accessibilityLabel("Add media")
                Spacer()
                Text("\(document.project.media.count) item\(document.project.media.count == 1 ? "" : "s")").font(.caption).foregroundStyle(.secondary)
            }
            .buttonStyle(.borderless)
            .padding(.horizontal, 10).frame(height: 28)
        }
        .overlay { if dropTargeted { RoundedRectangle(cornerRadius: 6).strokeBorder(Color.accentColor, lineWidth: 2).padding(3) } }
        .onDrop(of: [.fileURL], isTargeted: $dropTargeted) { providers in
            Task {
                var urls: [URL] = []
                for p in providers {
                    if let url = try? await p.loadItem(forTypeIdentifier: UTType.fileURL.identifier) as? URL { urls.append(url) }
                    else if let data = try? await p.loadItem(forTypeIdentifier: UTType.fileURL.identifier) as? Data, let url = URL(dataRepresentation: data, relativeTo: nil) { urls.append(url) }
                }
                await MainActor.run { _ = document.addMedia(urls) }
            }
            return true
        }
    }
}

struct MediaRow: View {
    @ObservedObject var document: ProjectDocument
    var item: MediaItem

    var body: some View {
        let status = document.mediaStatus(item.id)
        HStack(spacing: 8) {
            MediaThumbnail(url: status == "missing" ? nil : document.mediaURL(item.id), kind: item.kind, hash: item.fingerprint?.quick_hash ?? "")
            VStack(alignment: .leading, spacing: 1) {
                Text(item.fileName).lineLimit(1).truncationMode(.middle)
                HStack(spacing: 4) {
                    Text(item.id).font(.caption.monospaced())
                    if let d = item.probe?.duration_s, d > 0 { Text("· " + Fmt.duration(d)).font(.caption) }
                }
                .foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer(minLength: 0)
            if status == "missing" {
                Text("Missing").font(.caption2.weight(.semibold)).padding(.horizontal, 5).padding(.vertical, 1).background(Color.red, in: Capsule()).foregroundStyle(.white)
            } else if status == "changed" {
                Text("Changed").font(.caption2.weight(.semibold)).padding(.horizontal, 5).padding(.vertical, 1).background(Color.orange, in: Capsule()).foregroundStyle(.white)
            }
        }
        .padding(.vertical, 1)
        .contextMenu {
            Button("Insert in Spec") { document.editor.insert(MediaLink.markdown(id: item.id, alt: item.fileName)) }
            Button("Copy Reference") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(MediaLink.markdown(id: item.id, alt: item.fileName), forType: .string)
            }
            Divider()
            if let url = document.mediaURL(item.id) {
                Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([url]) }
                Button("Quick Look") { document.preview.quickLook(url) }
            }
            Button("Relink…") { MediaActions.locate(item.id, in: document) }
            if status == "changed" { Button("Accept Changed File") { document.acceptChanged(item.id) } }
            Divider()
            Button("Remove from Project", role: .destructive) { MediaActions.remove(item, from: document) }
        }
        .onDrag { NSItemProvider(object: MediaLink.markdown(id: item.id, alt: item.fileName) as NSString) }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(item.fileName), \(item.kind)\(status == "missing" ? ", missing" : status == "changed" ? ", changed" : "")")
    }
}

@MainActor
enum MediaActions {
    /// Locate…: pick the file for one item; offers the prefix fix for the others (spec §6.3).
    static func locate(_ id: String, in document: ProjectDocument) {
        let panel = NSOpenPanel()
        panel.message = "Choose the file for “\(document.project.mediaItem(id)?.fileName ?? id)”"
        panel.prompt = "Relink"
        guard panel.runModal() == .OK, let url = panel.url, let result = document.relink(id, to: url) else { return }
        guard let fix = result.prefix_fix, !result.candidates.isEmpty else { return }
        let n = result.candidates.count
        if Prompt.confirm("Relink \(n) more item\(n == 1 ? "" : "s") the same way?",
                          message: "\(n == 1 ? "Another missing item is" : "\(n) other missing items are") under the same folder.\n\n\(fix.old_prefix)\nbecomes\n\(fix.new_prefix)", ok: "Apply Prefix Fix") {
            _ = document.applyPrefixFix(fix)
        }
    }

    static func remove(_ item: MediaItem, from document: ProjectDocument) {
        let used = MediaLink.ids(in: document.project.spec).contains(item.id)
        if used, !Prompt.confirm("Remove “\(item.fileName)”?", message: "The spec still refers to media:\(item.id). The file itself isn't touched.", ok: "Remove", destructive: true) { return }
        document.removeMedia(item.id)
        if document.selectedMediaID == item.id { document.selectedMediaID = nil }
    }
}

/// The selected media item: role, notes and tags for Claude, and where the file is.
struct MediaDetail: View {
    @ObservedObject var document: ProjectDocument
    var item: MediaItem

    private func set(_ change: @escaping (inout MediaItem) -> Void) {
        document.update { p in
            if let i = p.media.firstIndex(where: { $0.id == item.id }) { change(&p.media[i]) }
        }
    }

    var body: some View {
        let status = document.mediaStatus(item.id)
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(item.id).font(.caption.monospaced()).textSelection(.enabled)
                Spacer()
                Text(PreviewController.describe(item)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Picker("Role", selection: Binding(get: { item.role ?? "" }, set: { v in set { $0.role = v.isEmpty ? nil : v } })) {
                Text("Not set").tag("")
                ForEach(MediaItem.roles, id: \.key) { Text($0.label).tag($0.key) }
            }
            .help("A hint for Claude about how this file is meant to be used")
            TextField("Notes for Claude", text: Binding(get: { item.notes ?? "" }, set: { v in set { $0.notes = v.isEmpty ? nil : v } }), axis: .vertical)
                .lineLimit(1...3)
            TextField("Tags, separated by commas", text: Binding(get: { (item.tags ?? []).joined(separator: ", ") }, set: { v in
                let tags = v.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                set { $0.tags = tags.isEmpty ? nil : tags }
            }))
            Text(document.mediaURL(item.id)?.path ?? item.paths.absolute ?? "").font(.caption2).foregroundStyle(status == "missing" ? Color.red : Color.secondary).lineLimit(2).truncationMode(.middle).textSelection(.enabled)
            if let err = item.probe?.error { Text(err).font(.caption2).foregroundStyle(.red).lineLimit(2) }
            HStack {
                if status == "changed" { Button("Accept Change") { document.acceptChanged(item.id) }.help("Use the file as it is now") }
                Button("Relink…") { MediaActions.locate(item.id, in: document) }
                if let url = document.mediaURL(item.id) { Button("Reveal") { NSWorkspace.shared.activateFileViewerSelecting([url]) } }
            }
        }
        .textFieldStyle(.roundedBorder)
        .controlSize(.small)
        .padding(10)
    }
}

// MARK: Relink sheet (spec §6.3)

struct RelinkSheet: View {
    @ObservedObject var document: ProjectDocument
    @State private var lastResult = ""
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Relink Media").font(.title3.weight(.semibold))
            if document.missingMedia.isEmpty {
                Label("Every media item was found.", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
            } else {
                Text("\(document.missingMedia.count) file\(document.missingMedia.count == 1 ? " wasn't" : "s weren't") found. Locate one and the others under the same folder can follow, or search a folder for all of them.")
                    .foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                List(document.missingMedia) { r in
                    HStack {
                        Image(systemName: MediaItem.symbol(document.project.mediaItem(r.id)?.kind ?? "other")).foregroundStyle(.secondary).frame(width: 18)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(document.project.mediaItem(r.id)?.fileName ?? r.id)
                            Text(r.stored_path ?? "").font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                        }
                        Spacer()
                        Button("Locate…") { MediaActions.locate(r.id, in: document) }
                    }
                }
                .frame(minHeight: 160, maxHeight: 280)
                .clipShape(RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color(nsColor: .separatorColor)))
            }
            if !lastResult.isEmpty { Text(lastResult).font(.callout).foregroundStyle(.secondary) }
            HStack {
                Button("Search Folder…") {
                    let panel = NSOpenPanel()
                    panel.canChooseDirectories = true
                    panel.canChooseFiles = false
                    panel.message = "Missing files are matched by name, then by content."
                    panel.prompt = "Search"
                    guard panel.runModal() == .OK, let url = panel.url else { return }
                    let found = document.searchFolder(url)
                    lastResult = found.isEmpty ? "Nothing in \(url.lastPathComponent) matched." : "Relinked \(found.count) item\(found.count == 1 ? "" : "s") from \(url.lastPathComponent)."
                }
                .disabled(document.missingMedia.isEmpty)
                .help("Scan a folder and match every missing item by name, then fingerprint")
                Spacer()
                Button(document.missingMedia.isEmpty ? "Done" : "Later") { dismiss() }.keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 560)
    }
}

// MARK: Runs

struct RunsList: View {
    @ObservedObject var document: ProjectDocument

    var body: some View {
        if document.project.runs.isEmpty {
            EmptyState(symbol: "clock.arrow.circlepath", title: "No runs yet", message: "Dry runs and production runs are listed here with their status, cost and folder.")
        } else {
            List(selection: Binding(get: { document.selectedRunID }, set: { id in
                document.selectedRunID = id
                if id != nil { document.inspectorTab = .run }
            })) {
                ForEach(document.project.runs.reversed()) { r in
                    HStack(spacing: 8) {
                        Image(systemName: RunStatus.symbol(r.status)).foregroundStyle(StatusColor.run(r.status)).frame(width: 16)
                        VStack(alignment: .leading, spacing: 1) {
                            HStack(spacing: 4) {
                                Text(r.run_config_name ?? r.run_config).lineLimit(1)
                                if r.kind == "dry_run" { Text("Dry run").font(.caption2).padding(.horizontal, 4).background(Color(nsColor: .quaternaryLabelColor), in: Capsule()) }
                                if r.previous_run != nil { Image(systemName: "arrow.uturn.backward").font(.caption2).foregroundStyle(.secondary).help("A revision of an earlier run") }
                            }
                            HStack(spacing: 4) {
                                Text(Fmt.when(r.started))
                                if let c = r.cost_usd { Text("· " + Fmt.money(c)) }
                            }
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                        Spacer(minLength: 0)
                        if !FileManager.default.fileExists(atPath: r.folder) {
                            Image(systemName: "questionmark.folder").foregroundStyle(.orange).help("The run folder has moved. Right-click to locate it.")
                        }
                    }
                    .tag(r.id as String?)
                    .contextMenu {
                        Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: r.folder)]) }
                            .disabled(!FileManager.default.fileExists(atPath: r.folder))
                        Button("Locate…") {
                            let panel = NSOpenPanel()
                            panel.canChooseDirectories = true
                            panel.canChooseFiles = false
                            panel.message = "Choose the run folder (it contains a logs folder)."
                            if panel.runModal() == .OK, let url = panel.url { document.relocateRun(r.id, to: url) }
                        }
                        Button("Copy Folder Path") {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(r.folder, forType: .string)
                        }
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel("\(r.kind == "dry_run" ? "Dry run" : "Run") \(r.run_config_name ?? ""), \(RunStatus.label(r.status)), \(Fmt.when(r.started))")
                }
            }
            .listStyle(.sidebar)
        }
    }
}
