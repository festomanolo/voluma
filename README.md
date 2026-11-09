# VOLUMA
**Every disk. Every filesystem. One place.**

A universal filesystem explorer and storage manager. Rust core engine, native SwiftUI front end on macOS.

## Status: early foundation
| Filesystem | Detection | Read | Write | Mount |
|---|---|---|---|---|
| FAT12/16/32 | Tested (OS-generated images) | **Tested** | **Tested** (macOS `fsck_msdos` + OS mount) | Planned |
| exFAT | Tested (OS-generated images) | **Tested** | **Tested** (macOS `fsck_exfat` + OS mount) | Planned |
| HFS+ / HFSX | Tested (OS-generated images) | **Tested** (vs. a real macOS mount) | No | Planned |
| APFS | Tested (OS-generated images) | **Tested** (vs. a real macOS mount) | No | Planned |
| ext2/3/4 | Tested (unit) | **Tested** (`mke2fs -d` images, `e2fsck`-clean) | No | Planned |
| NTFS | Tested (unit) | Planned | Planned | Planned |
| XFS, Btrfs | Tested (unit) | Planned | Planned | Planned |
| ZFS | Not detected | Planned | Planned | Planned |

"Tested (unit)" = hand-built fixtures only. FAT and exFAT are tested against images generated and populated by macOS's own implementations (`tests/fixtures/make_fat_fixture.sh`): long/Unicode names, nested dirs, a 3 MB SHA-256-checked file, and 200 corrupted-image reads. Fixture tests need macOS; on macOS a generator failure fails the test instead of skipping.

### Real vs. simulated
- **Real:** raw image backend (read-only by default, bounds-checked), partition windows, MBR/GPT parsing, signature-based filesystem detection, the C-ABI bridge, and the SwiftUI "Open Disk Image" inspector.
- **Simulated (badged in the UI):** the dashboard's device list (`apps/macos/Sources/Voluma/Mock`).
- **Real (new):** FAT12/16/32 (`fs-fat`) and exFAT (`fs-exfat`) read/write drivers, `fs-core` filesystem trait with capability reporting, folder browsing, SHA-256-verified export, and (behind an explicit unlock) import / new folder / rename / delete in the app. Write access is off by default.
- **Real (read-only):** HFS+/HFSX (`fs-hfsplus`), APFS (`fs-apfs`) and ext2/3/4 (`fs-ext`) browse + verified export; the FFI refuses writes to them.
- **Not started:** NTFS / Btrfs / XFS / ZFS drivers, cross-volume transfers, recovery, mounting, real device discovery.

## Layout
- `crates/block-device`: `BlockDevice` trait, `ImageDevice`, `PartitionWindow` (drivers get a window, so they can't write outside their partition)
- `crates/partition-manager`: MBR/GPT parsing that validates all on-disk geometry
- `crates/filesystem-detection`: signature/structure-based identification (never names)
- `crates/voluma-ffi`: JSON-over-C-ABI; the front end never gets a device handle
- `apps/macos`: SwiftUI app (SwiftPM)

## Architecture decision: Swift on macOS
The original spec called for Tauri + React. Per project direction, the macOS front end is native SwiftUI over the same Rust engine. Windows/Linux front ends remain open (Tauri is still a candidate there). The FFI boundary is the stable contract.

## Build & test
```sh
cargo test --workspace
apps/macos/build.sh && apps/macos/.build/release/Voluma
```

## Docs
- [Filesystem support & limitations](docs/FILESYSTEM_SUPPORT.md)
- [Safety model](docs/SECURITY.md)
- [Testing](docs/TESTING.md)
