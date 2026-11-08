import SwiftUI
import AppKit

struct BrowseTarget: Equatable { let image: String; let part: Int; let title: String }

struct BrowserView: View {
    let target: BrowseTarget
    let close: () -> Void

    @State private var path = "/"
    @State private var listing: Listing?
    @State private var error: String?
    @State private var selection: Set<Listing.Entry.ID> = []
    @State private var sortOrder = [KeyPathComparator(\Listing.Entry.name)]
    @State private var status: String?

    private var crumbs: [String] { path.split(separator: "/").map(String.init) }
    private var sorted: [Listing.Entry] {
        (listing?.entries ?? []).sorted(using: sortOrder).sorted { $0.isDir && !$1.isDir }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Button { close() } label: { Image(systemName: "chevron.left") }.help("Back to image")
                Button(target.title) { go("/") }.buttonStyle(.link)
                ForEach(Array(crumbs.enumerated()), id: \.offset) { i, c in
                    Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary)
                    Button(c) { go("/" + crumbs[0...i].joined(separator: "/")) }.buttonStyle(.link)
                }
                Spacer()
                if let l = listing { Text("\(l.filesystem) · read-only").font(.caption).foregroundStyle(.secondary) }
            }.padding(.horizontal, 14).padding(.vertical, 8)
            Divider()
            if let error {
                ContentUnavailableView("Can't read folder", systemImage: "exclamationmark.triangle", description: Text(error))
            } else if listing != nil && sorted.isEmpty {
                ContentUnavailableView("Empty folder", systemImage: "folder")
            } else {
                Table(sorted, selection: $selection, sortOrder: $sortOrder) {
                    TableColumn("Name", value: \.name) { e in
                        Label(e.name, systemImage: e.isDir ? "folder.fill" : "doc")
                            .foregroundStyle(e.hidden ? .secondary : .primary)
                    }
                    TableColumn("Size", value: \.size) { e in
                        Text(e.isDir ? "—" : formatBytes(Int64(e.size))).foregroundStyle(.secondary)
                    }.width(90)
                    TableColumn("Modified") { e in
                        Text(e.modified.map { Date(timeIntervalSince1970: TimeInterval($0)).formatted(date: .abbreviated, time: .shortened) } ?? "—")
                            .foregroundStyle(.secondary)
                    }.width(150)
                }
                .contextMenu(forSelectionType: Listing.Entry.ID.self) { ids in
                    if let e = entry(ids), !e.isDir { Button("Export…") { export(e) } }
                } primaryAction: { ids in
                    guard let e = entry(ids) else { return }
                    if e.isDir { go(join(path, e.name)) } else { export(e) }
                }
            }
            Divider()
            HStack {
                Text(status ?? "\(sorted.count) items").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                Spacer()
            }.padding(.horizontal, 14).padding(.vertical, 5)
        }
        .onAppear { load() }
    }

    func entry(_ ids: Set<Listing.Entry.ID>) -> Listing.Entry? {
        ids.first.flatMap { id in sorted.first { $0.id == id } }
    }
    func join(_ a: String, _ b: String) -> String { a == "/" ? "/\(b)" : "\(a)/\(b)" }
    func go(_ p: String) { path = p; selection = []; load() }

    func load() {
        let (img, part, p) = (target.image, target.part, path)
        Task.detached {
            let r = Result { try Engine.listDirectory(image: img, part: part, path: p) }
            await MainActor.run {
                switch r {
                case .success(let l): listing = l; error = nil
                case .failure(let e): listing = nil; error = e.localizedDescription
                }
            }
        }
    }

    func export(_ e: Listing.Entry) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = e.name
        panel.message = "Export copies the file out of the image. Existing files are never overwritten."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        // NSSavePanel already confirmed overwrite with the user; engine refuses to clobber,
        // so remove the confirmed target first.
        try? FileManager.default.removeItem(at: url)
        let (img, part, src) = (target.image, target.part, join(path, e.name))
        status = "Exporting \(e.name)…"
        Task.detached {
            let r = Result { try Engine.exportFile(image: img, part: part, path: src, dest: url.path) }
            await MainActor.run {
                switch r {
                case .success(let x): status = "Exported \(e.name) · \(formatBytes(Int64(x.bytes))) · SHA-256 \(x.sha256.prefix(12))…"
                case .failure(let err): status = "Export failed: \(err.localizedDescription)"
                }
            }
        }
    }
}
