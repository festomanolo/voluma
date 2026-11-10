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
    @State private var groups: [EntryGroup] = []
    @AppStorage("voluma.groupBy") private var groupBy = GroupKey.none.rawValue
    @AppStorage("voluma.inspector") private var showInspector = false
    @State private var error: String?
    @State private var selection: Set<Listing.Entry.ID> = []
    @State private var sortOrder = SortKey.name.comparators(ascending: true)
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
    @State private var showNewFile = false
    @State private var newFileName = "untitled.txt"
    @State private var newFileTemplate = FileTemplate.all[1]
    @State private var renaming: Listing.Entry?
    @State private var renameText = ""
    @State private var infoEntry: Listing.Entry?
    @State private var anchor: Listing.Entry.ID?

    @State private var confirmNtfs = false
    @State private var enablingNtfs = false

    init(target: BrowseTarget) {
        self.target = target
        _path = State(initialValue: target.startPath)
    }

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
    private var driverWritable: Bool { listing?.driver_supports_write ?? false }
    private var selected: [Listing.Entry] { rows.filter { selection.contains($0.id) } }
    private var group: GroupKey { GroupKey(rawValue: groupBy) ?? .none }
    private var sortKey: SortKey? { SortKey.of(sortOrder) }
    private var ascending: Bool { sortOrder.first?.order != .reverse }
    private var mode: ViewMode { ViewMode(rawValue: viewMode) ?? .list }
    private var rootTitle: String { target.nativeRoot == "/" && target.startPath != "/" ? "Macintosh HD" : target.title }

    private func url(_ e: Listing.Entry) -> URL? { backend.hostURL(joinPath(path, e.name)) }
    private func isCutRow(_ e: Listing.Entry) -> Bool { _ = cutTick; return Clip.shared.isCutItem(backendID: backend.id, path: joinPath(path, e.name)) }

    var body: some View {
        VStack(spacing: 0) {
            toolbarRow
            Divider()
            if needsNtfsUnlock { ntfsBanner }
            if isHelper { helperBanner }
            content
                .overlay { if dropTargeted { RoundedRectangle(cornerRadius: 8).strokeBorder(Color.accentColor, lineWidth: 3).padding(4).allowsHitTesting(false) } }
                .dropDestination(for: URL.self) { urls, _ in dropURLs(urls) } isTargeted: { dropTargeted = $0 }
            Divider()
            footer
        }
        .background(shortcuts)
        .onAppear { load(); publishNav() }
        .onDisappear { if isHelper, let b = target.bsd { Task { _ = await NtfsSessions.shared.end(bsd: b) } } }
        .confirmationDialog("Enable writing to “\(target.title)”?", isPresented: $confirmNtfs, titleVisibility: .visible) {
            Button("Unmount and Enable Writing") { enableNtfsWriting() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("macOS can only read NTFS. VOLUMA will unmount the volume from macOS, ask for your administrator password, and write with its own NTFS driver. It refuses volumes that Windows left dirty or hibernated. The volume is remounted in macOS when you leave this window. Back up anything important first.")
        }
        .onChange(of: backStack) { _, _ in publishNav() }
        .onChange(of: sortOrder) { _, _ in recompute() }
        .onChange(of: filter) { _, _ in recompute() }
        .onChange(of: showHidden) { _, _ in recompute() }
        .onChange(of: groupBy) { _, _ in recompute() }
        .inspector(isPresented: $showInspector) {
            InspectorPane(selection: selected, url: { url($0) }, folderName: crumbs.last ?? rootTitle, folderPath: path,
                          itemCount: rows.count, filesystem: listing?.filesystem ?? "—")
                .inspectorColumnWidth(min: 240, ideal: 290, max: 420)
        }
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
            Button("Create") { createFolder(newFolderName) }
            Button("Cancel", role: .cancel) {}
        }
        .alert("Rename", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Name", text: $renameText)
            Button("Rename") { if let e = renaming { let n = renameText; run("Renamed to \(n)", select: [n]) { try backend.rename(joinPath(path, e.name), joinPath(path, n)) } }; renaming = nil }
            Button("Cancel", role: .cancel) { renaming = nil }
        }
        .sheet(isPresented: $showNewFile) { newFileSheet }
        .sheet(item: $infoEntry) { e in InfoSheet(entry: e, url: url(e), size: e.size) }
    }

    // MARK: banners

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
            Button { goBack() } label: { Image(systemName: "chevron.left") }.disabled(backStack.isEmpty).help("Previous folder (⌘[)")
            Button { goForward() } label: { Image(systemName: "chevron.right") }.disabled(forwardStack.isEmpty).help("Next folder (⌘])")
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 4) {
                    Button(rootTitle) { go("/") }.buttonStyle(.link)
                    ForEach(Array(crumbs.enumerated()), id: \.offset) { i, c in
                        Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary)
                        Button(c) { go("/" + crumbs[0...i].joined(separator: "/")) }.buttonStyle(.link)
                    }
                }
            }
            Spacer(minLength: 8)
            if let l = listing { Text(l.filesystem).font(.caption).foregroundStyle(.secondary) }

            Picker("View", selection: $viewMode) {
                Image(systemName: "list.bullet").tag(ViewMode.list.rawValue)
                Image(systemName: "square.grid.2x2").tag(ViewMode.icons.rawValue)
            }.pickerStyle(.segmented).frame(width: 74).labelsHidden().help("List (⌘1) or Icons (⌘2)")

            Button("Copy", systemImage: "doc.on.doc") { copySelection() }.disabled(selection.isEmpty).help("Copy selected items (⌘C)")
            Button("Cut", systemImage: "scissors") { cutSelection() }.disabled(selection.isEmpty || !canMoveFromHere).help("Cut: the next paste moves the items (⌘X)")
            Button("Paste", systemImage: "doc.on.clipboard") { paste() }.disabled(!canWrite || busy).help(canWrite ? "Paste into this folder (⌘V)" : "Unlock writing to paste into this image")
            if canWrite {
                Button("New Folder", systemImage: "folder.badge.plus") { newFolderName = ""; showNewFolder = true }.help("New Folder (⇧⌘N)")
                Menu {
                    ForEach(FileTemplate.all) { t in Button(t.title, systemImage: t.symbol) { openNewFile(t) } }
                } label: { Label("New File", systemImage: "doc.badge.plus") } primaryAction: { openNewFile(FileTemplate.all[1]) }
                .help("New File (⌥⌘N)")
                Button("Import…", systemImage: "square.and.arrow.down") { importFiles() }
            }
            moreMenu
            TextField("Filter", text: $filter).textFieldStyle(.roundedBorder).frame(width: 130).focused($filterFocused)
                .onSubmit { filterFocused = false }
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

    private var moreMenu: some View {
        Menu {
            Button("Get Info", systemImage: "info.circle") { showInfo() }.disabled(selection.count != 1)
            Button("Quick Look", systemImage: "eye") { quickLook() }.disabled(selection.isEmpty)
            Divider()
            Button("Duplicate", systemImage: "plus.square.on.square") { duplicate() }.disabled(selection.isEmpty || !canWrite)
            Button("Make Alias", systemImage: "arrow.turn.up.right") { makeAlias() }.disabled(selection.isEmpty || !canWrite || !isNative)
            Button("Compress", systemImage: "archivebox") { compress() }.disabled(selection.isEmpty || !canWrite || !isNative)
            Button("Copy Path", systemImage: "link") { copyPath() }.disabled(selection.isEmpty)
            Divider()
            Toggle("Show Hidden Files", isOn: $showHidden)
            Button("Select All", systemImage: "checkmark.circle") { selection = Set(rows.map(\.id)) }
        } label: { Image(systemName: "ellipsis.circle") }
        .menuIndicator(.hidden).frame(width: 30).help("More actions")
    }

    // MARK: content

    @ViewBuilder private var content: some View {
        if let error {
            ContentUnavailableView("Can't read folder", systemImage: "exclamationmark.triangle", description: Text(error))
        } else if listing != nil && rows.isEmpty {
            ContentUnavailableView(filter.isEmpty ? "Empty folder" : "No matches", systemImage: filter.isEmpty ? "folder" : "magnifyingglass",
                                   description: Text(filter.isEmpty && canWrite ? "Paste (⌘V) or drop files here." : ""))
                .contextMenu { if canWrite { backgroundMenu } }
        } else if mode == .icons {
            grid
        } else {
            table
        }
    }

    private var table: some View {
        Table(rows, selection: $selection, sortOrder: $sortOrder) {
            TableColumn("Name", value: \.name) { e in
                HStack(spacing: 7) {
                    Image(nsImage: FileIcons.icon(for: e, url: url(e))).resizable().interpolation(.high).frame(width: 18, height: 18)
                    Text(e.name).lineLimit(1).foregroundStyle(e.hidden ? .secondary : .primary)
                    TagDots(colors: FinderTag.dots(for: e))
                }
                .opacity(isCutRow(e) ? 0.4 : 1)
                .modifier(DragOut(url: url(e)))
            }
            TableColumn("Date Modified", value: \.modifiedSort) { e in
                Text(e.modified.map { Date(timeIntervalSince1970: TimeInterval($0)).formatted(date: .abbreviated, time: .shortened) } ?? "—")
                    .foregroundStyle(.secondary)
            }.width(min: 110, ideal: 150)
            TableColumn("Size", value: \.size) { e in
                Text(e.isDir ? "—" : formatBytes(Int64(e.size))).foregroundStyle(.secondary)
            }.width(min: 60, ideal: 80)
            TableColumn("Kind", value: \.kindText) { e in
                Text(e.kindText).foregroundStyle(.secondary).lineLimit(1)
            }.width(min: 80, ideal: 130)
        }
        .contextMenu(forSelectionType: Listing.Entry.ID.self) { ids in
            let es = rows.filter { ids.contains($0.id) }
            if !es.isEmpty { itemMenu(es, ids: ids) } else if canWrite { backgroundMenu }
        } primaryAction: { ids in
            guard let e = rows.first(where: { ids.contains($0.id) }) else { return }
            open(e)
        }
        // ⌘C / ⌘X / ⌘V reach these through the responder chain, so text fields in the alerts keep working.
        .onCopyCommand { copySelection(); return [] }
        .onCutCommand { cutSelection(); return [] }
        .onPasteCommand(of: [.fileURL, UTType(Clip.type.rawValue) ?? .data]) { _ in paste() }
        .onKeyPress(.space) { quickLook(); return .handled }
    }

    private var grid: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 96, maximum: 124), spacing: 8, alignment: .top)], spacing: 10) {
                ForEach(rows) { e in
                    VStack(spacing: 4) {
                        Image(nsImage: FileIcons.icon(for: e, url: url(e))).resizable().interpolation(.high).frame(width: 56, height: 56)
                        Text(e.name).font(.caption).lineLimit(2).multilineTextAlignment(.center)
                            .padding(.horizontal, 4).padding(.vertical, 1)
                            .background(selection.contains(e.id) ? Color.accentColor : .clear, in: RoundedRectangle(cornerRadius: 4))
                            .foregroundStyle(selection.contains(e.id) ? Color.white : (e.hidden ? Color.secondary : Color.primary))
                        TagDots(colors: FinderTag.dots(for: e))
                    }
                    .padding(6).frame(maxWidth: .infinity)
                    .background(selection.contains(e.id) ? Color.accentColor.opacity(0.18) : .clear, in: RoundedRectangle(cornerRadius: 8))
                    .opacity(isCutRow(e) ? 0.4 : 1)
                    .contentShape(Rectangle())
                    .onTapGesture(count: 2) { open(e) }
                    .onTapGesture { clickSelect(e) }
                    .contextMenu {
                        if !selection.contains(e.id) { Color.clear.frame(width: 0, height: 0).onAppear { selection = [e.id] } }
                        itemMenu(selected.isEmpty ? [e] : selected, ids: Set((selected.isEmpty ? [e] : selected).map(\.id)))
                    }
                    .modifier(DragOut(url: url(e)))
                }
            }.padding(12)
        }
        .contentShape(Rectangle())
        .onTapGesture { selection = [] }
        .contextMenu { if canWrite { backgroundMenu } }
        .onCopyCommand { copySelection(); return [] }
        .onCutCommand { cutSelection(); return [] }
        .onPasteCommand(of: [.fileURL, UTType(Clip.type.rawValue) ?? .data]) { _ in paste() }
        .onKeyPress(.space) { quickLook(); return .handled }
    }

    private func clickSelect(_ e: Listing.Entry) {
        let f = NSEvent.modifierFlags
        if f.contains(.command) { if selection.contains(e.id) { selection.remove(e.id) } else { selection.insert(e.id) }; anchor = e.id }
        else if f.contains(.shift), let a = anchor, let i = rows.firstIndex(where: { $0.id == a }), let j = rows.firstIndex(where: { $0.id == e.id }) {
            selection = Set(rows[min(i, j)...max(i, j)].map(\.id))
        } else { selection = [e.id]; anchor = e.id }
    }

    // MARK: menus

    @ViewBuilder private func itemMenu(_ es: [Listing.Entry], ids: Set<Listing.Entry.ID>) -> some View {
        let one = es.count == 1 ? es.first : nil
        if let e = one { Button("Open") { open(e) } }
        if let e = one, isNative, !e.isDir, let u = url(e) {
            let apps = NSWorkspace.shared.urlsForApplications(toOpen: u).prefix(12)
            if !apps.isEmpty {
                Menu("Open With") {
                    ForEach(Array(apps), id: \.self) { app in
                        Button(FileManager.default.displayName(atPath: app.path)) {
                            NSWorkspace.shared.open([u], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
                        }
                    }
                }
            }
        }
        Button("Quick Look") { selection = ids; quickLook() }
        if one != nil { Button("Get Info") { selection = ids; showInfo() } }
        Divider()
        Button("Copy") { selection = ids; copySelection() }
        if canWrite && canMoveFromHere { Button("Cut") { selection = ids; cutSelection() } }
        if canWrite { Button("Paste") { paste() } }
        if canWrite { Button("Duplicate") { selection = ids; duplicate() } }
        Button("Copy Path") { selection = ids; copyPath() }
        if let e = one {
            if canWrite { Button("Rename…") { renameText = e.name; renaming = e } }
            if isNative { Button("Reveal in Finder") { if let u = url(e) { NSWorkspace.shared.activateFileViewerSelecting([u]) } } }
            else if !e.isDir { Button("Export…") { export(e) } }
        }
        if isNative && canWrite {
            Button("Make Alias") { selection = ids; makeAlias() }
            Button("Compress") { selection = ids; compress() }
            Divider()
            // Finder tag colours (real tags: they show up in Finder too)
            ForEach(FinderTag.allCases) { t in
                Button { selection = ids; applyTag(t) } label: { Label(t.rawValue, systemImage: "circle.fill") }.tint(t.color)
            }
            Button("Remove Tags") { selection = ids; clearTags() }
        }
        if canWrite {
            Divider()
            Button("New Folder") { newFolderName = ""; showNewFolder = true }
            Menu("New File") { ForEach(FileTemplate.all) { t in Button(t.title) { openNewFile(t) } } }
            Divider()
            Button(isNative ? "Move to Trash…" : "Delete…", role: .destructive) { confirmDelete = es }
        }
    }

    @ViewBuilder private var backgroundMenu: some View {
        Button("Paste") { paste() }
        Button("New Folder") { newFolderName = ""; showNewFolder = true }
        Menu("New File") { ForEach(FileTemplate.all) { t in Button(t.title) { openNewFile(t) } } }
        Button("Select All") { selection = Set(rows.map(\.id)) }
    }

    /// Keyboard shortcuts that are not reachable through the responder chain. Disabled while the filter field has focus.
    private var shortcuts: some View {
        Group {
            Button("") { newFolderName = ""; showNewFolder = true }.keyboardShortcut("n", modifiers: [.command, .shift]).disabled(!canWrite)
            Button("") { openNewFile(FileTemplate.all[1]) }.keyboardShortcut("n", modifiers: [.command, .option]).disabled(!canWrite)
            Button("") { duplicate() }.keyboardShortcut("d", modifiers: .command).disabled(selection.isEmpty || !canWrite)
            Button("") { showInfo() }.keyboardShortcut("i", modifiers: .command).disabled(selection.count != 1)
            Button("") { if !selected.isEmpty { confirmDelete = selected } }.keyboardShortcut(.delete, modifiers: .command).disabled(selection.isEmpty || !canWrite || filterFocused)
            Button("") { paste(forceMove: true) }.keyboardShortcut("v", modifiers: [.command, .option]).disabled(!canWrite || busy)
            Button("") { copyPath() }.keyboardShortcut("c", modifiers: [.command, .option]).disabled(selection.isEmpty)
            Button("") { showHidden.toggle() }.keyboardShortcut(".", modifiers: [.command, .shift])
            Button("") { goUp() }.keyboardShortcut(.upArrow, modifiers: .command).disabled(path == "/")
            Button("") { if let e = selected.first { open(e) } }.keyboardShortcut(.downArrow, modifiers: .command).disabled(selection.isEmpty)
            Button("") { viewMode = ViewMode.list.rawValue }.keyboardShortcut("1", modifiers: .command)
            Button("") { viewMode = ViewMode.icons.rawValue }.keyboardShortcut("2", modifiers: .command)
            Button("") { filterFocused = true }.keyboardShortcut("f", modifiers: .command)
            Button("") { makeAlias() }.keyboardShortcut("a", modifiers: [.command, .control]).disabled(selection.isEmpty || !canWrite || !isNative)
            Button("") { goBack() }.keyboardShortcut("[", modifiers: .command).disabled(backStack.isEmpty)
            Button("") { goForward() }.keyboardShortcut("]", modifiers: .command).disabled(forwardStack.isEmpty)
            if mode == .icons { Button("") { selection = Set(rows.map(\.id)) }.keyboardShortcut("a", modifiers: .command).disabled(filterFocused) }
        }
        .frame(width: 0, height: 0).opacity(0).allowsHitTesting(false)
    }

    // MARK: footer / progress

    private var footer: some View {
        HStack(spacing: 8) {
            if let t = transfer {
                TimelineView(.periodic(from: .now, by: 0.1)) { _ in transferBar(t) }
            } else {
                if busy { ProgressView().controlSize(.small) }
                Text(status ?? "\(rows.count) item\(rows.count == 1 ? "" : "s")" + (selection.isEmpty ? "" : " · \(selection.count) selected"))
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
        }.padding(.horizontal, 14).padding(.vertical, 5).frame(minHeight: 26)
    }

    private func transferBar(_ t: TransferProgress) -> some View {
        let s = t.snapshot
        let el = max(Date().timeIntervalSince(t.started), 0.001)
        let frac = s.bytesTotal > 0 ? Double(s.bytesDone) / Double(s.bytesTotal) : (s.filesTotal > 0 ? Double(s.filesDone) / Double(s.filesTotal) : 0)
        let rate = Double(s.bytesDone) / el
        let left = rate > 1 && s.bytesTotal > s.bytesDone ? Double(s.bytesTotal - s.bytesDone) / rate : 0
        return HStack(spacing: 10) {
            ProgressView(value: min(max(frac, 0), 1)).frame(width: 180)
            Text("\(formatBytes(s.bytesDone)) of \(formatBytes(s.bytesTotal))\(s.scanning ? "+" : "")")
            if rate > 1 { Text("\(formatBytes(Int64(rate)))/s").monospacedDigit() }
            if left >= 1 { Text("\(Int(left.rounded(.up))) s left") }
            Text("\(s.filesDone.formatted()) of \(s.filesTotal.formatted()) files")
            if s.errors > 0 { Text("\(s.errors) failed").foregroundStyle(.red) }
            Button("Cancel") { t.cancel() }.controlSize(.small)
        }.font(.caption).foregroundStyle(.secondary).lineLimit(1)
    }

    // MARK: navigation

    private func publishNav() {
        nav.innerCanBack = !backStack.isEmpty
        nav.innerBack = { goBack() }
    }
    func go(_ p: String) {
        guard p != path else { return }
        backStack.append(path); forwardStack = []
        path = p; selection = []; filter = ""; load()
    }
    func goBack() { guard let p = backStack.popLast() else { return }; forwardStack.append(path); path = p; selection = []; load() }
    func goForward() { guard let p = forwardStack.popLast() else { return }; backStack.append(path); path = p; selection = []; load() }
    func goUp() {
        guard path != "/" else { return }
        let parent = (path as NSString).deletingLastPathComponent
        go(parent.isEmpty ? "/" : parent)
    }

    func open(_ e: Listing.Entry) {
        if isNative, let u = url(e), (e.isDir && u.pathExtension.lowercased() == "app") || !e.isDir { NSWorkspace.shared.open(u); return }
        if e.isDir { go(joinPath(path, e.name)) } else { export(e) }
    }

    func load(select: [String] = []) {
        let (b, p) = (backend, path)
        Task.detached {
            let r = Result { try b.list(p) }
            await MainActor.run {
                switch r {
                case .success(let l): listing = l; error = nil
                case .failure(let e): listing = nil; error = e.localizedDescription
                }
                recompute()
                if !select.isEmpty { selection = Set(select.filter { n in rows.contains { $0.id == n } }) }
            }
        }
    }

    private func recompute() {
        var e = listing?.entries ?? []
        if !showHidden { e = e.filter { !$0.hidden } }
        if !filter.isEmpty { e = e.filter { $0.name.localizedCaseInsensitiveContains(filter) } }
        e.sort(using: sortOrder)
        rows = e.filter(\.isDir) + e.filter { !$0.isDir }
        selection = selection.filter { id in rows.contains { $0.id == id } }
    }

    /// Runs a blocking mutation off the main thread, then refreshes (and selects the named items).
    func run(_ ok: String, select: [String] = [], _ work: @escaping () throws -> Void) {
        busy = true
        Task.detached {
            let r = Result { try work() }
            await MainActor.run {
                busy = false
                switch r {
                case .success: status = ok
                case .failure(let e): status = "Failed: \(e.localizedDescription)"
                }
                load(select: select)
            }
        }
    }

    // MARK: copy / cut / paste

    /// Cut needs somewhere to remove the originals from: a writable volume.
    private var canMoveFromHere: Bool { canWrite }

    func copySelection() {
        let es = selected
        guard !es.isEmpty else { return }
        Clip.shared.copy(backend: backend, items: es.map { (joinPath(path, $0.name), $0) })
        cutTick += 1
        status = "Copied \(es.count) item\(es.count == 1 ? "" : "s")"
    }

    func cutSelection() {
        let es = selected
        guard !es.isEmpty else { return }
        guard canMoveFromHere else { copySelection(); status = "This volume is read-only, so the items were copied instead of cut"; return }
        Clip.shared.copy(backend: backend, items: es.map { (joinPath(path, $0.name), $0) }, cut: true)
        cutTick += 1
        status = "Cut \(es.count) item\(es.count == 1 ? "" : "s") · paste to move them"
    }

    func paste(forceMove: Bool = false) {
        guard canWrite else { status = "Unlock writing first to paste into an image"; return }
        let src = Clip.shared.source()
        let moving: Bool
        switch src { case .clip: moving = forceMove || Clip.shared.isCut; case .files: moving = false; case .none: moving = false }
        let cutBackend = Clip.shared.backend, cutItems = Clip.shared.items

        switch src {
        case .none: status = "Nothing to paste"
        case .files(let urls):
            if isNative { startTransfer(urls, mode: .copy, verb: "Pasted") } else { genericPaste(.files(urls), moving: false, cutBackend: nil, cutItems: []) }
        case .clip(let sb, let items):
            if isNative, sb.isNative, let urls = Optional(items.compactMap { sb.hostURL($0.path) }), urls.count == items.count {
                startTransfer(urls, mode: moving ? .move : .copy, verb: moving ? "Moved" : "Pasted", clearCut: moving)
            } else {
                genericPaste(src, moving: moving, cutBackend: cutBackend, cutItems: cutItems)
            }
        }
    }

    /// Mounted volume → mounted volume (or Finder → here): the parallel engine.
    private func startTransfer(_ urls: [URL], mode: FastTransfer.Mode, verb: String, clearCut: Bool = false) {
        guard let dstDir = backend.hostURL(path) else { return }
        let p = TransferProgress()
        transfer = p; busy = true; status = nil
        let placed = PlacedBox()
        Task.detached(priority: .userInitiated) {
            let r = FastTransfer.run(urls, into: dstDir, mode: mode, progress: p) { placed.add($0.lastPathComponent) }
            await MainActor.run {
                transfer = nil; busy = false
                if clearCut { Clip.shared.items = []; Clip.shared.clearCut(); NSPasteboard.general.clearContents(); cutTick += 1 }
                status = Self.summary(verb: verb, r)
                load(select: placed.names)
            }
        }
    }

    static func summary(verb: String, _ r: FastTransfer.Result) -> String {
        let n = r.items
        var s: String
        if r.cancelled { s = "Cancelled after \(r.files.formatted()) file\(r.files == 1 ? "" : "s")" }
        else if r.renamed == n && n > 0 { s = "\(verb) \(n) item\(n == 1 ? "" : "s") instantly (same volume)" }
        else {
            s = "\(verb) \(r.files.formatted()) file\(r.files == 1 ? "" : "s")"
            if r.bytes > 0 { s += " · \(formatBytes(r.bytes))" }
            s += String(format: " in %.1f s", r.seconds)
            if r.bytes > 1_000_000, r.seconds > 0.05 { s += " · \(formatBytes(Int64(Double(r.bytes) / r.seconds)))/s" }
            else if r.seconds > 0.05 { s += " · \(Int(Double(r.files) / r.seconds).formatted()) files/s" }
            if r.cloned > 0 { s += " · \(r.cloned.formatted()) cloned instantly" }
        }
        if r.errors > 0 { s += " · \(r.errors) failed" + (r.firstError.map { " (\($0))" } ?? "") }
        return s
    }

    /// Any other pairing (disk images, the NTFS helper, ext4 …): the engine copies one file at a time.
    private func genericPaste(_ src: Clip.Source, moving: Bool, cutBackend: FSBackend?, cutItems: [(path: String, entry: Listing.Entry)]) {
        let (dst, dir) = (backend, path)
        busy = true
        status = moving ? "Moving…" : "Pasting…"
        Task.detached {
            var count = 0
            var failure: String?
            var movedNames: [String] = []
            do {
                var taken = Copier.takenNames(dst, dir)
                switch src {
                case .clip(let sb, let items):
                    for it in items {
                        let before = taken
                        count += try Copier.copy(src: sb, srcPath: it.path, entry: it.entry, dst: dst, dstDir: dir, taken: &taken) { n in
                            Task { @MainActor in status = "\(moving ? "Moving" : "Pasting") \(n)…" }
                        }
                        movedNames += taken.subtracting(before).map { $0 }
                        if moving { // the copy landed, so the original can go
                            if sb.isNative, let u = sb.hostURL(it.path) { try FileManager.default.removeItem(at: u) } else { try sb.delete(it.path) }
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
                if moving && fail == nil { Clip.shared.items = []; Clip.shared.clearCut(); NSPasteboard.general.clearContents(); cutTick += 1 }
                let verb = moving ? "Moved" : "Pasted"
                status = fail.map { "\(verb) stopped after \(done) item\(done == 1 ? "" : "s"): \($0)" } ?? "\(verb) \(done) item\(done == 1 ? "" : "s")"
                load()
            }
        }
    }

    /// Files dropped from Finder (or another VOLUMA window): same volume moves, other volumes copy, ⌥ forces a copy.
    private func dropURLs(_ urls: [URL]) -> Bool {
        let files = urls.filter(\.isFileURL)
        guard !files.isEmpty, canWrite, isNative, let dstDir = backend.hostURL(path) else { status = canWrite ? "Drop files onto a mounted volume" : "This volume is read-only"; return false }
        if files.allSatisfy({ $0.deletingLastPathComponent().standardizedFileURL.path == dstDir.standardizedFileURL.path }) { return false }   // dropped where it already is
        var d = stat(), s = stat()
        let sameVolume = stat(dstDir.path, &d) == 0 && files.allSatisfy { stat($0.path, &s) == 0 && s.st_dev == d.st_dev }
        let move = sameVolume && !NSEvent.modifierFlags.contains(.option)
        startTransfer(files, mode: move ? .move : .copy, verb: move ? "Moved" : "Copied")
        return true
    }

    // MARK: new file / folder

    func openNewFile(_ t: FileTemplate) {
        guard canWrite else { return }
        newFileTemplate = t
        newFileName = NameClash.unique(t.defaultName, taken: Set((listing?.entries ?? []).map { $0.name.lowercased() }))
        showNewFile = true
    }

    private var newFileSheet: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New File").font(.title3.weight(.semibold))
            TextField("Name", text: $newFileName).textFieldStyle(.roundedBorder).onSubmit { createNewFile() }
            Picker("Template", selection: $newFileTemplate) {
                ForEach(FileTemplate.all) { t in Label(t.title, systemImage: t.symbol).tag(t) }
            }
            .onChange(of: newFileTemplate) { old, new in newFileName = Self.swapExtension(newFileName, from: old, to: new) }
            Text("In \(path == "/" ? target.title : path)").font(.caption).foregroundStyle(.secondary).lineLimit(1)
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { showNewFile = false }.keyboardShortcut(.cancelAction)
                Button("Create") { createNewFile() }.keyboardShortcut(.defaultAction)
                    .disabled(newFileName.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }.padding(20).frame(width: 380)
    }

    static func swapExtension(_ name: String, from: FileTemplate, to: FileTemplate) -> String {
        let ns = name as NSString
        let base = ns.pathExtension == from.ext || from.ext.isEmpty && ns.pathExtension.isEmpty ? ns.deletingPathExtension : name
        let stem = (base == "untitled file" || base.isEmpty) ? "untitled" : base
        return to.ext.isEmpty ? (stem == "untitled" ? "untitled file" : stem) : "\(stem).\(to.ext)"
    }

    func createNewFile() {
        let raw = newFileName.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
        guard !raw.isEmpty else { return }
        showNewFile = false
        let name = NameClash.unique(raw, taken: Set((listing?.entries ?? []).map { $0.name.lowercased() }))   // never replaces an existing file
        let (b, p, t) = (backend, joinPath(path, name), newFileTemplate)
        run("Created \(name)", select: [name]) {
            try b.createFile(p, contents: t.data)
            if t.executable, let u = b.hostURL(p) { chmod(u.path, 0o755) }
        }
    }

    func createFolder(_ raw: String) {
        let n = raw.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "/", with: "-")
        guard !n.isEmpty else { return }
        let name = NameClash.unique(n, taken: Set((listing?.entries ?? []).map { $0.name.lowercased() }))
        let (b, p) = (backend, joinPath(path, name))
        run("Created \(name)", select: [name]) { try b.mkdir(p) }
    }

    // MARK: other actions

    func deleteSelected(_ es: [Listing.Entry]) {
        let (b, dir) = (backend, path)
        run("\(isNative ? "Moved to Trash" : "Deleted") \(es.count) item\(es.count == 1 ? "" : "s")") { for e in es { try b.delete(joinPath(dir, e.name)) } }
    }

    func duplicate() {
        let es = selected
        guard !es.isEmpty, canWrite else { return }
        if isNative {
            startTransfer(es.compactMap { url($0) }, mode: .copy, verb: "Duplicated")
        } else {
            let (b, dir) = (backend, path)
            run("Duplicated \(es.count) item\(es.count == 1 ? "" : "s")") {
                var taken = Copier.takenNames(b, dir)
                for e in es { try Copier.copy(src: b, srcPath: joinPath(dir, e.name), entry: e, dst: b, dstDir: dir, taken: &taken) { _ in } }
            }
        }
    }

    func makeAlias() {
        let urls = selected.compactMap { url($0) }
        guard isNative, !urls.isEmpty else { return }
        run("Made \(urls.count) alias\(urls.count == 1 ? "" : "es")") {
            for u in urls {
                let dir = u.deletingLastPathComponent()
                var taken = Set(((try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []).map { $0.lowercased() })
                taken.insert("") ; let name = NameClash.unique(u.lastPathComponent + " alias", taken: taken)
                let data = try u.bookmarkData(options: .suitableForBookmarkFile)
                try URL.writeBookmarkData(data, to: dir.appendingPathComponent(name))
            }
        }
    }

    func compress() {
        let es = selected, dirURL = backend.hostURL(path)
        guard isNative, !es.isEmpty, let dir = dirURL else { return }
        let single = es.count == 1
        let base = single ? es[0].name : "Archive"
        let zip = NameClash.unique(base + ".zip", taken: Set((listing?.entries ?? []).map { $0.name.lowercased() }))
        let names = es.map(\.name)
        run("Compressed to \(zip)", select: [zip]) {
            let p = Process()
            p.currentDirectoryURL = dir
            if single { p.executableURL = URL(fileURLWithPath: "/usr/bin/ditto"); p.arguments = ["-c", "-k", "--sequesterRsrc", "--keepParent", names[0], zip] }
            else { p.executableURL = URL(fileURLWithPath: "/usr/bin/zip"); p.arguments = ["-r", "-y", "-q", zip] + names }
            p.standardOutput = FileHandle.nullDevice; p.standardError = FileHandle.nullDevice
            try p.run(); p.waitUntilExit()
            if p.terminationStatus != 0 { throw CocoaError(.fileWriteUnknown, userInfo: [NSLocalizedDescriptionKey: "compression failed (\(p.terminationStatus))"]) }
        }
    }

    func copyPath() {
        let paths = selected.map { e -> String in
            if let u = url(e) { return u.path }
            return joinPath(path, e.name)
        }
        guard !paths.isEmpty else { return }
        let pb = NSPasteboard.general
        pb.clearContents(); pb.setString(paths.joined(separator: "\n"), forType: .string)
        status = "Copied path\(paths.count == 1 ? "" : "s") as text"
    }

    func applyTag(_ t: FinderTag) {
        let urls = selected.compactMap { url($0) }
        let names = selected.map(\.id)
        guard !urls.isEmpty else { return }
        let ok = FinderTag.toggle(t, on: urls)
        status = ok ? "Tagged \(urls.count) item\(urls.count == 1 ? "" : "s")" : "Couldn’t set the tag on some items"
        load(select: names)
    }

    func clearTags() {
        let urls = selected.compactMap { url($0) }, names = selected.map(\.id)
        FinderTag.clear(on: urls); status = "Removed tags"; load(select: names)
    }

    func showInfo() { if selection.count == 1, let e = selected.first { infoEntry = e } }

    func quickLook() {
        let es = selected
        guard !es.isEmpty else { return }
        if isNative { QuickLook.shared.toggle(es.compactMap { url($0) }); return }
        // images / helper volumes: export the first file to a temp folder so Quick Look can read it
        guard let e = es.first, !e.isDir else { return }
        let b = backend, src = joinPath(path, e.name)
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("voluma-ql-\(UUID().uuidString.prefix(6))")
        status = "Preparing preview…"
        Task.detached {
            try? FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
            let dest = tmp.appendingPathComponent(e.name)
            let r = Result { try b.exportFile(src, to: dest) }
            await MainActor.run {
                if case .success = r { status = nil; QuickLook.shared.toggle([dest]) } else { status = "Couldn’t preview \(e.name)" }
            }
        }
    }

    func importFiles() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = true
        panel.message = "Choose files or folders to copy into \(path)"
        guard panel.runModal() == .OK else { return }
        let (dst, dir, urls) = (backend, path, panel.urls)
        if isNative { startTransfer(urls, mode: .copy, verb: "Imported"); return }
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

/// Thread-safe list of the names a transfer placed, so they can be selected when it finishes.
final class PlacedBox: @unchecked Sendable {
    private let lock = NSLock(); private var _names: [String] = []
    func add(_ n: String) { lock.lock(); _names.append(n); lock.unlock() }
    var names: [String] { lock.lock(); defer { lock.unlock() }; return _names }
}

/// Drag a row out to Finder, the Desktop or another app (mounted volumes only).
private struct DragOut: ViewModifier {
    let url: URL?
    func body(content: Content) -> some View {
        if let url { content.draggable(url) } else { content }
    }
}
