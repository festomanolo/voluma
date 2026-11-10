import Foundation
import AppKit
import UniformTypeIdentifiers
import DiskArbitration

/// Where a Browser reads and writes: the Rust engine (disk images / partitions) or a mounted volume.
protocol FSBackend {
    var id: String { get }
    var isNative: Bool { get }
    func list(_ path: String) throws -> Listing
    /// The file on the host, if it already is one (native volumes); nil for engine-backed images.
    func hostURL(_ path: String) -> URL?
    func exportFile(_ path: String, to dest: URL) throws
    func importFile(from host: URL, to path: String) throws
    func mkdir(_ path: String) throws
    /// Creates a new file holding `contents` (empty by default). Never overwrites.
    func createFile(_ path: String, contents: Data) throws
    func delete(_ path: String) throws
    func rename(_ a: String, _ b: String) throws
}

struct EngineBackend: FSBackend {
    let image: String; let part: Int
    var id: String { "img:\(image):\(part)" }
    var isNative: Bool { false }
    func list(_ p: String) throws -> Listing { try Engine.listDirectory(image: image, part: part, path: p) }
    func hostURL(_ path: String) -> URL? { nil }
    func exportFile(_ path: String, to dest: URL) throws { _ = try Engine.exportFile(image: image, part: part, path: path, dest: dest.path) }
    func importFile(from host: URL, to path: String) throws { try Engine.mutate(image: image, part: part, op: "import", a: path, b: host.path) }
    func mkdir(_ path: String) throws { try Engine.mutate(image: image, part: part, op: "mkdir", a: path) }
    func createFile(_ path: String, contents: Data) throws { try createViaImport(self, path, contents) }
    func delete(_ path: String) throws { try Engine.mutate(image: image, part: part, op: "delete", a: path) }
    func rename(_ a: String, _ b: String) throws { try Engine.mutate(image: image, part: part, op: "rename", a: a, b: b) }
}

struct NativeBackend: FSBackend {
    let root: String
    var id: String { "vol:\(root)" }
    var isNative: Bool { true }
    private func url(_ p: String) -> URL { p == "/" ? URL(fileURLWithPath: root) : URL(fileURLWithPath: root).appendingPathComponent(String(p.dropFirst())) }

    func list(_ p: String) throws -> Listing {
        let keys: [URLResourceKey] = [.isDirectoryKey, .fileSizeKey, .totalFileAllocatedSizeKey, .contentModificationDateKey, .isHiddenKey, .isPackageKey]
        let items = try FileManager.default.contentsOfDirectory(at: url(p), includingPropertiesForKeys: keys, options: [])
        let entries: [Listing.Entry] = items.map { u in
            let v = try? u.resourceValues(forKeys: Set(keys))
            let dir = v?.isDirectory == true
            return Listing.Entry(name: u.lastPathComponent, kind: dir ? "directory" : "file",
                                 size: dir ? 0 : UInt64(max(0, v?.fileSize ?? 0)),
                                 modified: v?.contentModificationDate.map { Int64($0.timeIntervalSince1970) },
                                 hidden: v?.isHidden == true || u.lastPathComponent.hasPrefix("."))
        }
        return Listing(filesystem: "Mounted volume", label: nil, entries: entries,
                       capabilities: .init(read: true, write: true), driver_supports_write: true, write_blocked: nil)
    }
    func hostURL(_ path: String) -> URL? { url(path) }
    func exportFile(_ path: String, to dest: URL) throws { try FileManager.default.copyItem(at: url(path), to: dest) }
    func importFile(from host: URL, to path: String) throws { try FileManager.default.copyItem(at: host, to: url(path)) }   // never overwrites
    func mkdir(_ path: String) throws { try FileManager.default.createDirectory(at: url(path), withIntermediateDirectories: false) }
    func delete(_ path: String) throws { try FileManager.default.trashItem(at: url(path), resultingItemURL: nil) }   // Trash, never a hard delete
    func rename(_ a: String, _ b: String) throws { try FileManager.default.moveItem(at: url(a), to: url(b)) }
}

func joinPath(_ a: String, _ b: String) -> String { a == "/" ? "/\(b)" : "\(a)/\(b)" }

/// Recursive copy between any two backends (or from host files). Never overwrites: a clash gets " copy" / " copy N".
enum Copier {
    struct CopyError: LocalizedError { let message: String; var errorDescription: String? { message } }

    static func uniqueName(_ name: String, taken: Set<String>) -> String {
        if !taken.contains(name.lowercased()) { return name }
        let ns = name as NSString
        let (base, ext) = (ns.deletingPathExtension, ns.pathExtension)
        var n = 1
        while true {
            let cand = (n == 1 ? "\(base) copy" : "\(base) copy \(n)") + (ext.isEmpty ? "" : ".\(ext)")
            if !taken.contains(cand.lowercased()) { return cand }
            n += 1
        }
    }

    static func takenNames(_ b: FSBackend, _ dir: String) -> Set<String> {
        Set(((try? b.list(dir))?.entries ?? []).map { $0.name.lowercased() })
    }

    @discardableResult
    static func copy(src: FSBackend, srcPath: String, entry: Listing.Entry, dst: FSBackend, dstDir: String,
                     taken: inout Set<String>, progress: (String) -> Void) throws -> Int {
        if src.id == dst.id && entry.isDir && (dstDir == srcPath || dstDir.hasPrefix(srcPath + "/")) {
            throw CopyError(message: "Can’t copy “\(entry.name)” into itself")
        }
        let name = uniqueName(entry.name, taken: taken)
        taken.insert(name.lowercased())
        let dpath = joinPath(dstDir, name)
        progress(entry.name)
        if entry.isDir {
            try dst.mkdir(dpath)
            var inner = Set<String>()
            var n = 1
            for c in try src.list(srcPath).entries { n += try copy(src: src, srcPath: joinPath(srcPath, c.name), entry: c, dst: dst, dstDir: dpath, taken: &inner, progress: progress) }
            return n
        }
        if let u = src.hostURL(srcPath) { try dst.importFile(from: u, to: dpath); return 1 }
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("voluma-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: tmp) }
        try src.exportFile(srcPath, to: tmp)
        try dst.importFile(from: tmp, to: dpath)
        return 1
    }

    @discardableResult
    static func copyHost(_ url: URL, dst: FSBackend, dstDir: String, taken: inout Set<String>, progress: (String) -> Void) throws -> Int {
        let name = uniqueName(url.lastPathComponent, taken: taken)
        taken.insert(name.lowercased())
        let dpath = joinPath(dstDir, name)
        progress(url.lastPathComponent)
        var isDir: ObjCBool = false
        FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir)
        if isDir.boolValue {
            try dst.mkdir(dpath)
            var inner = Set<String>(), n = 1
            for c in try FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: nil) {
                n += try copyHost(c, dst: dst, dstDir: dpath, taken: &inner, progress: progress)
            }
            return n
        }
        try dst.importFile(from: url, to: dpath)
        return 1
    }
}

// MARK: - clipboard

/// The in-app clipboard. A token on the system pasteboard says "the copy is ours", so a Paste can stream
/// backend-to-backend (no temp files); anything else on the pasteboard is treated as files from Finder.
final class Clip: @unchecked Sendable {
    static let shared = Clip()
    static let type = NSPasteboard.PasteboardType("com.voluma.clipboard-token")
    var token = ""
    var backend: FSBackend?
    var items: [(path: String, entry: Listing.Entry)] = []
    var promises: [PromiseDelegate] = []   // keep file-promise delegates alive while on the pasteboard

    /// Puts the selection on the system pasteboard (file URLs for native volumes, lazy file promises for
    /// images, so Finder can paste both) and remembers it for in-app paste.
    func copy(backend: FSBackend, items: [(path: String, entry: Listing.Entry)]) {
        self.backend = backend; self.items = items; token = UUID().uuidString; promises = []
        let pb = NSPasteboard.general
        pb.clearContents()
        if backend.isNative {
            pb.writeObjects(items.compactMap { backend.hostURL($0.path) as NSURL? })
        } else {
            var providers: [NSPasteboardWriting] = []
            for it in items {
                let d = PromiseDelegate(backend: backend, path: it.path, entry: it.entry)
                promises.append(d)
                let p = NSFilePromiseProvider(fileType: it.entry.isDir ? UTType.folder.identifier : UTType.data.identifier, delegate: d)
                providers.append(p)
            }
            pb.writeObjects(providers)
        }
        pb.addTypes([Clip.type], owner: nil)
        pb.setString(token, forType: Clip.type)
    }

    /// What a Paste should copy: our own copy if the pasteboard still carries our token, else file URLs.
    enum Source { case clip(FSBackend, [(path: String, entry: Listing.Entry)]), files([URL]), none }
    func source() -> Source {
        let pb = NSPasteboard.general
        if let t = pb.string(forType: Clip.type), t == token, let b = backend, !items.isEmpty { return .clip(b, items) }
        let urls = (pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
        return urls.isEmpty ? .none : .files(urls)
    }
}

final class PromiseDelegate: NSObject, NSFilePromiseProviderDelegate {
    let backend: FSBackend; let path: String; let entry: Listing.Entry
    init(backend: FSBackend, path: String, entry: Listing.Entry) { self.backend = backend; self.path = path; self.entry = entry }
    func filePromiseProvider(_ p: NSFilePromiseProvider, fileNameForType fileType: String) -> String { entry.name }
    func operationQueue(for p: NSFilePromiseProvider) -> OperationQueue { let q = OperationQueue(); q.maxConcurrentOperationCount = 1; return q }
    func filePromiseProvider(_ p: NSFilePromiseProvider, writePromiseTo url: URL, completionHandler: @escaping (Error?) -> Void) {
        do {
            // Finder gives us the exact destination; copy into its parent keeping that name.
            let parent = NativeBackend(root: url.deletingLastPathComponent().path)
            var taken = Set<String>()
            var e = entry; e = Listing.Entry(name: url.lastPathComponent, kind: entry.kind, size: entry.size, modified: entry.modified, hidden: entry.hidden)
            try Copier.copy(src: backend, srcPath: path, entry: e, dst: parent, dstDir: "/", taken: &taken, progress: { _ in })
            completionHandler(nil)
        } catch { completionHandler(error) }
    }
}

// MARK: - eject

/// Unmounts every volume of a whole disk and ejects it (DiskArbitration; also detaches disk images).
enum Ejector {
    private final class Box { let session: DASession; let done: (String?) -> Void
        init(_ s: DASession, _ d: @escaping (String?) -> Void) { session = s; done = d } }

    private static func text(_ d: DADissenter) -> String {
        (DADissenterGetStatusString(d) as String?) ?? "error \(DADissenterGetStatus(d))"
    }

    /// Unmounts one volume (not the whole disk). Returns an error message, or nil on success.
    static func unmount(bsd: String) async -> String? {
        await withCheckedContinuation { cont in
            guard let session = DASessionCreate(kCFAllocatorDefault),
                  let disk = DADiskCreateFromBSDName(kCFAllocatorDefault, session, bsd) else { cont.resume(returning: "Couldn’t open \(bsd)"); return }
            DASessionSetDispatchQueue(session, DispatchQueue.main)
            let box = Unmanaged.passRetained(Box(session) { cont.resume(returning: $0) }).toOpaque()
            DADiskUnmount(disk, DADiskUnmountOptions(kDADiskUnmountOptionDefault), { _, dissenter, ctx in
                let b = Unmanaged<Box>.fromOpaque(ctx!).takeRetainedValue()
                b.done(dissenter.map { Ejector.text($0) })
            }, box)
        }
    }

    /// Mounts a volume again. Returns an error message, or nil on success.
    static func mount(bsd: String) async -> String? {
        await withCheckedContinuation { cont in
            guard let session = DASessionCreate(kCFAllocatorDefault),
                  let disk = DADiskCreateFromBSDName(kCFAllocatorDefault, session, bsd) else { cont.resume(returning: "Couldn’t open \(bsd)"); return }
            DASessionSetDispatchQueue(session, DispatchQueue.main)
            let box = Unmanaged.passRetained(Box(session) { cont.resume(returning: $0) }).toOpaque()
            DADiskMount(disk, nil, DADiskMountOptions(kDADiskMountOptionDefault), { _, dissenter, ctx in
                let b = Unmanaged<Box>.fromOpaque(ctx!).takeRetainedValue()
                b.done(dissenter.map { Ejector.text($0) })
            }, box)
        }
    }

    static func eject(bsd: String, completion: @escaping (String?) -> Void) {
        guard let session = DASessionCreate(kCFAllocatorDefault),
              let disk = DADiskCreateFromBSDName(kCFAllocatorDefault, session, bsd) else { completion("Couldn’t open \(bsd)"); return }
        DASessionSetDispatchQueue(session, DispatchQueue.main)
        let box = Unmanaged.passRetained(Box(session, completion)).toOpaque()
        DADiskUnmount(disk, DADiskUnmountOptions(kDADiskUnmountOptionWhole), { disk, dissenter, ctx in
            let b = Unmanaged<Box>.fromOpaque(ctx!)
            if let d = dissenter { let m = Ejector.text(d); b.takeRetainedValue().done("Couldn’t unmount: \(m)"); return }
            DADiskEject(disk, DADiskEjectOptions(kDADiskEjectOptionDefault), { _, dissenter, ctx in
                let box = Unmanaged<Box>.fromOpaque(ctx!).takeRetainedValue()
                box.done(dissenter.map { "Couldn’t eject: \(Ejector.text($0))" })
            }, ctx)
        }, box)
    }
}
