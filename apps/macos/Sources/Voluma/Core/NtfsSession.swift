import Foundation
import AppKit
import DiskArbitration

/// macOS mounts NTFS read-only. To write, VOLUMA unmounts the volume from macOS, starts `voluma-helper` as root
/// (one administrator prompt) so its own NTFS driver can use the raw partition, and remounts the volume afterwards.
/// The helper serves exactly this one device over a Unix socket that only this user can use (see the helper's docs).
private struct HelperEnv<U: Decodable>: Decodable { let ok: U?; let error: String? }

final class NtfsSession: @unchecked Sendable {
    let bsd: String
    let title: String
    let stage: URL
    private let lock = NSLock()
    private var fd: Int32 = -1

    struct SessionError: LocalizedError { let message: String; var errorDescription: String? { message } }

    init(bsd: String, title: String, stage: URL) { self.bsd = bsd; self.title = title; self.stage = stage }

    // MARK: protocol
    func connect() throws {
        let path = stage.appendingPathComponent("helper.sock").path
        let s = socket(AF_UNIX, SOCK_STREAM, 0)
        guard s >= 0 else { throw SessionError(message: "socket() failed") }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8CString)
        guard bytes.count <= MemoryLayout.size(ofValue: addr.sun_path) else { Darwin.close(s); throw SessionError(message: "socket path too long") }
        withUnsafeMutableBytes(of: &addr.sun_path) { p in bytes.withUnsafeBytes { p.copyMemory(from: $0) } }
        let ok = withUnsafePointer(to: &addr) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(s, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) } }
        guard ok == 0 else { Darwin.close(s); throw SessionError(message: "cannot reach the NTFS helper") }
        fd = s
    }

    /// One request/response line. Serialised: the helper handles a single request at a time.
    func call(_ obj: [String: Any]) throws -> Data {
        lock.lock(); defer { lock.unlock() }
        guard fd >= 0 else { throw SessionError(message: "NTFS writing session is closed") }
        var line = try JSONSerialization.data(withJSONObject: obj)
        line.append(0x0A)
        try line.withUnsafeBytes { p in
            var off = 0
            while off < p.count {
                let n = write(fd, p.baseAddress! + off, p.count - off)
                if n <= 0 { throw SessionError(message: "lost connection to the NTFS helper") }
                off += n
            }
        }
        var out = Data()
        var ch: UInt8 = 0
        while true {
            let n = read(fd, &ch, 1)
            if n <= 0 { throw SessionError(message: "lost connection to the NTFS helper") }
            if ch == 0x0A { return out }
            out.append(ch)
        }
    }

    /// Calls and unwraps the engine's {"ok": …} / {"error": …} envelope.
    func callOK<T: Decodable>(_ obj: [String: Any], as: T.Type) throws -> T {
        let data = try call(obj)
        guard let env = try? JSONDecoder().decode(HelperEnv<T>.self, from: data) else { throw SessionError(message: "unreadable helper response") }
        if let e = env.error { throw SessionError(message: e) }
        guard let ok = env.ok else { throw SessionError(message: "empty helper response") }
        return ok
    }

    func close() { lock.lock(); if fd >= 0 { Darwin.close(fd); fd = -1 }; lock.unlock() }
}

/// Starts and ends sessions. At most one per partition.
@MainActor
final class NtfsSessions {
    static let shared = NtfsSessions()
    private(set) var active: [String: NtfsSession] = [:]
    private var observer: NSObjectProtocol?

    init() {
        // Never leave a volume unmounted behind us.
        observer = NotificationCenter.default.addObserver(forName: NSApplication.willTerminateNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.endAllSync() }
        }
    }

    func session(for bsd: String) -> NtfsSession? { active[bsd] }

    static func locateHelper() -> URL? {
        let fm = FileManager.default
        var candidates: [URL] = []
        if let e = ProcessInfo.processInfo.environment["VOLUMA_HELPER"] { candidates.append(URL(fileURLWithPath: e)) }
        if let exe = Bundle.main.executableURL {
            candidates.append(exe.deletingLastPathComponent().appendingPathComponent("voluma-helper"))
            // swift build layout: <repo>/apps/macos/.build/<config>/Voluma  ->  <repo>/target/release/voluma-helper
            var up = exe.deletingLastPathComponent()
            for _ in 0..<4 { up = up.deletingLastPathComponent() }
            candidates.append(up.appendingPathComponent("target/release/voluma-helper"))
        }
        return candidates.first { fm.isExecutableFile(atPath: $0.path) }
    }

    /// Unmounts the volume, starts the helper (admin prompt), and connects. On any failure the volume is remounted.
    func start(bsd: String, title: String) async throws -> NtfsSession {
        if let s = active[bsd] { return s }
        guard let helper = Self.locateHelper() else { throw NtfsSession.SessionError(message: "voluma-helper wasn’t found. Build it with apps/macos/build.sh.") }
        let stage = FileManager.default.temporaryDirectory.appendingPathComponent("voluma-\(UUID().uuidString.prefix(8))")   // short: unix socket paths max out at 104 bytes
        try FileManager.default.createDirectory(at: stage, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        let canonical = stage.resolvingSymlinksInPath()
        let quote = "'\\\"\n"
        guard !helper.path.contains(where: { quote.contains($0) }), !canonical.path.contains(where: { quote.contains($0) }) else {
            throw NtfsSession.SessionError(message: "paths containing quotes aren’t supported")
        }

        if let err = await Ejector.unmount(bsd: bsd) {
            try? FileManager.default.removeItem(at: stage)
            throw NtfsSession.SessionError(message: "Couldn’t unmount from macOS (close any windows using it): \(err)")
        }
        let session = NtfsSession(bsd: bsd, title: title, stage: canonical)
        do {
            // Least privilege: only ask for administrator rights if the device node isn't already ours (disk images are).
            let direct = access("/dev/\(bsd)", R_OK | W_OK) == 0
            let args = ["serve", "--device", "/dev/\(bsd)", "--stage", canonical.path, "--uid", "\(getuid())", "--parent", "\(getpid())"]
            let cmd = "'\(helper.path)' serve --device '/dev/\(bsd)' --stage '\(canonical.path)' --uid \(getuid()) --parent \(getpid()) > /dev/null 2> '\(canonical.path)/helper.log' &"
            let script = "do shell script \"\(cmd)\" with prompt \"VOLUMA needs administrator access to write to the NTFS volume “\(title.filter { !quote.contains($0) })”.\" with administrator privileges"
            let status: Int32 = await withCheckedContinuation { cont in
                DispatchQueue.global().async {
                    let p = Process()
                    if direct { p.executableURL = helper; p.arguments = args }
                    else { p.executableURL = URL(fileURLWithPath: "/usr/bin/osascript"); p.arguments = ["-e", script] }
                    p.standardOutput = FileHandle.nullDevice
                    p.standardError = (try? FileHandle(forWritingTo: { FileManager.default.createFile(atPath: canonical.appendingPathComponent("helper.log").path, contents: nil); return canonical.appendingPathComponent("helper.log") }())) ?? FileHandle.nullDevice
                    do { try p.run(); if !direct { p.waitUntilExit() }; cont.resume(returning: direct ? 0 : p.terminationStatus) } catch { cont.resume(returning: -1) }
                }
            }
            guard status == 0 else { throw NtfsSession.SessionError(message: "Administrator access was not granted.") }
            // wait for the socket
            var up = false
            for _ in 0..<100 {
                if FileManager.default.fileExists(atPath: canonical.appendingPathComponent("helper.sock").path) { up = true; break }
                try await Task.sleep(nanoseconds: 100_000_000)
            }
            guard up else {
                let log = (try? String(contentsOf: canonical.appendingPathComponent("helper.log"), encoding: .utf8))?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                throw NtfsSession.SessionError(message: "The NTFS helper didn’t start" + (log.isEmpty ? "." : ": \(log)"))
            }
            try session.connect()
            _ = try session.call(["op": "ping"])
            // surface a refusal (dirty / hibernated volume) now instead of at the first paste
            struct L: Decodable { let write_blocked: String? }
            if let why = try session.callOK(["op": "list", "path": "/"], as: L.self).write_blocked {
                throw NtfsSession.SessionError(message: "Can’t write safely: \(why)")
            }
        } catch {
            session.close()
            _ = try? session.call(["op": "quit"])
            try? FileManager.default.removeItem(at: stage)
            _ = await Ejector.mount(bsd: bsd)
            throw error
        }
        active[bsd] = session
        return session
    }

    /// Stops the helper and gives the volume back to macOS.
    func end(bsd: String) async -> String? {
        guard let s = active.removeValue(forKey: bsd) else { return nil }
        _ = try? s.call(["op": "quit"])
        s.close()
        for _ in 0..<30 where FileManager.default.fileExists(atPath: s.stage.appendingPathComponent("helper.sock").path) {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        try? FileManager.default.removeItem(at: s.stage)
        return await Ejector.mount(bsd: bsd)
    }

    private func endAllSync() {
        for (bsd, s) in active {
            _ = try? s.call(["op": "quit"]); s.close()
            try? FileManager.default.removeItem(at: s.stage)
            let p = Process(); p.executableURL = URL(fileURLWithPath: "/usr/sbin/diskutil"); p.arguments = ["mount", "/dev/\(bsd)"]
            p.standardOutput = FileHandle.nullDevice; p.standardError = FileHandle.nullDevice
            try? p.run(); p.waitUntilExit()
        }
        active = [:]
    }
}

/// The browser backend for a volume being written through the helper.
struct HelperBackend: FSBackend {
    let session: NtfsSession
    var id: String { "ntfs-helper:\(session.bsd)" }
    var isNative: Bool { false }
    func list(_ p: String) throws -> Listing { try session.callOK(["op": "list", "path": p], as: Listing.self) }
    func hostURL(_ path: String) -> URL? { nil }

    struct Done: Decodable { let bytes: UInt64?; let done: String? }

    func exportFile(_ path: String, to dest: URL) throws {
        let name = "out-\(UUID().uuidString)"
        _ = try session.callOK(["op": "export", "path": path, "dst": name], as: Done.self)
        let staged = session.stage.appendingPathComponent(name)
        defer { try? FileManager.default.removeItem(at: staged) }
        try FileManager.default.moveItem(at: staged, to: dest)
    }
    func importFile(from host: URL, to path: String) throws {
        let name = "in-\(UUID().uuidString)"
        let staged = session.stage.appendingPathComponent(name)
        try FileManager.default.copyItem(at: host, to: staged)
        defer { try? FileManager.default.removeItem(at: staged) }
        _ = try session.callOK(["op": "import", "src": name, "path": path], as: Done.self)
    }
    func mkdir(_ path: String) throws { _ = try session.callOK(["op": "mkdir", "path": path], as: Done.self) }
    func delete(_ path: String) throws { _ = try session.callOK(["op": "delete", "path": path], as: Done.self) }
    func rename(_ a: String, _ b: String) throws { _ = try session.callOK(["op": "rename", "path": a, "to": b], as: Done.self) }
}
