import SwiftUI
import AppKit

/// Every physical disk, image and network volume the OS knows about, as a collapsible tree:
/// disk → partitions → APFS containers → volumes, with capacity and free space.
/// Read live from IOKit / DiskArbitration; nothing is simulated.
struct DisksView: View {
    @ObservedObject var monitor: DiskMonitor
    /// Owned by the window so the tree keeps its shape when you browse into a volume and come back.
    @Binding var collapsed: Set<String>
    let openLens: (DiskNode) -> Void
    let openBrowse: (DiskNode) -> Void

    @State private var ejecting: Set<String> = []
    @State private var ejectError: String?

    var body: some View {
        if !monitor.loaded {
            ProgressView("Reading disks…").frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if monitor.disks.isEmpty {
            ContentUnavailableView("No disks found", systemImage: "externaldrive.badge.questionmark")
        } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    header
                    ForEach(monitor.disks) { d in
                        DiskCard(disk: d, collapsed: $collapsed, ejecting: ejecting.contains(d.bsd),
                                 openLens: openLens, openBrowse: openBrowse, eject: { eject(d) })
                    }
                }.padding(24)
            }
            .alert("Couldn’t eject", isPresented: Binding(get: { ejectError != nil }, set: { if !$0 { ejectError = nil } })) {
                Button("OK", role: .cancel) {}
            } message: { Text(ejectError ?? "") }
        }
    }

    private var header: some View {
        let physical = monitor.disks.filter { !$0.isDiskImage }
        let cap = physical.reduce(Int64(0)) { $0 + $1.size }
        return HStack {
            Text("\(physical.count) disk\(physical.count == 1 ? "" : "s") · \(formatBytes(cap)) total"
                 + (monitor.disks.contains { $0.isDiskImage } ? " · plus mounted disk images" : ""))
                .font(.title3).foregroundStyle(.secondary)
            Spacer()
            Button("Expand All", systemImage: "arrow.down.right.and.arrow.up.left") { collapsed = [] }.controlSize(.small)
            Button("Collapse All", systemImage: "arrow.up.left.and.arrow.down.right") { collapsed = Set(allWithChildren(monitor.disks)) }.controlSize(.small)
        }
    }

    private func allWithChildren(_ ns: [DiskNode]) -> [String] {
        ns.flatMap { ($0.children.isEmpty ? [] : [$0.bsd]) + allWithChildren($0.children) }
    }

    private func eject(_ d: DiskNode) {
        ejecting.insert(d.bsd)
        Ejector.eject(bsd: d.bsd) { err in
            ejecting.remove(d.bsd)
            if let err { ejectError = "\(d.title): \(err)" }
            monitor.refresh()
        }
    }
}

struct DiskCard: View {
    let disk: DiskNode
    @Binding var collapsed: Set<String>
    let ejecting: Bool
    let openLens: (DiskNode) -> Void
    let openBrowse: (DiskNode) -> Void
    let eject: () -> Void

    private var icon: String {
        if disk.isDiskImage { return "opticaldisc" }
        if disk.isNetwork { return "network" }
        return disk.isInternal == false ? "externaldrive.fill" : "internaldrive.fill"
    }
    private var subtitle: String {
        var parts = [disk.bsd]
        if let p = disk.busProtocol, !p.isEmpty { parts.append(p) }
        if let i = disk.isInternal { parts.append(i ? "Internal" : "External") }
        if disk.removable || disk.ejectable { parts.append("Removable") }
        if !disk.writable { parts.append("Read-only media") }
        return parts.joined(separator: " · ")
    }
    /// External drives, removable media and mounted images can be ejected; the internal disk never.
    private var canEject: Bool { disk.isDiskImage || (disk.isInternal == false && (disk.ejectable || disk.removable || disk.busProtocol != nil)) }
    private var isCollapsed: Bool { collapsed.contains(disk.bsd) }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                if !disk.children.isEmpty { Chevron(bsd: disk.bsd, collapsed: $collapsed) } else { Color.clear.frame(width: 16) }
                Image(systemName: icon).font(.title2).foregroundStyle(.secondary).frame(width: 30)
                VStack(alignment: .leading, spacing: 2) {
                    Text(disk.title).font(.headline)
                    Text(subtitle).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                if canEject {
                    if ejecting { ProgressView().controlSize(.small) }
                    else { Button("Eject", systemImage: "eject.fill", action: eject).controlSize(.small).help("Unmount and safely eject \(disk.title)") }
                }
                Text(formatBytes(disk.size)).font(.headline.monospacedDigit())
            }
            .contentShape(Rectangle())
            .onTapGesture { if !disk.children.isEmpty { toggle(disk.bsd, in: $collapsed) } }
            if disk.isMounted && disk.children.isEmpty {
                // Whole-disk filesystem (no partition map), e.g. a mounted image.
                SpaceRow(node: disk, openLens: openLens, openBrowse: openBrowse)
            }
            if !isCollapsed {
                ForEach(disk.children) { c in PartitionRow(node: c, collapsed: $collapsed, openLens: openLens, openBrowse: openBrowse) }
            }
        }
        .padding(16)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 12))
    }
}

func toggle(_ id: String, in set: Binding<Set<String>>) {
    withAnimation(.easeInOut(duration: 0.15)) {
        if set.wrappedValue.contains(id) { set.wrappedValue.remove(id) } else { set.wrappedValue.insert(id) }
    }
}

struct Chevron: View {
    let bsd: String
    @Binding var collapsed: Set<String>
    var body: some View {
        Button { toggle(bsd, in: $collapsed) } label: {
            Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                .rotationEffect(.degrees(collapsed.contains(bsd) ? 0 : 90)).frame(width: 16, height: 16)
        }
        .buttonStyle(.plain).help(collapsed.contains(bsd) ? "Expand" : "Collapse")
    }
}

struct PartitionRow: View {
    let node: DiskNode
    @Binding var collapsed: Set<String>
    let openLens: (DiskNode) -> Void
    let openBrowse: (DiskNode) -> Void

    private var icon: String {
        if node.isMounted { return "externaldrive" }
        if !node.children.isEmpty { return "square.stack.3d.up" }
        return "rectangle.split.3x1"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Divider()
            HStack(alignment: .top, spacing: 10) {
                if !node.children.isEmpty { Chevron(bsd: node.bsd, collapsed: $collapsed) } else { Color.clear.frame(width: 16) }
                Image(systemName: icon).foregroundStyle(.secondary).frame(width: 22)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(node.title).font(.body.weight(.medium))
                        Text(node.kindLabel).font(.caption.monospaced())
                            .padding(.horizontal, 6).padding(.vertical, 1)
                            .background(.quaternary, in: RoundedRectangle(cornerRadius: 4))
                    }
                    Text(([node.bsd] + (node.mountPath.map { [$0] } ?? ["Not mounted"])).joined(separator: " · "))
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                if node.isMounted { SpaceRow(node: node, openLens: openLens, openBrowse: openBrowse).frame(width: 400) }
                else if node.children.isEmpty || node.total == nil {
                    Text(formatBytes(node.size)).font(.callout.monospacedDigit()).foregroundStyle(.secondary)
                } else {
                    // APFS container: free space is shared by its volumes.
                    VStack(alignment: .trailing, spacing: 4) {
                        Text("\(formatBytes(node.free ?? 0)) free of \(formatBytes(node.size))").font(.caption).foregroundStyle(.secondary)
                        UsageBar(fraction: node.size > 0 ? 1 - Double(node.free ?? 0) / Double(node.size) : 0).frame(width: 400)
                    }
                }
            }
            .contentShape(Rectangle())
            .onTapGesture { if !node.children.isEmpty { toggle(node.bsd, in: $collapsed) } }
            if !collapsed.contains(node.bsd) {
                ForEach(node.children) { c in PartitionRow(node: c, collapsed: $collapsed, openLens: openLens, openBrowse: openBrowse) }
                    .padding(.leading, 28)
            }
        }
    }
}

/// Used/free bar with the buttons that act on a mounted volume.
struct SpaceRow: View {
    let node: DiskNode
    let openLens: (DiskNode) -> Void
    let openBrowse: (DiskNode) -> Void

    var body: some View {
        VStack(alignment: .trailing, spacing: 4) {
            if let t = node.total, let f = node.free, t > 0 {
                Text("\(formatBytes(f)) available of \(formatBytes(t))").font(.caption).foregroundStyle(.secondary)
                UsageBar(fraction: 1 - Double(f) / Double(t))
            } else {
                Text("Capacity unavailable").font(.caption).foregroundStyle(.secondary)
            }
            HStack(spacing: 8) {
                Button("Browse", systemImage: "folder.fill") { openBrowse(node) }.help("Browse files; copy and paste between volumes")
                Button("Storage Lens", systemImage: "chart.pie") { openLens(node) }
                Button("Show in Finder", systemImage: "arrow.up.forward.app") {
                    if let m = node.mountPath { NSWorkspace.shared.open(URL(fileURLWithPath: m)) }
                }
            }.controlSize(.small)
        }
    }
}
