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
}
