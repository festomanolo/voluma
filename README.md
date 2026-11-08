# VOLUMA
**Every disk. Every filesystem. One place.**

A universal filesystem explorer and storage manager. Rust core engine, native SwiftUI front end on macOS.

## Status: early foundation
| Filesystem | Detection | Read | Write | Mount |
|---|---|---|---|---|
| FAT12/16/32 | Tested | Planned | Planned | Planned |
| exFAT | Tested (unit) | Planned | Planned | Planned |
| NTFS | Tested (unit) | Planned | Planned | Planned |
| ext2/3/4 | Tested (unit) | Planned | Planned | Planned |
| APFS | Signature only | Planned | No | Planned |

"Tested (unit)" = hand-built fixtures. FAT32 is also verified against an image made by `newfs_msdos`.

### Real vs. simulated
- **Real:** raw image backend (read-only by default, bounds-checked), partition windows, MBR/GPT parsing, signature-based filesystem detection, the C-ABI bridge, and the SwiftUI "Open Disk Image" inspector.
- **Simulated (badged in the UI):** the dashboard's device list (`apps/macos/Sources/Voluma/Mock`).
- **Not started:** every filesystem driver, file browsing, transfers, recovery, mounting, device discovery.

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
