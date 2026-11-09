import Foundation
import SwiftUI

enum FileCategory: Int, CaseIterable, Identifiable {
    case apps, documents, images, video, audio, archives, code, system, other
    var id: Int { rawValue }
    var label: String {
        ["Applications", "Documents", "Images", "Video", "Audio", "Archives", "Developer", "System & Library", "Other"][rawValue]
    }
    var color: Color {
        [Color.blue, .orange, .green, .purple, .pink, .brown, .teal, .gray, .secondary.opacity(0.6)][rawValue]
    }
    var symbol: String {
        ["app", "doc.text", "photo", "film", "music.note", "archivebox", "chevron.left.forwardslash.chevron.right", "gearshape", "questionmark.folder"][rawValue]
    }

    static func of(extension e: String) -> FileCategory {
        switch e.lowercased() {
        case "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "pages", "numbers", "key", "txt", "rtf", "md", "csv", "epub", "odt", "ods": .documents
        case "jpg", "jpeg", "png", "gif", "heic", "heif", "tif", "tiff", "bmp", "webp", "raw", "cr2", "nef", "arw", "dng", "svg", "psd", "ai", "icns": .images
        case "mov", "mp4", "m4v", "avi", "mkv", "wmv", "webm", "mpg", "mpeg", "prores": .video
        case "mp3", "m4a", "aac", "wav", "aiff", "flac", "ogg", "caf", "aif": .audio
        case "zip", "gz", "tgz", "bz2", "xz", "7z", "rar", "tar", "dmg", "iso", "pkg", "img", "sparsebundle": .archives
        case "swift", "rs", "c", "h", "cpp", "m", "mm", "py", "js", "ts", "tsx", "jsx", "java", "kt", "go", "rb", "php", "html", "css", "json", "xml", "yml", "yaml", "sh", "o", "a", "dylib", "rlib", "class", "jar", "pyc", "swiftmodule", "d", "wasm": .code
        case "plist", "db", "sqlite", "log", "cache", "dat", "bin", "tmp", "kext", "framework", "car", "nib", "strings", "lproj": .system
        default: .other
        }
    }
}

/// A retained file. Kept tiny: its URL is rebuilt from the owning folder when needed.
struct FileItem: Identifiable, Hashable {
    let name: String
    let size: Int64
    let category: FileCategory
    var id: String { name }
}

/// A file with its full location (used for the global "largest files" list only).
struct LocatedFile: Identifiable, Hashable {
    let id = UUID()
    let item: FileItem
    let url: URL
}

/// A scanned directory with aggregated sizes. Only the largest files per folder are retained (bounded memory).
final class FolderNode: Identifiable {
    let name: String
    var size: Int64 = 0                 // aggregated, on-disk bytes
    var itemCount = 0                   // aggregated file count
    var cats = [Int64](repeating: 0, count: FileCategory.allCases.count)
    var children: [FolderNode] = []
    var files: [FileItem] = []          // largest files directly in this folder
    var omittedFiles = 0
    var omittedBytes: Int64 = 0
    var unreadable = false
    weak var parent: FolderNode?
    let rootURL: URL?                   // only set on the scan root
    var id: ObjectIdentifier { ObjectIdentifier(self) }
    init(name: String, rootURL: URL? = nil) { self.name = name; self.rootURL = rootURL }

    var url: URL {
        if let r = rootURL { return r }
        var parts: [String] = []
        var n: FolderNode? = self
        while let x = n, x.rootURL == nil { parts.append(x.name); n = x.parent }
        var u = n?.rootURL ?? URL(fileURLWithPath: "/")
        for p in parts.reversed() { u.appendPathComponent(p, isDirectory: true) }
        return u
    }
    func url(of f: FileItem) -> URL { url.appendingPathComponent(f.name) }
}

struct ScanResult {
    let root: FolderNode
    let largest: [LocatedFile]
    let unreadableFolders: Int
    let seconds: Double
    let cancelled: Bool
}

/// Thread-safe progress counters polled by the UI.
final class ScanProgress: @unchecked Sendable {
    private let lock = NSLock()
    private var seenLinks = Set<AnyHashable>()
    /// True the first time a hard-linked file identity is seen (so each is counted once, like `du`).
    func firstSighting(_ id: AnyHashable) -> Bool { lock.lock(); defer { lock.unlock() }; return seenLinks.insert(id).inserted }
    private var _files = 0, _bytes: Int64 = 0, _current = "", _cancelled = false, _unreadable = 0
    func addBatch(files: Int, bytes: Int64) { lock.lock(); _files += files; _bytes += bytes; lock.unlock() }
    func setCurrent(_ s: String) { lock.lock(); _current = s; lock.unlock() }
    func addUnreadable() { lock.lock(); _unreadable += 1; lock.unlock() }
    func cancel() { lock.lock(); _cancelled = true; lock.unlock() }
    var cancelled: Bool { lock.lock(); defer { lock.unlock() }; return _cancelled }
    var snapshot: (files: Int, bytes: Int64, current: String, unreadable: Int) {
        lock.lock(); defer { lock.unlock() }; return (_files, _bytes, _current, _unreadable)
    }
}

/// Parallel work-queue scanner. Every directory is an independent task pulled by a pool of threads
/// (so parallelism holds at every depth, not just near the root), and each directory is read with a
/// single `getattrlistbulk` call that returns name, type, device, file id, link count and allocated
/// size for all of its entries at once.
enum StorageScanner {
    static let keptFilesPerFolder = 100
    private static let skipUnderRoot: Set<String> = ["/System/Volumes", "/Volumes", "/dev", "/.vol", "/.Spotlight-V100", "/.fseventsd", "/private/var/vm", "/Network", "/cores"]

    private struct Task { let node: FolderNode; let path: String; let forced: FileCategory?; let systemish: Bool }

    private final class Pool {
        let cond = NSCondition()
        var stack: [Task] = []
        var active = 0
        func push(_ t: Task) { cond.lock(); stack.append(t); cond.signal(); cond.unlock() }
        func next() -> Task? {
            cond.lock(); defer { cond.unlock() }
            while stack.isEmpty {
                if active == 0 { cond.broadcast(); return nil }
                cond.wait()
            }
            active += 1
            return stack.removeLast()
        }
        func done() {
            cond.lock(); active -= 1
            if stack.isEmpty && active == 0 { cond.broadcast() }
            cond.unlock()
        }
    }

    static func scan(root: URL, progress: ScanProgress) -> ScanResult {
        let t0 = Date()
        let rootPath = root.path
        let node = FolderNode(name: root.lastPathComponent, rootURL: root)
        var st = stat()
        let rootDev: UInt32 = stat(rootPath, &st) == 0 ? UInt32(truncatingIfNeeded: st.st_dev) : 0
        let systemRoot = rootPath == "/"

        let pool = Pool()
        pool.push(Task(node: node, path: rootPath, forced: nil, systemish: isSystemish(rootPath)))
        let workers = min(48, max(8, ProcessInfo.processInfo.activeProcessorCount * 4))
        let group = DispatchGroup()
        for _ in 0..<workers {
            group.enter()
            let th = Thread {
                let buf = UnsafeMutableRawPointer.allocate(byteCount: 256 * 1024, alignment: 8)
                defer { buf.deallocate() }
                var n = 0
                while let t = pool.next() {
                    if !progress.cancelled { scanDir(t, rootDev: rootDev, systemRoot: systemRoot, pool: pool, progress: progress, buf: buf, tick: &n) }
                    pool.done()
                }
                group.leave()
            }
            th.stackSize = 1 << 20
            th.start()
        }
        group.wait()

        finalize(node)
        var all: [(FolderNode, FileItem)] = []
        func gather(_ n: FolderNode) { all.append(contentsOf: n.files.prefix(30).map { (n, $0) }); n.children.forEach(gather) }
        gather(node)
        all.sort { $0.1.size > $1.1.size }
        let largest = all.prefix(200).map { LocatedFile(item: $0.1, url: $0.0.url(of: $0.1)) }
        func unread(_ n: FolderNode) -> Int { (n.unreadable ? 1 : 0) + n.children.reduce(0) { $0 + unread($1) } }
        return ScanResult(root: node, largest: largest, unreadableFolders: unread(node),
                          seconds: Date().timeIntervalSince(t0), cancelled: progress.cancelled)
    }

    /// Bottom-up aggregation of sizes/counts/categories once all workers are done.
    private static func finalize(_ n: FolderNode) {
        for c in n.children {
            finalize(c)
            n.size += c.size; n.itemCount += c.itemCount
            for i in 0..<n.cats.count { n.cats[i] += c.cats[i] }
        }
        n.children.sort { $0.size > $1.size }
    }

    private static func isSystemish(_ p: String) -> Bool {
        p.contains("/Library/") || p.hasPrefix("/System") || p.hasPrefix("/private") || p.hasPrefix("/usr") || p.hasSuffix("/Library")
    }

    @inline(__always) private static func ld<T>(_ b: UnsafeRawPointer, _ o: Int, _ t: T.Type) -> T { b.loadUnaligned(fromByteOffset: o, as: T.self) }

    private static func scanDir(_ t: Task, rootDev: UInt32, systemRoot: Bool, pool: Pool, progress: ScanProgress,
                                buf: UnsafeMutableRawPointer, tick: inout Int) {
        tick += 1
        if tick & 63 == 1 { progress.setCurrent(t.path) }
        let fd = open(t.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
        if fd < 0 { t.node.unreadable = true; progress.addUnreadable(); return }
        defer { close(fd) }

        var al = attrlist()
        al.bitmapcount = u_short(ATTR_BIT_MAP_COUNT)
        al.commonattr = ATTR_CMN_RETURNED_ATTRS | UInt32(ATTR_CMN_NAME | ATTR_CMN_DEVID | ATTR_CMN_OBJTYPE | ATTR_CMN_FILEID | ATTR_CMN_ERROR)
        al.fileattr = attrgroup_t(ATTR_FILE_LINKCOUNT | ATTR_FILE_ALLOCSIZE)

        var files: [FileItem] = []
        var filesBytes: Int64 = 0, filesCount = 0
        var localBytes: Int64 = 0
        var sawError = false

        loop: while true {
            let n = getattrlistbulk(fd, &al, buf, 256 * 1024, 0)
            if n < 0 { sawError = true; break }
            if n == 0 { break }
            var off = 0
            for _ in 0..<Int(n) {
                let base = off
                let len = Int(ld(buf, base, UInt32.self))
                off = base + len
                var p = base + 4
                let rcCommon = ld(buf, p, UInt32.self), rcFile = ld(buf, p + 12, UInt32.self)
                p += 20
                if rcCommon & UInt32(ATTR_CMN_ERROR) != 0 { let e = ld(buf, p, UInt32.self); p += 4; if e != 0 { sawError = true; continue } }
                var name = ""
                if rcCommon & UInt32(ATTR_CMN_NAME) != 0 {
                    let ref = ld(buf, p, Int32.self)
                    name = String(cString: (buf + p + Int(ref)).assumingMemoryBound(to: CChar.self))
                    p += 8
                }
                var dev: UInt32 = 0, objtype: UInt32 = 0, fileid: UInt64 = 0
                if rcCommon & UInt32(ATTR_CMN_DEVID) != 0 { dev = ld(buf, p, UInt32.self); p += 4 }
                if rcCommon & UInt32(ATTR_CMN_OBJTYPE) != 0 { objtype = ld(buf, p, UInt32.self); p += 4 }
                if rcCommon & UInt32(ATTR_CMN_FILEID) != 0 { fileid = ld(buf, p, UInt64.self); p += 8 }
                var links: UInt32 = 1, alloc: Int64 = 0
                if rcFile & UInt32(ATTR_FILE_LINKCOUNT) != 0 { links = ld(buf, p, UInt32.self); p += 4 }
                if rcFile & UInt32(ATTR_FILE_ALLOCSIZE) != 0 { alloc = ld(buf, p, Int64.self); p += 8 }

                switch objtype {
                case UInt32(VDIR.rawValue):
                    let child = t.path == "/" ? "/" + name : t.path + "/" + name
                    if systemRoot && skipUnderRoot.contains(child) { continue }
                    if !systemRoot && rootDev != 0 && dev != rootDev { continue }   // other mounted volume
                    let c = FolderNode(name: name)
                    c.parent = t.node
                    t.node.children.append(c)
                    let forced = t.forced ?? (name.lowercased().hasSuffix(".app") ? FileCategory.apps : nil)
                    pool.push(Task(node: c, path: child, forced: forced, systemish: t.systemish || name == "Library"))
                case UInt32(VREG.rawValue):
                    var size = alloc
                    if links > 1 && !progress.firstSighting(AnyHashable(UInt64(dev) << 56 ^ fileid)) { size = 0 }
                    let cat = t.forced ?? category(name, systemish: t.systemish)
                    t.node.cats[cat.rawValue] += size
                    filesBytes += size; filesCount += 1; localBytes += size
                    files.append(FileItem(name: name, size: size, category: cat))
                default: break   // symlinks, devices, sockets
                }
            }
        }
        if sawError && filesCount == 0 && t.node.children.isEmpty { t.node.unreadable = true; progress.addUnreadable() }
        t.node.size += filesBytes; t.node.itemCount += filesCount
        if files.count > keptFilesPerFolder {
            files.sort { $0.size > $1.size }
            t.node.omittedFiles = files.count - keptFilesPerFolder
            t.node.omittedBytes = files[keptFilesPerFolder...].reduce(0) { $0 + $1.size }
            files.removeSubrange(keptFilesPerFolder...)
        } else { files.sort { $0.size > $1.size } }
        t.node.files = files
        progress.addBatch(files: filesCount, bytes: localBytes)
    }

    private static func category(_ name: String, systemish: Bool) -> FileCategory {
        let ext = name.lastIndex(of: ".").map { String(name[name.index(after: $0)...]) } ?? ""
        let c = FileCategory.of(extension: ext)
        return systemish && c == .other ? .system : c
    }
}

@MainActor
final class LensModel: ObservableObject {
    @Published var target: URL?
    @Published var targetTitle = ""
    @Published var volume: DiskNode?
    @Published var scanning = false
    @Published var result: ScanResult?
    @Published var current: FolderNode?
    @Published var live: (files: Int, bytes: Int64, current: String, unreadable: Int) = (0, 0, "", 0)
    private var progress: ScanProgress?
    private var poll: Timer?

    func start(url: URL, title: String, volume: DiskNode?) {
        cancel()
        target = url; targetTitle = title; self.volume = volume
        result = nil; current = nil; scanning = true
        let p = ScanProgress(); progress = p
        poll = Timer.scheduledTimer(withTimeInterval: 0.15, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.live = p.snapshot }
        }
        Task.detached(priority: .userInitiated) {
            let r = StorageScanner.scan(root: url, progress: p)
            await MainActor.run {
                guard self.progress === p else { return }
                self.poll?.invalidate(); self.poll = nil
                self.live = p.snapshot
                self.result = r; self.current = r.root; self.scanning = false
            }
        }
    }

    func cancel() { progress?.cancel(); poll?.invalidate(); poll = nil; progress = nil; scanning = false }
}
