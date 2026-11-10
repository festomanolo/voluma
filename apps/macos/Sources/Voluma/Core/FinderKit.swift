import SwiftUI
import AppKit
import QuickLookUI
import QuickLookThumbnailing
import UniformTypeIdentifiers

// MARK: - kinds

/// The "Kind" column: "PDF document", "Folder", "JPEG image" … from the file extension, like Finder.
enum FileKinds {
    private static let cache = NSCache<NSString, NSString>()
    static func describe(_ e: Listing.Entry) -> String {
        if e.isDir { return (e.name as NSString).pathExtension.lowercased() == "app" ? "Application" : "Folder" }
        let ext = (e.name as NSString).pathExtension.lowercased()
        if ext.isEmpty { return "Document" }
        if let c = cache.object(forKey: ext as NSString) { return c as String }
        let text = UTType(filenameExtension: ext)?.localizedDescription?.capitalizedFirst ?? "\(ext.uppercased()) file"
        cache.setObject(text as NSString, forKey: ext as NSString)
        return text
    }
}

private extension String {
    var capitalizedFirst: String { prefix(1).uppercased() + dropFirst() }
}

// MARK: - icons

/// Real Finder icons. Mounted volumes ask the system (so apps, custom folder icons and image previews look like
/// Finder); images and other volumes get the type icon for the extension.
enum FileIcons {
    private static let cache = NSCache<NSString, NSImage>()

    static func icon(for e: Listing.Entry, url: URL?) -> NSImage {
        if let url {
            let key = "\(url.path)|\(e.modified ?? 0)" as NSString
            if let c = cache.object(forKey: key) { return c }
            let img = NSWorkspace.shared.icon(forFile: url.path)
            cache.setObject(img, forKey: key)
            return img
        }
        let ext = (e.name as NSString).pathExtension.lowercased()
        let key = (e.isDir ? "dir" : "ext:\(ext)") as NSString
        if let c = cache.object(forKey: key) { return c }
        let type: UTType = e.isDir ? .folder : (UTType(filenameExtension: ext) ?? .data)
        let img = NSWorkspace.shared.icon(for: type)
        cache.setObject(img, forKey: key)
        return img
    }
}

// MARK: - Finder tags (the colours)

/// The seven Finder tag colours. Setting one writes a real Finder tag, so it shows up in Finder too.
enum FinderTag: String, CaseIterable, Identifiable {
    case red = "Red", orange = "Orange", yellow = "Yellow", green = "Green", blue = "Blue", purple = "Purple", gray = "Gray"
    var id: String { rawValue }
    var color: Color {
        switch self {
        case .red: Color(red: 1.0, green: 0.31, blue: 0.29)
        case .orange: Color(red: 1.0, green: 0.62, blue: 0.20)
        case .yellow: Color(red: 0.98, green: 0.80, blue: 0.15)
        case .green: Color(red: 0.30, green: 0.80, blue: 0.35)
        case .blue: Color(red: 0.22, green: 0.55, blue: 1.0)
        case .purple: Color(red: 0.69, green: 0.42, blue: 0.90)
        case .gray: Color(red: 0.62, green: 0.62, blue: 0.65)
        }
    }
    /// Finder's `labelNumber`: 1 gray, 2 green, 3 purple, 4 blue, 5 yellow, 6 red, 7 orange.
    static func fromLabel(_ n: Int) -> FinderTag? {
        [1: .gray, 2: .green, 3: .purple, 4: .blue, 5: .yellow, 6: .red, 7: .orange][n]
    }
    /// The dots to draw for an entry: one per standard tag, plus the label colour for custom coloured tags.
    static func dots(for e: Listing.Entry) -> [Color] {
        guard let names = e.tags, !names.isEmpty else { return [] }
        var out: [Color] = []
        for n in names {
            let base = n.components(separatedBy: "\n").first ?? n
            if let t = FinderTag(rawValue: base) { out.append(t.color) }
            else if let l = e.label, let t = fromLabel(l) { out.append(t.color) }
            else { out.append(FinderTag.gray.color) }
        }
        return out
    }
    /// Applies or removes the tag on host files. If every file already has it the tag is removed (a toggle, like Finder).
    @discardableResult
    static func toggle(_ tag: FinderTag, on urls: [URL]) -> Bool {
        let all = urls.allSatisfy { ((try? $0.resourceValues(forKeys: [.tagNamesKey]).tagNames) ?? []).contains(tag.rawValue) }
        var ok = true
        for u in urls {
            var names = (try? u.resourceValues(forKeys: [.tagNamesKey]).tagNames) ?? []
            if all { names.removeAll { $0 == tag.rawValue } } else if !names.contains(tag.rawValue) { names.append(tag.rawValue) }
            do { try (u as NSURL).setResourceValue(names, forKey: .tagNamesKey) } catch { ok = false }
        }
        return ok
    }
    static func clear(on urls: [URL]) { for u in urls { try? (u as NSURL).setResourceValue([String](), forKey: .tagNamesKey) } }
}

struct TagDots: View {
    let colors: [Color]
    var body: some View {
        if !colors.isEmpty {
            HStack(spacing: -3) {
                ForEach(Array(colors.prefix(4).enumerated()), id: \.offset) { _, c in
                    Circle().fill(c).frame(width: 9, height: 9).overlay(Circle().stroke(.background, lineWidth: 1))
                }
            }
        }
    }
}

// MARK: - new-file templates

struct FileTemplate: Identifiable, Hashable {
    let id: String; let title: String; let symbol: String; let ext: String; let body: String; var executable = false

    static let all: [FileTemplate] = [
        .init(id: "empty", title: "Empty File", symbol: "doc", ext: "", body: ""),
        .init(id: "txt", title: "Plain Text", symbol: "doc.text", ext: "txt", body: ""),
        .init(id: "md", title: "Markdown", symbol: "text.alignleft", ext: "md", body: "# Untitled\n\n"),
        .init(id: "json", title: "JSON", symbol: "curlybraces", ext: "json", body: "{\n}\n"),
        .init(id: "html", title: "HTML Page", symbol: "chevron.left.forwardslash.chevron.right", ext: "html",
              body: "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  <title>Untitled</title>\n</head>\n<body>\n\n</body>\n</html>\n"),
        .init(id: "sh", title: "Shell Script", symbol: "terminal", ext: "sh", body: "#!/bin/sh\nset -e\n\n", executable: true),
        .init(id: "py", title: "Python Script", symbol: "chevron.left.forwardslash.chevron.right", ext: "py", body: "#!/usr/bin/env python3\n\n", executable: true),
    ]

    var defaultName: String { ext.isEmpty ? "untitled file" : "untitled.\(ext)" }
    var data: Data { Data(body.utf8) }
}

// MARK: - Quick Look

/// Space bar preview, the same panel Finder uses.
@MainActor
final class QuickLook: NSObject, QLPreviewPanelDataSource, QLPreviewPanelDelegate {
    static let shared = QuickLook()
    private var items: [URL] = []

    func toggle(_ urls: [URL]) {
        guard let panel = QLPreviewPanel.shared() else { return }
        if panel.isVisible { panel.orderOut(nil); return }
        guard !urls.isEmpty else { return }
        items = urls
        panel.dataSource = self; panel.delegate = self
        panel.reloadData()
        panel.makeKeyAndOrderFront(nil)
    }

    nonisolated func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { MainActor.assumeIsolated { items.count } }
    nonisolated func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> QLPreviewItem! {
        MainActor.assumeIsolated { items[index] as NSURL }
    }
}

// MARK: - Get Info

struct ItemInfo {
    var rows: [(String, String)] = []
    var folderBytes: Int64?
}

enum InfoLoader {
    /// Details of a mounted-volume item. Folder size is computed by walking the folder (can take a while).
    static func load(_ url: URL, entry: Listing.Entry) -> ItemInfo {
        var info = ItemInfo()
        let keys: Set<URLResourceKey> = [.creationDateKey, .contentModificationDateKey, .contentAccessDateKey, .fileSizeKey,
                                         .totalFileAllocatedSizeKey, .volumeNameKey, .isAliasFileKey, .isSymbolicLinkKey]
        let v = try? url.resourceValues(forKeys: keys)
        let attrs = (try? FileManager.default.attributesOfItem(atPath: url.path)) ?? [:]
        func d(_ x: Date?) -> String { x.map { $0.formatted(date: .long, time: .standard) } ?? "—" }
        info.rows = [
            ("Kind", entry.kindText),
            ("Where", url.deletingLastPathComponent().path),
            ("Volume", v?.volumeName ?? "—"),
            ("Created", d(v?.creationDate)),
            ("Modified", d(v?.contentModificationDate)),
            ("Last opened", d(v?.contentAccessDate)),
        ]
        if !entry.isDir {
            let size = Int64(v?.fileSize ?? 0), disk = Int64(v?.totalFileAllocatedSize ?? 0)
            info.rows.insert(("Size", "\(formatBytes(size)) (\(size.formatted()) bytes) · \(formatBytes(disk)) on disk"), at: 2)
        }
        if let perm = attrs[.posixPermissions] as? NSNumber {
            let owner = (attrs[.ownerAccountName] as? String) ?? "?", group = (attrs[.groupOwnerAccountName] as? String) ?? "?"
            info.rows.append(("Permissions", "\(String(perm.intValue, radix: 8)) · \(owner):\(group)"))
        }
        if v?.isSymbolicLink == true, let dest = try? FileManager.default.destinationOfSymbolicLink(atPath: url.path) { info.rows.append(("Links to", dest)) }
        if let tags = entry.tags, !tags.isEmpty { info.rows.append(("Tags", tags.joined(separator: ", "))) }
        return info
    }

    static func folderSize(_ url: URL) -> Int64 {
        var total: Int64 = 0
        guard let en = FileManager.default.enumerator(at: url, includingPropertiesForKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey],
                                                      options: [], errorHandler: { _, _ in true }) else { return 0 }
        for case let u as URL in en {
            if let v = try? u.resourceValues(forKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey]), v.isRegularFile == true {
                total += Int64(v.totalFileAllocatedSize ?? 0)
            }
        }
        return total
    }
}

/// The details grid for one item (shared by Get Info and the inspector panel).
struct ItemDetails: View {
    let entry: Listing.Entry; let url: URL?
    @State private var info = ItemInfo()
    @State private var folderBytes: Int64?
    @State private var computing = false

    var body: some View {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 7) {
            if url == nil {
                row("Kind", entry.kindText)
                if !entry.isDir { row("Size", "\(formatBytes(Int64(entry.size))) (\(entry.size.formatted()) bytes)") }
                row("Modified", entry.modified.map { Date(timeIntervalSince1970: TimeInterval($0)).formatted(date: .long, time: .standard) } ?? "—")
            } else {
                ForEach(Array(info.rows.enumerated()), id: \.offset) { _, r in row(r.0, r.1) }
                if entry.isDir { row("Size", folderBytes.map { "\(formatBytes($0)) on disk" } ?? (computing ? "Calculating…" : "—")) }
            }
        }
        .task(id: (url?.path ?? "") + entry.name) {
            folderBytes = nil
            guard let url else { return }
            info = InfoLoader.load(url, entry: entry)
            if entry.isDir {
                computing = true
                let b = await Task.detached(priority: .utility) { InfoLoader.folderSize(url) }.value
                if !Task.isCancelled { folderBytes = b }
                computing = false
            }
        }
    }

    private func row(_ k: String, _ v: String) -> some View {
        GridRow {
            Text(k).foregroundStyle(.secondary).gridColumnAlignment(.trailing)
            Text(v).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
        }.font(.callout)
    }
}

struct InfoSheet: View {
    let entry: Listing.Entry; let url: URL?; let size: UInt64
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 12) {
                Image(nsImage: FileIcons.icon(for: entry, url: url)).resizable().frame(width: 56, height: 56)
                VStack(alignment: .leading, spacing: 3) {
                    Text(entry.name).font(.title3.weight(.semibold)).lineLimit(2).textSelection(.enabled)
                    HStack(spacing: 6) { Text(entry.kindText).foregroundStyle(.secondary); TagDots(colors: FinderTag.dots(for: entry)) }
                }
                Spacer()
            }
            Divider()
            ItemDetails(entry: entry, url: url)
            HStack { Spacer(); Button("Done") { dismiss() }.keyboardShortcut(.defaultAction) }
        }
        .padding(20).frame(width: 440)
    }
}

/// The right-hand panel: a large preview and the details of the selection (or of the folder when nothing is selected).
struct InspectorPane: View {
    let selection: [Listing.Entry]
    let url: (Listing.Entry) -> URL?
    let folderName: String; let folderPath: String; let itemCount: Int; let filesystem: String
    @State private var thumb: NSImage?

    var body: some View {
        ScrollView {
            VStack(spacing: 14) {
                if selection.count == 1, let e = selection.first { single(e) }
                else if selection.count > 1 { multiple }
                else { folder }
            }.padding(16).frame(maxWidth: .infinity)
        }
    }

    private func infoRow(_ k: String, _ v: String) -> some View {
        GridRow {
            Text(k).foregroundStyle(.secondary).gridColumnAlignment(.trailing)
            Text(v).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
