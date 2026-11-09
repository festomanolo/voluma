import Foundation
import IOKit
import DiskArbitration
import AppKit

/// One disk, partition, APFS container or volume as reported by the OS (IOKit + DiskArbitration).
struct DiskNode: Identifiable, Hashable {
    var id: String { bsd }
    let bsd: String
    var isWhole = false
    var size: Int64 = 0
    var content = ""
    var volumeName: String?
    var mountPath: String?
    var fsKind: String?
    var model: String?
    var busProtocol: String?
    var isInternal: Bool?
    var removable = false
    var ejectable = false
    var writable = true
    var isNetwork = false
    /// Capacity / available space of the mounted filesystem (nil when not mounted).
    var total: Int64?
    var free: Int64?
    var children: [DiskNode] = []

    /// An unmounted partition (or whole disk without a partition map) that VOLUMA's drivers might read: offered a raw browse.
    var rawBrowsable: Bool {
        guard !isMounted, children.isEmpty, size >= 1 << 20, !isNetwork else { return false }
        let c = content.uppercased()
        let skip = ["EFI", "C12A7328", "E3C9E316", "APPLE_BOOT", "APPLE_APFS_ISC", "APPLE_APFS_RECOVERY", "GUID_PARTITION_SCHEME", "FDISK_PARTITION_SCHEME"]
        if skip.contains(where: { c.hasPrefix($0) }) { return false }
        if isWhole && c.hasPrefix("EF57347C") { return false }
        if c.hasPrefix("41504653") { return false }   // an APFS volume lives inside its container; only the container can be read raw
        return true
    }

    var isDiskImage: Bool { (busProtocol ?? "").localizedCaseInsensitiveContains("virtual") || (model ?? "").localizedCaseInsensitiveContains("disk image") }
    var isMounted: Bool { mountPath != nil }

    var title: String {
        if let v = volumeName, !v.isEmpty { return v }
        if isWhole && content.uppercased().hasPrefix("EF57347C") { return "APFS Container \(bsd)" }
        if isWhole {
            let m = (model ?? "").replacingOccurrences(of: " Media", with: "").trimmingCharacters(in: .whitespaces)
            return m.isEmpty ? "Disk \(bsd)" : m
        }
        return DiskNode.friendly(content) ?? "Partition \(bsd)"
    }

    var kindLabel: String {
        if let f = fsKind, !f.isEmpty { return DiskNode.fsName(f) }
        return DiskNode.friendly(content) ?? (isWhole ? "Disk" : "Unknown")
    }

    static func fsName(_ k: String) -> String {
        switch k.lowercased() {
        case "apfs": "APFS"; case "hfs": "HFS+"; case "msdos": "FAT"; case "exfat": "exFAT"
        case "ntfs": "NTFS"; case "udf": "UDF"; case "cd9660": "ISO 9660"; case "smbfs": "SMB"; case "nfs": "NFS"
        default: k.uppercased()
        }
    }

    static func friendly(_ content: String) -> String? {
        switch content {
        case "": nil
        case "EFI", "C12A7328-F81F-11D2-BA4B-00A0C93EC93B": "EFI System Partition"
        case "Apple_APFS", "7C3457EF-0000-11AA-AA11-00306543ECAC": "APFS Container"
        case "41504653-0000-11AA-AA11-00306543ECAC": "APFS Volume"
        case "EF57347C-0000-11AA-AA11-00306543ECAC": "APFS Container Disk"
        case "52637672-7900-11AA-AA11-00306543ECAC": "APFS Recovery"
        case "69646961-6700-11AA-AA11-00306543ECAC": "Apple APFS Preboot"
        case "Apple_APFS_ISC": "APFS Boot Support"
        case "Apple_APFS_Recovery": "APFS Recovery"
        case "Apple_HFS", "48465300-0000-11AA-AA11-00306543ECAC": "HFS+"
        case "Apple_Boot": "Recovery"
        case "Microsoft Basic Data", "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7": "Basic Data"
        case "Microsoft Reserved", "E3C9E316-0B5C-4DB8-817D-F92DF00215AE": "Reserved"
        case "GUID_partition_scheme": "GUID Partition Map"
        case "FDisk_partition_scheme": "MBR Partition Map"
        case "Apple_partition_scheme": "Apple Partition Map"
        case "Linux Filesystem", "0FC63DAF-8483-4772-8E79-3D69D8477DE4": "Linux Filesystem"
        default: content
        }
    }
}

enum DiskDiscovery {
    private static func prop(_ e: io_registry_entry_t, _ key: String) -> Any? {
        IORegistryEntryCreateCFProperty(e, key as CFString, kCFAllocatorDefault, 0)?.takeRetainedValue()
    }

    /// Nearest ancestor IOMedia (partition -> whole disk, synthesized APFS disk -> its container partition).
    private static func parentMedia(of e: io_registry_entry_t) -> String? {
        var cur = e
        IOObjectRetain(cur)
        defer { IOObjectRelease(cur) }
        var p: io_registry_entry_t = 0
        while IORegistryEntryGetParentEntry(cur, kIOServicePlane, &p) == KERN_SUCCESS {
            IOObjectRelease(cur); cur = p
            if IOObjectConformsTo(cur, "IOMedia") != 0, let n = prop(cur, "BSD Name") as? String { return n }
        }
        return nil
    }

    static func snapshot() -> [DiskNode] {
        guard let session = DASessionCreate(kCFAllocatorDefault) else { return [] }
        var iter: io_iterator_t = 0
        guard IOServiceGetMatchingServices(kIOMainPortDefault, IOServiceMatching("IOMedia"), &iter) == KERN_SUCCESS else { return [] }
        defer { IOObjectRelease(iter) }

        var nodes: [String: DiskNode] = [:]
        var parents: [String: String] = [:]
        while case let e = IOIteratorNext(iter), e != 0 {
            defer { IOObjectRelease(e) }
            guard let bsd = prop(e, "BSD Name") as? String else { continue }
            var n = DiskNode(bsd: bsd)
            n.isWhole = (prop(e, "Whole") as? Bool) ?? false
            n.size = (prop(e, "Size") as? NSNumber)?.int64Value ?? 0
            n.content = (prop(e, "Content") as? String) ?? ""
            n.writable = (prop(e, "Writable") as? Bool) ?? true
            if let disk = DADiskCreateFromBSDName(kCFAllocatorDefault, session, bsd),
               let d = DADiskCopyDescription(disk) as? [String: Any] {
                n.volumeName = d[kDADiskDescriptionVolumeNameKey as String] as? String
                if let u = d[kDADiskDescriptionVolumePathKey as String] as? URL { n.mountPath = u.path }
                n.fsKind = d[kDADiskDescriptionVolumeKindKey as String] as? String
                n.model = (d[kDADiskDescriptionDeviceModelKey as String] as? String)?.trimmingCharacters(in: .whitespaces)
                n.busProtocol = d[kDADiskDescriptionDeviceProtocolKey as String] as? String
                n.isInternal = d[kDADiskDescriptionDeviceInternalKey as String] as? Bool
                n.removable = (d[kDADiskDescriptionMediaRemovableKey as String] as? Bool) ?? false
                n.ejectable = (d[kDADiskDescriptionMediaEjectableKey as String] as? Bool) ?? false
                n.isNetwork = (d[kDADiskDescriptionVolumeNetworkKey as String] as? Bool) ?? false
                if let s = d[kDADiskDescriptionMediaSizeKey as String] as? NSNumber, n.size == 0 { n.size = s.int64Value }
            }
            if let m = n.mountPath { fillSpace(&n, mount: m) }
            nodes[bsd] = n
            if let p = parentMedia(of: e) { parents[bsd] = p }
        }

        // Assemble the tree.
        var kids: [String: [String]] = [:]
        for (c, p) in parents where nodes[p] != nil { kids[p, default: []].append(c) }
        func natural(_ a: String, _ b: String) -> Bool { a.compare(b, options: .numeric) == .orderedAscending }
        func build(_ id: String) -> DiskNode {
            var n = nodes[id]!
            n.children = (kids[id] ?? []).sorted(by: natural).map(build).map { c in
                // A sealed system volume shows up as an unmounted volume wrapping its mounted snapshot: show just the snapshot.
                if c.mountPath == nil, c.children.count == 1, let g = c.children.first, g.mountPath != nil, g.volumeName == c.volumeName { return g }
                return c
            }
            // An APFS container's free space is shared by its volumes: show it on the container, never on the physical disk.
            let isRoot = parents[id] == nil || nodes[parents[id]!] == nil
            if n.total == nil, !(isRoot && !n.isDiskImage), let c = n.children.first(where: { $0.free != nil }) {
                n.free = c.free
                n.total = n.size
            }
            return n
        }
        let roots = nodes.keys.filter { parents[$0] == nil || nodes[parents[$0]!] == nil }.sorted(by: natural)
        // Hide Apple's internal Cryptex disk images; user-attached images stay.
        func isSystemImage(_ n: DiskNode) -> Bool {
            func names(_ x: DiskNode) -> [String] { [x.volumeName ?? ""] + x.children.flatMap(names) }
            func mounts(_ x: DiskNode) -> [String] { (x.mountPath.map { [$0] } ?? []) + x.children.flatMap(mounts) }
            return n.isDiskImage && (names(n).contains { $0.localizedCaseInsensitiveContains("cryptex") } || mounts(n).contains { $0.hasPrefix("/System/") })
        }
        return roots.map(build).filter { ($0.size > 0 || !$0.children.isEmpty) && !isSystemImage($0) }
    }

    private static func fillSpace(_ n: inout DiskNode, mount: String) {
        let url = URL(fileURLWithPath: mount)
        let v = try? url.resourceValues(forKeys: [.volumeTotalCapacityKey, .volumeAvailableCapacityForImportantUsageKey, .volumeAvailableCapacityKey])
        if let t = v?.volumeTotalCapacity { n.total = Int64(t) }
        // "Important usage" (what Finder shows) includes purgeable space but reads 0 on system-internal volumes.
        if let f = v?.volumeAvailableCapacityForImportantUsage, f > 0 { n.free = f }
        else if let f = v?.volumeAvailableCapacity { n.free = Int64(f) }
    }
}

/// Keeps the disk list live: refreshes on mount/unmount/rename and periodically (free space changes).
@MainActor
final class DiskMonitor: ObservableObject {
    @Published private(set) var disks: [DiskNode] = []
    @Published private(set) var loaded = false
    private var timer: Timer?
    private var observers: [NSObjectProtocol] = []

    init() {
        refresh()
        let nc = NSWorkspace.shared.notificationCenter
        for name in [NSWorkspace.didMountNotification, NSWorkspace.didUnmountNotification, NSWorkspace.didRenameVolumeNotification] {
            observers.append(nc.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                Task { @MainActor in self?.refresh() }
            })
        }
        timer = Timer.scheduledTimer(withTimeInterval: 15, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.refresh() }
        }
    }

    func refresh() {
        Task.detached(priority: .utility) {
            let s = DiskDiscovery.snapshot()
            await MainActor.run { self.disks = s; self.loaded = true }
        }
    }

    /// Every mounted volume, flattened (for pickers).
    var mountedVolumes: [DiskNode] {
        func flat(_ n: DiskNode) -> [DiskNode] { (n.mountPath != nil ? [n] : []) + n.children.flatMap(flat) }
        return disks.flatMap(flat)
    }
}
