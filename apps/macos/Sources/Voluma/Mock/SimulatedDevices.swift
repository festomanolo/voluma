import Foundation

/// SIMULATED DATA. Replaced by platform-macos device discovery (DiskArbitration/IOKit) in Stage 8.
/// Anything built from this type must display the "Simulated" badge.
struct SimulatedDevice: Identifiable {
    let id = UUID()
    let name: String; let model: String; let connection: String
    let capacity: Int64; let used: Int64; let filesystem: String; let access: String
}

enum SimulatedData {
    static let devices: [SimulatedDevice] = [
        .init(name: "Samsung T7", model: "Portable SSD", connection: "USB 3.2", capacity: 1_000_000_000_000, used: 412_000_000_000, filesystem: "exFAT", access: "Read-only"),
        .init(name: "Windows Backup", model: "WD Elements", connection: "USB 3.0", capacity: 2_000_000_000_000, used: 1_310_000_000_000, filesystem: "NTFS", access: "Read-only"),
        .init(name: "Linux Data", model: "SanDisk Ultra", connection: "USB 3.0", capacity: 128_000_000_000, used: 40_000_000_000, filesystem: "ext4", access: "Read-only"),
    ]
}
