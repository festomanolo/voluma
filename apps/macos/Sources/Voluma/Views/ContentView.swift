import SwiftUI

enum Section: String, CaseIterable, Identifiable {
    case disks = "Disks", lens = "Storage Lens", images = "Disk Images"
    var id: String { rawValue }
    var icon: String {
        switch self { case .disks: "internaldrive"; case .lens: "chart.pie"; case .images: "opticaldisc" }
    }
}

enum SidebarItem: Hashable { case section(Section), place(String) }

/// Finder-style shortcuts to the folders you use every day, browsed natively with the full set of Finder actions.
}

struct ContentView: View {
    /// Where the window is: a section, optionally with a browser open on top of it.
    struct Place: Equatable { var section: Section; var browse: BrowseTarget? }

    @State private var place = Place(section: .disks, browse: nil)
    @State private var history: [Place] = []
    @State private var collapsedDisks: Set<String> = []
    @State private var rawError: String?
    @State private var report: ImageReport?
    @State private var errorText: String?
    @State private var importing = false
    @StateObject private var monitor = DiskMonitor()
    @StateObject private var lens = LensModel()
    @StateObject private var nav = NavState()

    private func navigate(section: Section? = nil, browse: BrowseTarget? = nil) {
        let next = Place(section: section ?? place.section, browse: browse)
        guard next != place else { return }
        history.append(place)
        if history.count > 100 { history.removeFirst() }
        place = next
    }

    /// Back first steps out of folders inside the open browser, then through the pages visited.
    private func goBack() {
        if place.browse != nil, nav.innerCanBack { nav.innerBack?(); return }
        if let p = history.popLast() { place = p }
    }
    private var canGoBack: Bool { (place.browse != nil && nav.innerCanBack) || !history.isEmpty }

    var body: some View {
        NavigationSplitView {
            List(Section.allCases, selection: Binding<Section?>(get: { place.section }, set: { if let s = $0 { navigate(section: s) } })) { s in
                Label(s.rawValue, systemImage: s.icon).tag(s)
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 210, max: 280)
        } detail: {
            Group {
                if let t = place.browse {
                    BrowserView(target: t).id(t)
                } else {
                    switch place.section {
                    case .disks:
                        DisksView(monitor: monitor, collapsed: $collapsedDisks,
                                  openLens: { node in
                                      guard let m = node.mountPath else { return }
                                      lens.start(url: URL(fileURLWithPath: m), title: node.title, volume: node)
                                      navigate(section: .lens)
                                  },
                                  openBrowse: { node in
                                      guard let m = node.mountPath else {
                                          // Unmounted partition: read it raw through the helper (admin prompt unless the device is ours).
                                          Task { @MainActor in
                                              do {
                                                  _ = try await NtfsSessions.shared.start(bsd: node.bsd, title: node.title, mounted: false)
                                                  navigate(browse: BrowseTarget(image: "", part: -1, title: node.title, bsd: node.bsd, viaHelper: true, rawOnly: true))
                                              } catch { rawError = error.localizedDescription }
                                          }
                                          return
                                      }
                                      let ro = (try? URL(fileURLWithPath: m).resourceValues(forKeys: [.volumeIsReadOnlyKey]))?.volumeIsReadOnly ?? false
                                      navigate(browse: BrowseTarget(image: "", part: -1, title: node.title, nativeRoot: m,
                                                                    bsd: node.bsd, fsKind: node.fsKind, readOnlyMount: ro))
                                  })
                    case .lens: LensView(model: lens, monitor: monitor)
                    case .images: ImageInspectorView(report: report, error: errorText, open: { importing = true },
                                                     browse: { navigate(section: .images, browse: $0) })
                    }
                }
            }
            .navigationTitle(place.browse?.title ?? place.section.rawValue)
        }
        .environmentObject(nav)
        .onAppear {
            nav.replaceBrowse = { t in place.browse = t }
            nav.closeBrowser = { if let p = history.popLast() { place = p } else { place = Place(section: .disks, browse: nil) } }
        }
        .toolbar {
            ToolbarItem(placement: .navigation) {
                Button { goBack() } label: { Label("Back", systemImage: "chevron.left") }
                    .disabled(!canGoBack).keyboardShortcut("[", modifiers: .command).help("Back (⌘[)")
            }
            ToolbarItem { Button("Refresh Disks", systemImage: "arrow.clockwise") { monitor.refresh() } }
            ToolbarItem { Button("Open Disk Image…", systemImage: "plus") { importing = true } }
        }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.data]) { result in
            guard case .success(let url) = result else { return }
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            do { report = try Engine.inspectImage(path: url.path); errorText = nil }
            catch { report = nil; errorText = error.localizedDescription }
            navigate(section: .images)
        }
        .alert("Couldn’t open the partition", isPresented: Binding(get: { rawError != nil }, set: { if !$0 { rawError = nil } })) {
            Button("OK", role: .cancel) {}
        } message: { Text(rawError ?? "") }
        .frame(minWidth: 960, minHeight: 600)
    }
}

struct ImageInspectorView: View {
    let report: ImageReport?; let error: String?; let open: () -> Void; let browse: (BrowseTarget) -> Void
    var body: some View {
        if let error {
            ContentUnavailableView("Couldn't read image", systemImage: "exclamationmark.triangle", description: Text(error))
        } else if let r = report {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text((r.path as NSString).lastPathComponent).font(.title2.weight(.semibold))
                        Text("\(formatBytes(Int64(r.size))) · \(r.table.kind.uppercased()) · opened read-only")
                            .foregroundStyle(.secondary)
                    }
                    if let fs = r.whole_disk_filesystem {
                        row("Filesystem (no partition table)", fs.detail ?? fs.kind, fs.label,
                            browse: canBrowse(fs.kind) ? { browse(.init(image: r.path, part: -1, title: fs.label ?? "Volume")) } : nil)
                    }
                    ForEach(r.partitions) { p in
                        row("Partition \(p.index) · \(p.type_name ?? p.type_id)",
                            p.filesystem.detail ?? p.filesystem.kind,
                            p.filesystem.label ?? p.name,
                            extra: "\(formatBytes(Int64(p.length))) @ offset \(p.offset)",
                            browse: canBrowse(p.filesystem.kind) ? { browse(.init(image: r.path, part: p.index, title: p.filesystem.label ?? p.name ?? "Partition \(p.index)")) } : nil)
                    }
                }.padding(24).frame(maxWidth: .infinity, alignment: .leading)
            }
        } else {
            ContentUnavailableView {
                Label("No image open", systemImage: "opticaldisc")
            } description: {
                Text("Open a raw disk image to inspect its partitions and filesystems.")
            } actions: {
                Button("Open Disk Image…", action: open).buttonStyle(.borderedProminent)
            }
        }
    }

    func canBrowse(_ kind: String) -> Bool { ["fat12", "fat16", "fat32", "ex_fat", "hfs_plus", "ext", "apfs", "ntfs"].contains(kind) }

    func row(_ title: String, _ fs: String, _ label: String?, extra: String? = nil, browse: (() -> Void)? = nil) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.headline)
                if let extra { Text(extra).font(.caption).foregroundStyle(.secondary) }
            }
            Spacer()
            if let label { Text(label).foregroundStyle(.secondary) }
            Text(fs).font(.caption.monospaced())
                .padding(.horizontal, 7).padding(.vertical, 2)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 5))
            if let browse { Button("Browse", action: browse) }
            else { Text("No driver yet").font(.caption).foregroundStyle(.tertiary) }
        }
        .padding(14).background(.background.secondary, in: RoundedRectangle(cornerRadius: 10))
    }
}
