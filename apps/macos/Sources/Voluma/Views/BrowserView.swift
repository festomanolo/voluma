import SwiftUI
import AppKit
import UniformTypeIdentifiers

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
    // Write access is OFF by default and per-window; every mutation is validated again in Rust.
    @State private var writeUnlocked = false
    @State private var confirmUnlock = false
    @State private var confirmDelete: Listing.Entry?
    @State private var newFolderName = ""
    @State private var showNewFolder = false
    @State private var renaming: Listing.Entry?
    @State private var renameText = ""

    private var crumbs: [String] { path.split(separator: "/").map(String.init) }
    private var sorted: [Listing.Entry] {
        (listing?.entries ?? []).sorted(using: sortOrder).sorted { $0.isDir && !$1.isDir }
    }
    private var driverWritable: Bool { listing?.driver_supports_write ?? false }

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
                if let l = listing { Text(l.filesystem).font(.caption).foregroundStyle(.secondary) }
                if writeUnlocked {
                    Button("New Folder", systemImage: "folder.badge.plus") { newFolderName = ""; showNewFolder = true }
                    Button("Import…", systemImage: "square.and.arrow.down") { importFiles() }
                }
                Button {
                    if writeUnlocked { writeUnlocked = false } else { confirmUnlock = true }
                } label: {
                    Label(writeUnlocked ? "Writable" : "Read-only", systemImage: writeUnlocked ? "lock.open.fill" : "lock.fill")
                        .foregroundStyle(writeUnlocked ? .orange : .secondary)
                }
                .disabled(!driverWritable && !writeUnlocked)
                .help(driverWritable ? "Allow changes to this image" : "This filesystem driver is read-only")
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
                    if let e = entry(ids) {
                        if !e.isDir { Button("Export…") { export(e) } }
                        if writeUnlocked {
                            Button("Rename…") { renameText = e.name; renaming = e }
                            Button("Delete…", role: .destructive) { confirmDelete = e }
                        }
                    }
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
        .confirmationDialog("Allow changes to this image?", isPresented: $confirmUnlock, titleVisibility: .visible) {
            Button("Allow Changes", role: .destructive) { writeUnlocked = true }
            Button("Keep Read-only", role: .cancel) {}
        } message: {
            Text("VOLUMA will modify the image file directly. Back it up first if it matters. Volumes that weren't cleanly unmounted are refused.")
        }
        .confirmationDialog("Delete “\(confirmDelete?.name ?? "")”?", isPresented: Binding(get: { confirmDelete != nil }, set: { if !$0 { confirmDelete = nil } }), titleVisibility: .visible) {
            Button("Delete", role: .destructive) { if let e = confirmDelete { mutate("Deleted \(e.name)", "delete", join(path, e.name)) } ; confirmDelete = nil }
            Button("Cancel", role: .cancel) { confirmDelete = nil }
        } message: { Text("This permanently removes it from the image. There is no Trash.") }
        .alert("New Folder", isPresented: $showNewFolder) {
            TextField("Name", text: $newFolderName)
            Button("Create") { mutate("Created \(newFolderName)", "mkdir", join(path, newFolderName)) }
            Button("Cancel", role: .cancel) {}
        }
        .alert("Rename", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Name", text: $renameText)
            Button("Rename") { if let e = renaming { mutate("Renamed to \(renameText)", "rename", join(path, e.name), join(path, renameText)) }; renaming = nil }
            Button("Cancel", role: .cancel) { renaming = nil }
        }
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

    func mutate(_ ok: String, _ op: String, _ a: String, _ b: String = "", overwrite: Bool = false, then: (() -> Void)? = nil) {
        let (img, part) = (target.image, target.part)
        Task.detached {
            let r = Result { try Engine.mutate(image: img, part: part, op: op, a: a, b: b, overwrite: overwrite) }
            await MainActor.run {
                switch r {
                case .success: status = ok; load()
                case .failure(let e): status = "Failed: \(e.localizedDescription)"
                }
            }
        }
    }

    func importFiles() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        panel.message = "Choose files to copy into \(path)"
        guard panel.runModal() == .OK else { return }
        for url in panel.urls {
            // Never silently replace: if the name exists, the engine refuses and we say so.
            mutate("Imported \(url.lastPathComponent)", "import", join(path, url.lastPathComponent), url.path)
        }
    }

    func export(_ e: Listing.Entry) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = e.name
        panel.message = "Export copies the file out of the image."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        try? FileManager.default.removeItem(at: url)   // user confirmed replacement in the panel
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
