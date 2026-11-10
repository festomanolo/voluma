import SwiftUI
import AppKit
import UniformTypeIdentifiers

struct BrowseTarget: Hashable {
    let image: String; let part: Int; let title: String
    /// Set for a mounted volume browsed natively (no disk image involved).
    var nativeRoot: String? = nil
    var bsd: String? = nil
    var fsKind: String? = nil
    /// macOS mounted this volume read-only (e.g. NTFS), so native writes would fail.
    var readOnlyMount = false
    /// Writing goes through the privileged NTFS helper (volume unmounted from macOS).
    var viaHelper = false
    /// The partition was never mounted by macOS (e.g. ext4): browsed raw through the helper, nothing to remount.
    var rawOnly = false
    /// Folder to open first (Favorites open Desktop, Downloads … directly).
    var startPath = "/"
}

enum ViewMode: String { case list, icons }

/// Finder-style browser for a disk image/partition (Rust engine) or a mounted volume (native).
/// Copy, cut and paste work between any two, within a volume, across volumes, and with Finder.
/// Between mounted volumes they run on `FastTransfer`: parallel, cloning and renaming wherever the disk allows.
struct BrowserView: View {
    let target: BrowseTarget
    @EnvironmentObject var nav: NavState

    @State private var path: String
    @State private var backStack: [String] = []
    @State private var forwardStack: [String] = []
    @State private var listing: Listing?
    @State private var rows: [Listing.Entry] = []
    @State private var error: String?
    @State private var selection: Set<Listing.Entry.ID> = []
    @State private var sortOrder = [KeyPathComparator(\Listing.Entry.name)]
    @State private var status: String?
    @State private var busy = false
    @State private var transfer: TransferProgress?
    @State private var cutTick = 0
    @AppStorage("voluma.viewMode") private var viewMode = ViewMode.list.rawValue
    @AppStorage("voluma.showHidden") private var showHidden = false
    @State private var filter = ""
    @FocusState private var filterFocused: Bool
    @State private var dropTargeted = false
    // Writing into an IMAGE is OFF by default and per-window; every mutation is validated again in Rust.
    // Mounted volumes behave like Finder: copying in is allowed, deleting goes to the Trash.
    @State private var writeUnlocked = false
    @State private var confirmUnlock = false
    @State private var confirmDelete: [Listing.Entry] = []
    @State private var newFolderName = ""
    @State private var showNewFolder = false
    @State private var renaming: Listing.Entry?
    @State private var renameText = ""

    @State private var confirmNtfs = false
    @State private var enablingNtfs = false

    private var backend: FSBackend {
        if target.viaHelper, let b = target.bsd, let s = NtfsSessions.shared.session(for: b) { return HelperBackend(session: s) }
        if let r = target.nativeRoot { return NativeBackend(root: r) }
        return EngineBackend(image: target.image, part: target.part)
    }
    private var isNative: Bool { target.nativeRoot != nil && !target.viaHelper }
    private var isHelper: Bool { target.viaHelper }
    /// Mounted volumes behave like Finder unless macOS mounted them read-only; helper sessions are explicitly writable.
    /// Helper sessions write only if the filesystem driver can and the volume state allows it (clean, not hibernated).
    private var helperCanWrite: Bool { (listing?.driver_supports_write ?? false) && listing?.write_blocked == nil }
    private var canWrite: Bool { (isNative && !target.readOnlyMount) || (isHelper && helperCanWrite) || writeUnlocked }
    private var needsNtfsUnlock: Bool { isNative && target.readOnlyMount && target.fsKind == "ntfs" && target.bsd != nil }
    private var crumbs: [String] { path.split(separator: "/").map(String.init) }
    private var sorted: [Listing.Entry] {
        (listing?.entries ?? []).sorted(using: sortOrder).sorted { $0.isDir && !$1.isDir }
    }
    private var driverWritable: Bool { listing?.driver_supports_write ?? false }
    private var selected: [Listing.Entry] { sorted.filter { selection.contains($0.id) } }

    var body: some View {
        VStack(spacing: 0) {
            toolbarRow
            Divider()
            if needsNtfsUnlock { ntfsBanner }
            if isHelper { helperBanner }
            if let error {
                ContentUnavailableView("Can't read folder", systemImage: "exclamationmark.triangle", description: Text(error))
            } else if listing != nil && sorted.isEmpty {
                ContentUnavailableView("Empty folder", systemImage: "folder", description: Text(canWrite ? "Paste (⌘V) files here." : ""))
            } else {
                table
            }
            Divider()
            HStack(spacing: 8) {
                if busy { ProgressView().controlSize(.small) }
                Text(status ?? "\(sorted.count) items").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                Spacer()
            }.padding(.horizontal, 14).padding(.vertical, 5)
        }
        .onAppear { load(); publishNav() }
        .onDisappear { if isHelper, let b = target.bsd { Task { _ = await NtfsSessions.shared.end(bsd: b) } } }
        .confirmationDialog("Enable writing to “\(target.title)”?", isPresented: $confirmNtfs, titleVisibility: .visible) {
            Button("Unmount and Enable Writing") { enableNtfsWriting() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("macOS can only read NTFS. VOLUMA will unmount the volume from macOS, ask for your administrator password, and write with its own NTFS driver. It refuses volumes that Windows left dirty or hibernated. The volume is remounted in macOS when you leave this window. Back up anything important first.")
        }
        .onChange(of: backStack) { _ in publishNav() }
        .confirmationDialog("Allow changes to this image?", isPresented: $confirmUnlock, titleVisibility: .visible) {
            Button("Allow Changes", role: .destructive) { writeUnlocked = true }
            Button("Keep Read-only", role: .cancel) {}
        } message: {
            Text("VOLUMA will modify the image file directly. Back it up first if it matters. Volumes that weren't cleanly unmounted are refused.")
        }
        .confirmationDialog(deleteTitle, isPresented: Binding(get: { !confirmDelete.isEmpty }, set: { if !$0 { confirmDelete = [] } }), titleVisibility: .visible) {
            Button(isNative ? "Move to Trash" : "Delete", role: .destructive) { deleteSelected(confirmDelete); confirmDelete = [] }
            Button("Cancel", role: .cancel) { confirmDelete = [] }
        } message: { Text(isNative ? "Items go to the Trash and can be restored." : "This permanently removes it from the image. There is no Trash.") }
        .alert("New Folder", isPresented: $showNewFolder) {
            TextField("Name", text: $newFolderName)
            Button("Create") { run("Created \(newFolderName)") { try backend.mkdir(joinPath(path, newFolderName)) } }
            Button("Cancel", role: .cancel) {}
        }
        .alert("Rename", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Name", text: $renameText)
            Button("Rename") { if let e = renaming { let n = renameText; run("Renamed to \(n)") { try backend.rename(joinPath(path, e.name), joinPath(path, n)) } }; renaming = nil }
            Button("Cancel", role: .cancel) { renaming = nil }
        }
    }

    private var ntfsBanner: some View {
        HStack(spacing: 10) {
            Image(systemName: "lock.fill").foregroundStyle(.orange)
            Text("macOS mounted this NTFS volume read-only.").font(.callout)
            Spacer()
            if enablingNtfs { ProgressView().controlSize(.small) }
            Button("Enable Writing…") { confirmNtfs = true }.disabled(enablingNtfs)
        }.padding(.horizontal, 14).padding(.vertical, 8).background(.orange.opacity(0.12))
    }

    private var helperBanner: some View {
        HStack(spacing: 10) {
            Image(systemName: "pencil.circle.fill").foregroundStyle(listing == nil ? Color.secondary : (helperCanWrite ? .green : .secondary))
            Text(listing == nil ? "Connecting through VOLUMA’s driver…"
                 : helperCanWrite ? "Writing through VOLUMA’s driver. The volume is unmounted from macOS until you finish."
                                : (listing?.write_blocked.map { "Read-only: \($0)" } ?? "Reading through VOLUMA’s driver (read-only for this filesystem)."))
                .font(.callout)
            Spacer()
            Button(target.rawOnly ? "Close" : "Finish & Remount") { finishHelper() }
        }.padding(.horizontal, 14).padding(.vertical, 8).background(((listing != nil && helperCanWrite) ? Color.green : Color.gray).opacity(0.12))
    }

    func enableNtfsWriting() {
        guard let bsd = target.bsd else { return }
        enablingNtfs = true
        status = "Unmounting and starting the NTFS helper…"
        Task {
            do {
                _ = try await NtfsSessions.shared.start(bsd: bsd, title: target.title)
                var t = target; t.viaHelper = true; t.readOnlyMount = false
                enablingNtfs = false
                nav.replaceBrowse?(t)
            } catch {
                enablingNtfs = false
                status = "Couldn’t enable writing: \(error.localizedDescription)"
            }
        }
    }

    func finishHelper() {
        guard let bsd = target.bsd else { return }
        status = "Remounting in macOS…"
        Task {
            let err = await NtfsSessions.shared.end(bsd: bsd)
            if let err { status = "Remount failed: \(err)" } else { nav.closeBrowser?() }
        }
    }

    private var deleteTitle: String {
        confirmDelete.count == 1 ? "\(isNative ? "Move" : "Delete") “\(confirmDelete[0].name)”?" : "\(isNative ? "Move" : "Delete") \(confirmDelete.count) items?"
    }

    // MARK: toolbar
    private var toolbarRow: some View {
        HStack(spacing: 6) {
            Button { goBack() } label: { Image(systemName: "chevron.left") }.disabled(backStack.isEmpty).help("Previous folder")
            Button { goForward() } label: { Image(systemName: "chevron.right") }.disabled(forwardStack.isEmpty).help("Next folder")
            Button(target.title) { go("/") }.buttonStyle(.link)
            ForEach(Array(crumbs.enumerated()), id: \.offset) { i, c in
                Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary)
                Button(c) { go("/" + crumbs[0...i].joined(separator: "/")) }.buttonStyle(.link)
            }
            Spacer()
            if let l = listing { Text(l.filesystem).font(.caption).foregroundStyle(.secondary) }
            Button("Copy", systemImage: "doc.on.doc") { copySelection() }.disabled(selection.isEmpty).help("Copy selected items (⌘C)")
            Button("Paste", systemImage: "doc.on.clipboard") { paste() }.disabled(!canWrite || busy).help(canWrite ? "Paste into this folder (⌘V)" : "Unlock writing to paste into this image")
            if canWrite {
                Button("New Folder", systemImage: "folder.badge.plus") { newFolderName = ""; showNewFolder = true }
                Button("Import…", systemImage: "square.and.arrow.down") { importFiles() }
            }
            if !isNative && !isHelper {
                Button {
                    if writeUnlocked { writeUnlocked = false } else { confirmUnlock = true }
                } label: {
                    Label(writeUnlocked ? "Writable" : "Read-only", systemImage: writeUnlocked ? "lock.open.fill" : "lock.fill")
                        .foregroundStyle(writeUnlocked ? .orange : .secondary)
                }
                .disabled((!driverWritable || listing?.write_blocked != nil) && !writeUnlocked)
                .help(listing?.write_blocked.map { "Can't write safely: \($0)" } ?? (driverWritable ? "Allow changes to this image" : "This filesystem driver is read-only"))
            }
        }.padding(.horizontal, 14).padding(.vertical, 8)
    }

    private var table: some View {
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
            let es = sorted.filter { ids.contains($0.id) }
            if !es.isEmpty {
                Button("Copy") { selection = ids; copySelection() }
                if canWrite { Button("Paste") { paste() } }
                if es.count == 1, let e = es.first {
                    if isNative { Button("Reveal in Finder") { if let u = backend.hostURL(joinPath(path, e.name)) { NSWorkspace.shared.activateFileViewerSelecting([u]) } } }
                    else if !e.isDir { Button("Export…") { export(e) } }
                    if canWrite { Button("Rename…") { renameText = e.name; renaming = e } }
                }
                if canWrite { Button(isNative ? "Move to Trash…" : "Delete…", role: .destructive) { confirmDelete = es } }
            } else if canWrite { Button("Paste") { paste() } }
        } primaryAction: { ids in
            guard let e = sorted.first(where: { ids.contains($0.id) }) else { return }
            if e.isDir { go(joinPath(path, e.name)) }
            else if isNative, let u = backend.hostURL(joinPath(path, e.name)) { NSWorkspace.shared.open(u) }
            else { export(e) }
        }
        // ⌘C / ⌘V reach these through the responder chain, so text fields in the alerts keep working.
        .onCopyCommand { copySelection(); return [] }
        .onPasteCommand(of: [.fileURL, UTType(Clip.type.rawValue) ?? .data]) { _ in paste() }
    }

    // MARK: navigation
    private func publishNav() {
        nav.innerCanBack = !backStack.isEmpty
        nav.innerBack = { goBack() }
    }
    func go(_ p: String) {
        guard p != path else { return }
        backStack.append(path); forwardStack = []
        path = p; selection = []; load()
    }
    func goBack() { guard let p = backStack.popLast() else { return }; forwardStack.append(path); path = p; selection = []; load() }
    func goForward() { guard let p = forwardStack.popLast() else { return }; backStack.append(path); path = p; selection = []; load() }

    func load() {
        let (b, p) = (backend, path)
        Task.detached {
            let r = Result { try b.list(p) }
            await MainActor.run {
                switch r {
                case .success(let l): listing = l; error = nil
                case .failure(let e): listing = nil; error = e.localizedDescription
                }
            }
        }
    }

    /// Runs a blocking mutation off the main thread, then refreshes.
    func run(_ ok: String, _ work: @escaping () throws -> Void) {
        busy = true
        Task.detached {
            let r = Result { try work() }
            await MainActor.run {
                busy = false
                switch r {
                case .success: status = ok
                case .failure(let e): status = "Failed: \(e.localizedDescription)"
                }
                load()
            }
        }
    }

    // MARK: copy / paste
    func copySelection() {
        let es = selected
        guard !es.isEmpty else { return }
        Clip.shared.copy(backend: backend, items: es.map { (joinPath(path, $0.name), $0) })
        status = "Copied \(es.count) item\(es.count == 1 ? "" : "s")"
    }

    func paste() {
        guard canWrite else { status = "Unlock writing first to paste into an image"; return }
        let (dst, dir) = (backend, path)
        let src = Clip.shared.source()
        if case .none = src { status = "Nothing to paste"; return }
        busy = true
        status = "Pasting…"
        Task.detached {
            var count = 0
            var failure: String?
            do {
                var taken = Copier.takenNames(dst, dir)
                switch src {
                case .clip(let sb, let items):
                    for it in items {
                        count += try Copier.copy(src: sb, srcPath: it.path, entry: it.entry, dst: dst, dstDir: dir, taken: &taken) { n in
                            Task { @MainActor in status = "Pasting \(n)…" }
                        }
                    }
                case .files(let urls):
                    for u in urls {
                        count += try Copier.copyHost(u, dst: dst, dstDir: dir, taken: &taken) { n in
                            Task { @MainActor in status = "Pasting \(n)…" }
                        }
                    }
                case .none: break
                }
            } catch { failure = error.localizedDescription }
            let (done, fail) = (count, failure)
            await MainActor.run {
                busy = false
                status = fail.map { "Paste stopped after \(done) item\(done == 1 ? "" : "s"): \($0)" } ?? "Pasted \(done) item\(done == 1 ? "" : "s")"
                load()
            }
        }
    }

    // MARK: other actions
    func deleteSelected(_ es: [Listing.Entry]) {
        let (b, dir) = (backend, path)
        run("Deleted \(es.count) item\(es.count == 1 ? "" : "s")") { for e in es { try b.delete(joinPath(dir, e.name)) } }
    }

    func importFiles() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = true
        panel.message = "Choose files or folders to copy into \(path)"
        guard panel.runModal() == .OK else { return }
        let (dst, dir, urls) = (backend, path, panel.urls)
        run("Imported \(urls.count) item\(urls.count == 1 ? "" : "s")") {
            var taken = Copier.takenNames(dst, dir)
            for u in urls { try Copier.copyHost(u, dst: dst, dstDir: dir, taken: &taken) { _ in } }
        }
    }

    func export(_ e: Listing.Entry) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = e.name
        panel.message = "Export copies the file out of the image."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        try? FileManager.default.removeItem(at: url)   // user confirmed replacement in the panel
        let (b, src) = (backend, joinPath(path, e.name))
        status = "Exporting \(e.name)…"
        busy = true
        Task.detached {
            let r = Result { try b.exportFile(src, to: url) }
            await MainActor.run {
                busy = false
                switch r {
                case .success: status = "Exported \(e.name) · \(formatBytes(Int64(e.size)))"
                case .failure(let err): status = "Export failed: \(err.localizedDescription)"
                }
            }
        }
    }
}
