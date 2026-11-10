import Foundation
import CVoluma

/// Decoded result of the real Rust engine (`voluma-ffi`). Nothing here is simulated.
struct ImageReport: Decodable, Identifiable {
    struct Table: Decodable { let kind: String; let disk_guid: String? }
    struct Filesystem: Decodable { let kind: String; let label: String?; let detail: String? }
    struct Part: Decodable, Identifiable {
        let index: Int; let offset: UInt64; let length: UInt64
        let type_id: String; let type_name: String?; let name: String?
        let is_efi_system: Bool; let filesystem: Filesystem
        var id: Int { index }
    }
    let path: String; let size: UInt64; let read_only: Bool
    let table: Table; let whole_disk_filesystem: Filesystem?; let partitions: [Part]
    var id: String { path }
}

enum EngineError: LocalizedError {
    case engine(String), decode
    var errorDescription: String? {
        switch self { case .engine(let m): m; case .decode: "Unreadable engine response" }
    }
}

enum Engine {
    static func inspectImage(path: String) throws -> ImageReport {
        guard let raw = voluma_inspect_image(path) else { throw EngineError.decode }
        defer { voluma_free_string(raw) }
        let data = Data(String(cString: raw).utf8)
        struct Envelope: Decodable { let ok: ImageReport?; let error: String? }
        guard let env = try? JSONDecoder().decode(Envelope.self, from: data) else { throw EngineError.decode }
        if let e = env.error { throw EngineError.engine(e) }
        guard let ok = env.ok else { throw EngineError.decode }
        return ok
    }

    private static func call<T: Decodable>(_ raw: UnsafeMutablePointer<CChar>?, as: T.Type) throws -> T {
        guard let raw else { throw EngineError.decode }
        defer { voluma_free_string(raw) }
        guard let env = try? JSONDecoder().decode(Env<T>.self, from: Data(String(cString: raw).utf8)) else { throw EngineError.decode }
        if let e = env.error { throw EngineError.engine(e) }
        guard let ok = env.ok else { throw EngineError.decode }
        return ok
    }

    static func listDirectory(image: String, part: Int, path: String) throws -> Listing {
        try call(voluma_list_directory(image, Int32(part), path), as: Listing.self)
    }

    @discardableResult
    static func mutate(image: String, part: Int, op: String, a: String, b: String = "", overwrite: Bool = false) throws -> MutateResult {
        try call(voluma_mutate(image, Int32(part), op, a, b, overwrite ? 1 : 0), as: MutateResult.self)
    }

    static func exportFile(image: String, part: Int, path: String, dest: String) throws -> ExportResult {
        try call(voluma_export_file(image, Int32(part), path, dest), as: ExportResult.self)
    }
}

struct Listing: Decodable {
    struct Entry: Decodable, Identifiable, Hashable {
        let name: String; let kind: String; let size: UInt64
        let modified: Int64?; let hidden: Bool
        /// Filled in for mounted volumes only (the Rust engine does not send them).
        var created: Int64? = nil
        var tags: [String]? = nil
        var label: Int? = nil
        var id: String { name }
        var isDir: Bool { kind == "directory" }
        var modifiedSort: Int64 { modified ?? 0 }
        var kindText: String { FileKinds.describe(self) }
    }
    struct Caps: Decodable { let read: Bool; let write: Bool }
    let filesystem: String; let label: String?; let entries: [Entry]; let capabilities: Caps; let driver_supports_write: Bool
    let write_blocked: String?
}

struct MutateResult: Decodable { let done: String; let bytes: UInt64? }
struct ExportResult: Decodable { let bytes: UInt64; let sha256: String }

private struct Env<U: Decodable>: Decodable { let ok: U?; let error: String? }
