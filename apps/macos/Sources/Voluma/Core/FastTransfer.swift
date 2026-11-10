import Foundation
import Darwin

/// Hyper-fast, parallel copy / move for files on volumes macOS mounts natively.
///
/// How it gets its speed:
///  * **Move on one volume is a rename**: instant, whatever the size (`renamex_np`, never overwrites).
///  * **Copy on one APFS volume is a clone** (`clonefile`): instant and takes no extra space.
///  * Everything else is **copied in parallel**: a worker pool (2× the cores) drains a queue that is filled
///    while the folder tree is still being walked, so thousands of small files overlap their latency.
///  * **Big files are split into 8 MB chunks** that every worker copies at once with `pread`/`pwrite`.
///  * A cross-volume **move** deletes each source file the moment its copy lands (no double space needed).
///  * Nothing is ever overwritten: a name clash becomes "name copy", "name copy 2" …
final class TransferProgress: @unchecked Sendable {
    struct Snapshot { var bytesDone: Int64; var bytesTotal: Int64; var filesDone: Int; var filesTotal: Int
        var scanning: Bool; var current: String; var errors: Int; var firstError: String?; var finished: Bool }
    private let lock = NSLock()
    private var s = Snapshot(bytesDone: 0, bytesTotal: 0, filesDone: 0, filesTotal: 0, scanning: true, current: "", errors: 0, firstError: nil, finished: false)
    private var _cancelled = false
    let started = Date()

    var snapshot: Snapshot { lock.lock(); defer { lock.unlock() }; return s }
    var cancelled: Bool { lock.lock(); defer { lock.unlock() }; return _cancelled }
    func cancel() { lock.lock(); _cancelled = true; lock.unlock() }

    func discovered(bytes: Int64) { lock.lock(); s.bytesTotal += bytes; s.filesTotal += 1; lock.unlock() }
    func addBytes(_ n: Int64) { lock.lock(); s.bytesDone += n; lock.unlock() }
    func fileDone(name: String) { lock.lock(); s.filesDone += 1; s.current = name; lock.unlock() }
    func scanDone() { lock.lock(); s.scanning = false; lock.unlock() }
    func finish() { lock.lock(); s.finished = true; s.scanning = false; lock.unlock() }
    func fail(_ msg: String) { lock.lock(); s.errors += 1; if s.firstError == nil { s.firstError = msg }; lock.unlock() }
}

enum FastTransfer {
    enum Mode { case copy, move }
    struct Result { var items = 0; var files = 0; var bytes: Int64 = 0; var errors = 0; var firstError: String?
        var renamed = 0; var cloned = 0; var cancelled = false; var seconds = 0.0 }

    static let chunkSize = 8 << 20          // 8 MB per parallel chunk
    static let bigFile: Int64 = 24 << 20    // files at or above this are split into chunks

    // MARK: public entry

    /// Copies or moves `sources` into the folder `dstDir`. Blocking: call it off the main thread.
    /// `taken` is the set of (lower-cased) names already in `dstDir`; the result is the destination URL of each item.
    @discardableResult
    static func run(_ sources: [URL], into dstDir: URL, mode: Mode, taken: Set<String>? = nil,
                    progress: TransferProgress, placed: ((URL) -> Void)? = nil) -> Result {
        let t0 = Date()
        var res = Result()
        var names = taken ?? Set(((try? FileManager.default.contentsOfDirectory(atPath: dstDir.path)) ?? []).map { $0.lowercased() })
        let pool = Pool(progress: progress, mode: mode)
        pool.start()
        var dstDev: dev_t = 0
        var st = stat(); if stat(dstDir.path, &st) == 0 { dstDev = st.st_dev }
        
        for src in sources {
            if progress.cancelled { break }
            let name = NameClash.unique(src.lastPathComponent, taken: names)
            names.insert(name.lowercased())
            let dst = dstDir.appendingPathComponent(name)
            res.items += 1

            // copying a folder into itself would never finish
            if src.hasDirectoryPath || isDir(src.path) {
                let sp = src.standardizedFileURL.path, dp = dstDir.standardizedFileURL.path
                if dp == sp || dp.hasPrefix(sp + "/") { pool.noteError("Can’t copy “\(src.lastPathComponent)” into itself"); continue }
            }

            if mode == .move {
                var ss = stat()
                if lstat(src.path, &ss) == 0, ss.st_dev == dstDev {
                    if renamex_np(src.path, dst.path, UInt32(RENAME_EXCL)) == 0 {   // same volume: instant
                        res.renamed += 1; progress.discovered(bytes: 0); progress.fileDone(name: name); placed?(dst); continue
                    }
                }
            }
            var ss2 = stat()
            guard lstat(src.path, &ss2) == 0 else { pool.noteError("\(src.lastPathComponent): \(String(cString: strerror(errno)))"); continue }
            walk(src.path, dst.path, ss2, pool, progress)
            placed?(dst)
        }
        progress.scanDone()
        pool.finishAndWait()

        if mode == .move && !progress.cancelled {
            // remove the now-empty source folders, deepest first; rmdir leaves any folder that still holds a failed file
            pool.moveDirs.reversed().forEach { _ = rmdir($0) }
        }
        let snap = progress.snapshot
        res.files = snap.filesDone; res.bytes = snap.bytesDone; res.errors += snap.errors
        res.firstError = snap.firstError; res.cloned = pool.cloned; res.cancelled = progress.cancelled
        res.seconds = Date().timeIntervalSince(t0)
        progress.finish()
        return res
    }

    private static func isDir(_ p: String) -> Bool { var s = stat(); return lstat(p, &s) == 0 && (s.st_mode & S_IFMT) == S_IFDIR }

    // MARK: tree walk (producer)

    private static func walk(_ src: String, _ dst: String, _ st: stat, _ pool: Pool, _ progress: TransferProgress) {
        if progress.cancelled { return }
        let kind = st.st_mode & S_IFMT
        if kind == S_IFDIR {
            // a folder on another volume is a mount point inside the tree; copy it like any folder
            if mkdir(dst, st.st_mode & 0o7777 | 0o700) != 0 && errno != EEXIST { pool.noteError("\(lastComp(dst)): \(String(cString: strerror(errno)))"); return }
            pool.addDir(src, dst, st)
            guard let d = opendir(src) else { pool.noteError("\(lastComp(src)): \(String(cString: strerror(errno)))"); return }
            var kids: [(String, stat)] = []
            while let e = readdir(d) {
                let name = withUnsafePointer(to: &e.pointee.d_name) { $0.withMemoryRebound(to: CChar.self, capacity: Int(e.pointee.d_namlen) + 1) { String(cString: $0) } }
                if name == "." || name == ".." { continue }
                var cs = stat()
                if fstatat(dirfd(d), name, &cs, AT_SYMLINK_NOFOLLOW) == 0 { kids.append((name, cs)) }
            }
            closedir(d)
            // FAT/exFAT/NTFS/SMB keep xattrs in "._name" sidecar files. copyfile folds them back into the real file's
            // xattrs, so copying the sidecar as well would litter the destination with junk.
            let sidecars = hasSidecars(src) ? Set(kids.map { $0.0 }) : []
            for (n, cs) in kids {
                if progress.cancelled { return }
                if n.hasPrefix("._"), sidecars.contains(String(n.dropFirst(2))) {
                    if pool.mode == .move { unlink(src + "/" + n) }
                    continue
                }
                if (cs.st_mode & S_IFMT) == S_IFDIR {
                    walk(src + "/" + n, dst + "/" + n, cs, pool, progress)
                } else {
                    pool.submit(.init(src: src + "/" + n, dst: dst + "/" + n, size: Int64(cs.st_size), mode: cs.st_mode))
                }
            }
        } else {
            pool.submit(.init(src: src, dst: dst, size: Int64(st.st_size), mode: st.st_mode))
        }
    }

    /// True for filesystems that store extended attributes in AppleDouble "._" sidecar files.
    private static func hasSidecars(_ path: String) -> Bool {
        var fs = statfs()
        guard statfs(path, &fs) == 0 else { return false }
        let t = withUnsafePointer(to: &fs.f_fstypename) { $0.withMemoryRebound(to: CChar.self, capacity: Int(MFSTYPENAMELEN)) { String(cString: $0) } }
        return ["msdos", "exfat", "ntfs", "smbfs", "afpfs", "webdav", "nfs", "ufsd_NTFS"].contains(t)
    }

    private static func lastComp(_ p: String) -> String { (p as NSString).lastPathComponent }

    // MARK: worker pool

    struct Job { let src: String; let dst: String; let size: Int64; let mode: mode_t }

    /// A big file being copied by many workers at once.
    final class BigFile {
        let job: Job; let srcFD: Int32; let dstFD: Int32; let chunks: Int
        var next = 0, finished = 0, failed = false
        init(job: Job, srcFD: Int32, dstFD: Int32) {
            self.job = job; self.srcFD = srcFD; self.dstFD = dstFD
            chunks = Int((job.size + Int64(FastTransfer.chunkSize) - 1) / Int64(FastTransfer.chunkSize))
        }
    }

    final class Pool: @unchecked Sendable {
        let progress: TransferProgress; let mode: Mode
        private let cond = NSCondition()
        private var small: [Job] = []
        private var big: [BigFile] = []
        private var closed = false
        private var active = 0
        private var threads: [Thread] = []
        private var dirs: [(String, String, stat)] = []   // (src, dst, st): metadata applied after the contents
        var cloned = 0
        var moveDirs: [String] { cond.lock(); defer { cond.unlock() }; return dirs.map { $0.0 } }
        private let maxQueue = 8192

        init(progress: TransferProgress, mode: Mode) { self.progress = progress; self.mode = mode }

        func noteError(_ m: String) { progress.fail(m) }
        func addDir(_ s: String, _ d: String, _ st: stat) { cond.lock(); dirs.append((s, d, st)); cond.unlock() }

        func start() {
            let n = min(max(ProcessInfo.processInfo.activeProcessorCount * 2, 8), 24)
            for i in 0..<n {
                let t = Thread { [self] in worker() }
                t.name = "voluma-copy-\(i)"; t.qualityOfService = .userInitiated; t.stackSize = 512 << 10
                threads.append(t); t.start()
            }
        }

        func submit(_ j: Job) {
            progress.discovered(bytes: j.size)
            cond.lock()
            while small.count + big.count >= maxQueue && !progress.cancelled { cond.wait() }   // back-pressure on the walker
            small.append(j)
            cond.broadcast()
            cond.unlock()
        }

        func finishAndWait() {
            cond.lock(); closed = true; cond.broadcast()
            while !(small.isEmpty && big.isEmpty && active == 0) { cond.wait() }
            cond.unlock()
            // folder metadata (mode, times) last, so writing into them did not disturb it
            cond.lock(); let ds = dirs; cond.unlock()
            for (s, d, st) in ds.reversed() { applyDirMeta(s, d, st) }
        }

        private enum Work { case file(Job), chunk(BigFile, Int), done }

        private func take() -> Work {
            cond.lock(); defer { cond.unlock() }
            while true {
                if let b = big.first {
                    let i = b.next; b.next += 1
                    if b.next >= b.chunks { big.removeFirst() }
                    active += 1
                    return .chunk(b, i)
                }
                if !small.isEmpty { active += 1; let j = small.removeLast(); cond.broadcast(); return .file(j) }
                if closed && active == 0 { cond.broadcast(); return .done }
                cond.wait()
            }
        }

        private func release() { cond.lock(); active -= 1; cond.broadcast(); cond.unlock() }

        private func worker() {
            let buf = UnsafeMutableRawPointer.allocate(byteCount: FastTransfer.chunkSize, alignment: 16384)
            defer { buf.deallocate() }
            while true {
                switch take() {
                case .done: return
                case .file(let j):
                    if progress.cancelled { release(); continue }
                    copyFile(j, buf)
                    release()
                case .chunk(let b, let i):
                    copyChunk(b, i, buf)
                    release()
                }
            }
        }

        // MARK: one file

        private func copyFile(_ j: Job, _ buf: UnsafeMutableRawPointer) {
            let kind = j.mode & S_IFMT
            let name = (j.dst as NSString).lastPathComponent
            if kind == S_IFLNK {                                        // symlinks are copied as links
                var tmp = [CChar](repeating: 0, count: Int(PATH_MAX) + 1)
                let n = readlink(j.src, &tmp, Int(PATH_MAX))
                if n >= 0 { tmp[n] = 0; if symlink(tmp, j.dst) != 0 { fail(name) } else { done(j, name) } } else { fail(name) }
                return
            }
            guard kind == S_IFREG else { return }                        // sockets, fifos, devices are skipped
            // 1. same APFS volume → clone, instant and free
            if clonefile(j.src, j.dst, UInt32(CLONE_NOFOLLOW)) == 0 { bumpClone(); progress.addBytes(j.size); done(j, name); return }
            if errno == EEXIST { fail(name, errno); return }
            // 2. big file → hand to all workers as chunks
            if j.size >= FastTransfer.bigFile { startBig(j, name); return }
            // 3. small / medium file: copyfile keeps xattrs, ACLs, mode and times
            if copyfile(j.src, j.dst, nil, copyfile_flags_t(COPYFILE_ALL | COPYFILE_EXCL)) != 0 {
                let e = errno
                if e == EEXIST { fail(name, e); return }
                // FAT/exFAT/NTFS refuse some metadata: remove the partial file and retry with data + times only
                unlink(j.dst)
                if copyfile(j.src, j.dst, nil, copyfile_flags_t(COPYFILE_DATA | COPYFILE_EXCL)) != 0 { fail(name, errno); unlink(j.dst); return }
                stampTimes(j.src, j.dst)
            }
            progress.addBytes(j.size)
            done(j, name)
        }

        private func bumpClone() { cond.lock(); cloned += 1; cond.unlock() }

        private func done(_ j: Job, _ name: String) {
            if mode == .move { unlink(j.src) }          // the copy landed, so the source can go
            progress.fileDone(name: name)
        }

        private func fail(_ name: String, _ e: Int32 = errno) { progress.fail("\(name): \(String(cString: strerror(e)))") }

        private func startBig(_ j: Job, _ name: String) {
            let s = open(j.src, O_RDONLY)
            guard s >= 0 else { fail(name); return }
            let d = open(j.dst, O_WRONLY | O_CREAT | O_EXCL, 0o600)
            guard d >= 0 else { let e = errno; close(s); fail(name, e); return }
            _ = fcntl(s, F_RDAHEAD, 1)
            if ftruncate(d, off_t(j.size)) != 0 { let e = errno; close(s); close(d); unlink(j.dst); fail(name, e); return }
            let b = BigFile(job: j, srcFD: s, dstFD: d)
            cond.lock(); big.append(b); cond.broadcast(); cond.unlock()
        }

        private func copyChunk(_ b: BigFile, _ i: Int, _ buf: UnsafeMutableRawPointer) {
            let off = Int64(i) * Int64(FastTransfer.chunkSize)
            let len = Int(min(Int64(FastTransfer.chunkSize), b.job.size - off))
            var ok = !progress.cancelled
            var got = 0
            while ok && got < len {
                let n = pread(b.srcFD, buf + got, len - got, off_t(off) + off_t(got))
                if n <= 0 { ok = false; if n < 0 { fail((b.job.dst as NSString).lastPathComponent) }; break }
                var w = 0
                while w < n {
                    let k = pwrite(b.dstFD, buf + got + w, n - w, off_t(off) + off_t(got + w))
                    if k <= 0 { ok = false; fail((b.job.dst as NSString).lastPathComponent); break }
                    w += k
                }
                got += n
                progress.addBytes(Int64(n))
            }
            cond.lock()
            b.finished += 1; if !ok { b.failed = true }
            let last = b.finished == b.chunks
            cond.unlock()
            if last {
                let name = (b.job.dst as NSString).lastPathComponent
                if b.failed || progress.cancelled { close(b.srcFD); close(b.dstFD); unlink(b.job.dst); return }
                _ = fcopyfile(b.srcFD, b.dstFD, nil, copyfile_flags_t(COPYFILE_METADATA))   // mode, owner, times, xattrs, ACLs
                close(b.srcFD); close(b.dstFD)
                stampTimes(b.job.src, b.job.dst)
                done(b.job, name)
            }
        }

        private func stampTimes(_ src: String, _ dst: String) {
            var s = stat(); guard stat(src, &s) == 0 else { return }
            var t = [timeval(tv_sec: s.st_atimespec.tv_sec, tv_usec: Int32(s.st_atimespec.tv_nsec / 1000)),
                     timeval(tv_sec: s.st_mtimespec.tv_sec, tv_usec: Int32(s.st_mtimespec.tv_nsec / 1000))]
            utimes(dst, &t)
        }

        private func applyDirMeta(_ s: String, _ d: String, _ st: stat) {
            _ = copyfile(s, d, nil, copyfile_flags_t(COPYFILE_METADATA))
            stampTimes(s, d)
        }
    }
}

/// " copy" / " copy N" naming that never collides with a name already taken (case-insensitive, like APFS/HFS+/exFAT).
enum NameClash {
    static func unique(_ name: String, taken: Set<String>) -> String {
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
}
