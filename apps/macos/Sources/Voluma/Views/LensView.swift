import SwiftUI
import AppKit

/// Storage Lens: scans a volume or folder and shows where the space went, Finder-style.
struct LensView: View {
    @ObservedObject var model: LensModel
    @ObservedObject var monitor: DiskMonitor

    var body: some View {
        if model.scanning { scanning }
        else if let r = model.result, let cur = model.current { LensResultView(model: model, result: r, current: cur) }
        else { picker }
    }

    private var picker: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text("Choose something to analyse").font(.title3).foregroundStyle(.secondary)
                ForEach(monitor.mountedVolumes.filter { !$0.isNetwork }) { v in
                    HStack {
                        Image(systemName: "externaldrive").foregroundStyle(.secondary)
                        VStack(alignment: .leading) {
                            Text(v.title).font(.headline)
                            Text(v.mountPath ?? "").font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if let t = v.total, let f = v.free {
                            Text("\(formatBytes(t - f)) used of \(formatBytes(t))").font(.caption).foregroundStyle(.secondary)
                        }
                        Button("Scan") { model.start(url: URL(fileURLWithPath: v.mountPath!), title: v.title, volume: v) }
                    }
                    .padding(14).background(.background.secondary, in: RoundedRectangle(cornerRadius: 10))
                }
                Button("Choose Folder…", systemImage: "folder") { chooseFolder() }
                Text("Scans read file metadata only. Folders macOS protects (Desktop, Documents, Photos…) may ask for permission or be skipped.")
                    .font(.caption).foregroundStyle(.tertiary)
            }.padding(24).frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func chooseFolder() {
        let p = NSOpenPanel()
        p.canChooseDirectories = true; p.canChooseFiles = false; p.prompt = "Scan"
        if p.runModal() == .OK, let u = p.url { model.start(url: u, title: u.lastPathComponent, volume: nil) }
    }

    private var scanning: some View {
        VStack(spacing: 14) {
            ProgressView().controlSize(.large)
            Text("Scanning \(model.targetTitle)…").font(.title3)
            Text("\(model.live.files.formatted()) files · \(formatBytes(model.live.bytes))").foregroundStyle(.secondary).monospacedDigit()
            Text(model.live.current).font(.caption).foregroundStyle(.tertiary).lineLimit(1).truncationMode(.middle).frame(maxWidth: 560)
            Button("Cancel") { model.cancel() }
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct Row: Identifiable {
    enum Kind { case folder, file, rest }
    let id: String
    let name: String; let size: Int64; let items: Int; let kind: Kind
    let category: FileCategory?; let url: URL?; let node: FolderNode?; let unreadable: Bool
}

struct LensResultView: View {
    @ObservedObject var model: LensModel
    let result: ScanResult
    let current: FolderNode
    @State private var tab = 0
    @State private var selection: Set<String> = []

    private var rows: [Row] {
        var r: [Row] = current.children.map {
            Row(id: "d" + $0.name, name: $0.name, size: $0.size, items: $0.itemCount, kind: .folder, category: nil, url: $0.url, node: $0, unreadable: $0.unreadable)
        }
        r += current.files.map { Row(id: "f" + $0.name, name: $0.name, size: $0.size, items: 1, kind: .file, category: $0.category, url: current.url(of: $0), node: nil, unreadable: false) }
        if current.omittedFiles > 0 {
            r.append(Row(id: "rest", name: "\(current.omittedFiles.formatted()) smaller files", size: current.omittedBytes, items: current.omittedFiles, kind: .rest, category: nil, url: nil, node: nil, unreadable: false))
        }
        return r.sorted { $0.size > $1.size }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            HStack(spacing: 8) {
                Picker("", selection: $tab) { Text("Folders").tag(0); Text("Largest Files").tag(1) }
                    .pickerStyle(.segmented).frame(width: 240).labelsHidden()
                if tab == 0 { breadcrumbs }
                Spacer()
                Button("Rescan", systemImage: "arrow.clockwise") {
                    if let u = model.target { model.start(url: u, title: model.targetTitle, volume: model.volume) }
                }.controlSize(.small)
                Button("New Scan", systemImage: "chart.pie") { model.result = nil; model.current = nil }.controlSize(.small)
            }.padding(.horizontal, 14).padding(.vertical, 8)
            Divider()
            if tab == 0 { folderTable } else { largestTable }
        }
    }

    // MARK: header with category bar
    private var header: some View {
        let total = max(current.size, 1)
        let vol = model.volume
        let freeBytes: Int64 = (current === result.root ? vol?.free : nil) ?? 0
        let denom = Double(total + freeBytes)
        return VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                Text(current === result.root ? model.targetTitle : current.name).font(.title2.weight(.semibold))
                Spacer()
                Text(formatBytes(current.size)).font(.title2.monospacedDigit())
            }
            Text(summaryLine).font(.caption).foregroundStyle(.secondary)
            GeometryReader { g in
                HStack(spacing: 1) {
                    ForEach(FileCategory.allCases) { c in
                        let v = current.cats[c.rawValue]
                        if v > 0 { Rectangle().fill(c.color).frame(width: max(2, g.size.width * Double(v) / denom)).help("\(c.label): \(formatBytes(v))") }
                    }
                    if freeBytes > 0 { Rectangle().fill(.quaternary).frame(width: g.size.width * Double(freeBytes) / denom).help("Available: \(formatBytes(freeBytes))") }
                    Spacer(minLength: 0)
                }
            }.frame(height: 16).clipShape(RoundedRectangle(cornerRadius: 5))
            FlowLegend(items: FileCategory.allCases.filter { current.cats[$0.rawValue] > 0 }.map { ($0.label, $0.color, current.cats[$0.rawValue]) }
                       + (freeBytes > 0 ? [("Available", Color.gray.opacity(0.35), freeBytes)] : []))
        }.padding(16)
    }

    private var summaryLine: String {
        var s = "\(current.itemCount.formatted()) files · scanned in \(String(format: "%.1f", result.seconds))s"
        if result.unreadableFolders > 0 { s += " · \(result.unreadableFolders) folder\(result.unreadableFolders == 1 ? "" : "s") couldn’t be read (permissions)" }
        if result.cancelled { s += " · scan was cancelled, totals are partial" }
        if let v = model.volume, current === result.root, let t = v.total, let f = v.free {
            let unexplained = max(0, t - f - current.size)
            if unexplained > t / 50 { s += " · \(formatBytes(unexplained)) used on the volume isn’t visible to this scan (snapshots, protected or other-volume data)" }
        }
        return s
    }

    private var breadcrumbs: some View {
        let chain: [FolderNode] = { var c: [FolderNode] = []; var n: FolderNode? = current; while let x = n { c.insert(x, at: 0); n = x.parent }; return c }()
        return HStack(spacing: 4) {
            Button { if let p = current.parent { model.current = p; selection = [] } } label: { Image(systemName: "chevron.left") }
                .disabled(current.parent == nil).help("Enclosing folder")
            ForEach(Array(chain.enumerated()), id: \.offset) { i, n in
                if i > 0 { Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary) }
                Button(i == 0 ? model.targetTitle : n.name) { model.current = n; selection = [] }.buttonStyle(.link)
            }
        }
    }

    // MARK: tables
    private var folderTable: some View {
        let rows = self.rows
        let total = Double(max(current.size, 1))
        return Table(rows, selection: $selection) {
            TableColumn("Name") { r in
                HStack(spacing: 6) {
                    Image(systemName: r.kind == .folder ? "folder.fill" : (r.category?.symbol ?? "ellipsis.circle"))
                        .foregroundStyle(r.kind == .folder ? Color.accentColor : (r.category?.color ?? .secondary))
                    Text(r.name).lineLimit(1)
                    if r.unreadable { Image(systemName: "lock.fill").font(.caption2).foregroundStyle(.orange).help("Couldn’t read this folder") }
                }
            }
            TableColumn("Share") { r in UsageBar(fraction: Double(r.size) / total).frame(width: 140) }.width(150)
            TableColumn("Size") { r in Text(formatBytes(r.size)).monospacedDigit() }.width(90)
            TableColumn("Items") { r in Text(r.kind == .file ? "" : r.items.formatted()).foregroundStyle(.secondary).monospacedDigit() }.width(80)
        }
        .contextMenu(forSelectionType: String.self) { ids in
            if let r = rows.first(where: { ids.contains($0.id) }), let u = r.url {
                Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([u]) }
                if r.kind == .folder, let n = r.node { Button("Open") { model.current = n; selection = [] } }
            }
        } primaryAction: { ids in
            if let r = rows.first(where: { ids.contains($0.id) }) {
                if r.kind == .folder, let n = r.node { model.current = n; selection = [] }
                else if let u = r.url { NSWorkspace.shared.activateFileViewerSelecting([u]) }
            }
        }
    }

    private var largestTable: some View {
        Table(result.largest) {
            TableColumn("Name") { f in
                HStack(spacing: 6) { Image(systemName: f.item.category.symbol).foregroundStyle(f.item.category.color); Text(f.item.name).lineLimit(1) }
            }
            TableColumn("Where") { f in
                Text(f.url.deletingLastPathComponent().path).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
            }
            TableColumn("Kind") { f in Text(f.item.category.label).foregroundStyle(.secondary) }.width(120)
            TableColumn("Size") { f in Text(formatBytes(f.item.size)).monospacedDigit() }.width(90)
        }
        .contextMenu(forSelectionType: UUID.self) { ids in
            if let f = result.largest.first(where: { ids.contains($0.id) }) { Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([f.url]) } }
        } primaryAction: { ids in
            if let f = result.largest.first(where: { ids.contains($0.id) }) { NSWorkspace.shared.activateFileViewerSelecting([f.url]) }
        }
    }
}

struct FlowLegend: View {
    let items: [(String, Color, Int64)]
    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 175), alignment: .leading)], alignment: .leading, spacing: 6) {
            ForEach(Array(items.enumerated()), id: \.offset) { _, i in
                HStack(spacing: 5) {
                    Circle().fill(i.1).frame(width: 8, height: 8)
                    Text(i.0).font(.caption).lineLimit(1)
                    Text(formatBytes(i.2)).font(.caption).foregroundStyle(.secondary).monospacedDigit().lineLimit(1)
                }.fixedSize()
            }
        }
    }
}
