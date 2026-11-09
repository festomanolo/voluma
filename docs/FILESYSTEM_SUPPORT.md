# Filesystem support

Updated only when a capability is implemented **and** tested.

| Filesystem | Detect | Read | Write | Notes |
|---|---|---|---|---|
| FAT12/16/32 | yes | yes | yes | see below |
| exFAT | yes | yes | yes | see below |
| HFS+ / HFSX | yes | yes (read-only) | no | see below |
| APFS | yes | yes (read-only) | no | see below |
| ext2/3/4 | yes | yes (read-only) | no | see below |
| NTFS | yes | yes | yes (guarded) | see below |
| XFS, Btrfs | detection only (unit-tested signatures) | no | no | no driver exists |
| ZFS | **not detected** | no | no | no detection, no driver |

## FAT12/16/32 (`fs-fat`)
Supported: long names (UTF-16), 8.3 short-name generation with `~N` tails, nested directories, directory growth, create / overwrite / delete / rename (including moving directories, with `..` fix-up), streaming writes, rollback on out-of-space or source failure.
Safety: refuses writes on a device opened read-only and on volumes whose clean-shutdown bit is clear (FAT16/32). FAT12 has no such bit, so dirtiness cannot be detected there.
Limitations: no Unicode normalisation; non-ASCII chars in generated short names become `_`; timestamps stored without time zone (treated as UTC); FAT32 FSInfo free-count is written as "unknown"; file size limit 4 GiB; fixed FAT12/16 root directory cannot grow; rename is not atomic (new entry is written before the old is erased, so a crash can leave both, never neither); no `fsck`/repair; no short-name-only (lowercase flag) writes, case is preserved via LFN.

## exFAT (`fs-exfat`)
Supported: entry-set checksum validation, NoFatChain and FAT-chained files, allocation-bitmap allocation, up-case-table-driven name matching/hashing, create / overwrite / delete / rename / mkdir, directory growth (converting NoFatChain directories), streaming writes with rollback.
Safety: refuses writes if VolumeDirty or MediaFailure is set.
Limitations: new data always uses FAT chains (never contiguous allocation) so large files may fragment; backup boot region and PercentInUse are not maintained (PercentInUse set to "unavailable"); TexFAT / multiple FATs unsupported; no ACLs; `valid_data_length < data_length` files report the valid length; rename not atomic (same ordering note as FAT).

## HFS+ / HFSX (`fs-hfsplus`, read-only)
Supported: catalog and extents-overflow B-trees, files with >8 extents, hard links (resolved via the private metadata directory), case-insensitive (HFS+) and case-sensitive (HFSX) lookup, volume name, hidden flags.
Safety: refuses volumes with an unreplayed journal (stale metadata) or with the software-lock attribute.
Limitations: transparently compressed (decmpfs) files are listed but reads fail with `Unsupported`; names are shown as stored (Unicode NFD, on-disk `:` is BSD `/`) and matching does not normalise NFC/NFD or use Apple's exact fold table (simple Unicode lowercase); symlinks are shown as small files holding the target; resource forks, xattrs, ACLs and directory hard links are not exposed; legacy HFS wrapper volumes are not supported. Verified against a read-only macOS mount of an OS-created image (616 entries, byte-compared) plus hard links, a fragmented file and a 600-file directory.

## APFS (`fs-apfs`, read-only)
Supported: newest valid checkpoint superblock, container/volume object maps, FS B-tree (keys verified by Fletcher-64 on every object read), extents incl. sparse holes, hard links, symlinks, case-sensitive and -insensitive volumes. Opens the first volume by default (`ApfsFs::open_volume` selects another).
Limitations: encrypted volumes are refused; snapshots are not browsable (live state only); compressed files fail on read; sealed/system-volume firmlinks, fusion drives, clone-aware free-space and xattrs are not exposed; the FFI offers no volume picker yet, so only the first volume of a container can be browsed from the app. Verified against a macOS mount (616 entries, byte-compared), including case-sensitive volumes and 200+ corrupted-image runs without panics.

## ext2/3/4 (`fs-ext`, read-only)
Supported: indirect-block and extent files (including multi-level extent trees and triple-indirect), sparse files, uninitialised extents, 64-bit, flex_bg, meta_bg, large and htree directories (linear scan), symlinks (fast and slow), hard links.
Safety: refuses volumes needing journal recovery and any unknown incompatible feature; encrypted and inline-data inodes fail with `Unsupported`.
Limitations: no checksum verification, no journal replay, casefold/encryption/inline-data unsupported, device nodes/FIFOs/sockets appear as empty files, xattrs/ACLs/permissions are not exposed. Verified against `mke2fs -d` images (host tree is ground truth, images confirmed clean by `e2fsck -fn`) for ext2 (1k), ext3, ext4 (1k and 4k) and ext4 with 64bit+meta_bg. Fixture tests need e2fsprogs (`tests/fixtures/build_e2fsprogs.sh`) and skip with a message when `VOLUMA_E2FSPROGS` is unset and the tools are not on `PATH`.

## NTFS (`fs-ntfs`, read + write)
Read: MFT incl. fragmented MFT and attribute lists, resident / non-resident / sparse data, B-tree directories (INDEX_ROOT + INDEX_ALLOCATION, tested to depth 3 with 3000+ entries), Win32/POSIX names (DOS 8.3 names hidden), `$UpCase` case-insensitive lookup, volume label. `$`-metafiles are hidden.
Write: create / overwrite / delete / rename (incl. moving directories) / mkdir. Directory index inserts split nodes, convert a full index root into INDEX_ALLOCATION blocks, grow the allocation, and deletes replace internal entries with their in-order neighbour. The MFT grows when it runs out of records. New files inherit the parent's security (security id, or a copied descriptor on volumes without `$Secure`).
Safety: writes need a read-write device AND a clean volume: NTFS 3.x, volume dirty bit clear, `$LogFile` clean, and no Windows hibernation/Fast Startup (`hiberfil.sys` marker). Otherwise `Refused` with the reason (the app shows it). The dirty bit is raised for the duration of each operation and cleared afterwards; after an I/O error it is left set so Windows runs chkdsk. Data is written first, then bitmaps, then the MFT record, and the directory entry last; failures release what was allocated. System files (MFT records < 24) cannot be deleted or renamed.
Limitations: compressed (LZNT1) and EFS-encrypted data are refused (`Unsupported`), as are dedup/cloud reparse points on read; writing refuses files with attribute lists, hard links, compressed/encrypted/sparse data and alternate-stream-heavy records; no `$LogFile` / `$UsnJrnl` / `$ObjId` / quota updates (like ntfs-3g); new names use the Win32 namespace only (no 8.3 alias); deleting can leave empty index blocks (valid, not compacted); the MFT bitmap must be non-resident and large directories whose index bitmap is non-resident cannot grow; no ADS or ACL editing; rename is not atomic (old names are unlinked before the new one is linked, restoring them if linking fails). **Not verified against Windows**: ntfs-3g (`ntfsresize -i -f`, `ntfscat`, `ntfsls`) is the only independent judge; run `chkdsk` on Windows before trusting a drive you wrote to with real data. Fixtures need ntfs-3g (`tests/fixtures/build_ntfs3g.sh`, then `VOLUMA_NTFS3G=...`); the tests SKIP loudly without it.

### Writing to NTFS volumes macOS mounted read-only (app)
macOS mounts NTFS read-only, so the app's native browser can't paste into it. **Enable Writing…** (shown on such volumes) unmounts the volume from macOS, starts `voluma-helper` (one administrator prompt; skipped when the device node is already writable by you, e.g. disk images) and browses the raw partition through VOLUMA's own NTFS driver, with all the guards above. **Finish & Remount** (or leaving the window, or quitting the app) stops the helper and remounts the volume in macOS. Copy first, then enable writing, because leaving the browser ends the session.
Helper security (`crates/voluma-helper`): it serves exactly one `/dev/diskN[sM]` fixed at launch; its Unix socket is mode 0600 and each connection's peer uid is checked; it touches host files only by bare name inside a per-session staging directory, with `O_NOFOLLOW`, requiring regular files owned by you, and copies through a root-private temp directory, so a request can neither read arbitrary files into the volume nor write arbitrary files out (all tested, including `../`, absolute paths, symlinks); it flushes the device after every write and exits when the app exits or after 15 minutes idle. **Development-grade packaging:** the helper is launched from the build directory; a distributable app needs a signed helper installed through SMAppService/`SMJobBless`, which is not done.

## Not yet implemented anywhere
Mounting, permissions, extended attributes, symlinks, snapshots, encryption, recovery, repair.
