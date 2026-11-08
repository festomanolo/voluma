import SwiftUI

enum Section: String, CaseIterable, Identifiable {
    case dashboard = "Dashboard", images = "Disk Images"
    var id: String { rawValue }
    var icon: String { self == .dashboard ? "externaldrive" : "opticaldisc" }
}

struct ContentView: View {
    @State private var section: Section? = .dashboard
    @State private var report: ImageReport?
    @State private var errorText: String?
    @State private var importing = false
    @State private var browse: BrowseTarget?

    var body: some View {
        NavigationSplitView {
            List(Section.allCases, selection: $section) { s in
                Label(s.rawValue, systemImage: s.icon).tag(s)
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 210, max: 280)
        } detail: {
            Group {
                switch section ?? .dashboard {
                case .dashboard: DashboardView()
                case .images:
                    if let t = browse { BrowserView(target: t) { browse = nil } }
                    else { ImageInspectorView(report: report, error: errorText, open: { importing = true }, browse: { browse = $0 }) }
                }
            }
            .navigationTitle((section ?? .dashboard).rawValue)
        }
        .toolbar {
            ToolbarItem { Button("Open Disk Image…", systemImage: "plus") { importing = true } }
        }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.data]) { result in
            guard case .success(let url) = result else { return }
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            do { report = try Engine.inspectImage(path: url.path); errorText = nil; section = .images }
            catch { report = nil; errorText = error.localizedDescription; section = .images }
        }
        .frame(minWidth: 860, minHeight: 540)
    }
}

struct DashboardView: View {
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                HStack {
                    Text("Every disk. Every filesystem. One place.").font(.title3).foregroundStyle(.secondary)
                    SimulatedBadge()
                }
                ForEach(SimulatedData.devices) { d in
                    VStack(alignment: .leading, spacing: 8) {
                        HStack {
                            Image(systemName: "externaldrive.fill").foregroundStyle(.secondary)
                            VStack(alignment: .leading) {
                                Text(d.name).font(.headline)
                                Text("\(d.model) · \(d.connection)").font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            Text(d.filesystem).font(.caption.monospaced())
                                .padding(.horizontal, 7).padding(.vertical, 2)
                                .background(.quaternary, in: RoundedRectangle(cornerRadius: 5))
                            Text(d.access).font(.caption).foregroundStyle(.secondary)
                        }
                        UsageBar(fraction: Double(d.used) / Double(d.capacity))
                        Text("\(formatBytes(d.used)) of \(formatBytes(d.capacity)) used").font(.caption).foregroundStyle(.secondary)
                    }
                    .padding(14)
                    .background(.background.secondary, in: RoundedRectangle(cornerRadius: 10))
                }
            }.padding(24)
        }
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

    func canBrowse(_ kind: String) -> Bool { ["fat12", "fat16", "fat32"].contains(kind) }

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
