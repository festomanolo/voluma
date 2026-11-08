# Filesystem support

Updated only when a capability is implemented **and** tested.

| Filesystem | Detect | Read | Write | Notes |
|---|---|---|---|---|
| FAT12/16/32 | yes | yes | yes | see below |
| exFAT | yes | yes | yes | see below |
| NTFS, ext2/3/4, APFS, HFS+, Btrfs, XFS, ZFS | detection only (unit-tested signatures) | no | no | no driver exists |

## FAT12/16/32 (`fs-fat`)
Supported: long names (UTF-16), 8.3 short-name generation with `~N` tails, nested directories, directory growth, create / overwrite / delete / rename (including moving directories, with `..` fix-up), streaming writes, rollback on out-of-space or source failure.
Safety: refuses writes on a device opened read-only and on volumes whose clean-shutdown bit is clear (FAT16/32). FAT12 has no such bit, so dirtiness cannot be detected there.
Limitations: no Unicode normalisation; non-ASCII chars in generated short names become `_`; timestamps stored without time zone (treated as UTC); FAT32 FSInfo free-count is written as "unknown"; file size limit 4 GiB; fixed FAT12/16 root directory cannot grow; rename is not atomic (new entry is written before the old is erased, so a crash can leave both, never neither); no `fsck`/repair; no short-name-only (lowercase flag) writes, case is preserved via LFN.

## exFAT (`fs-exfat`)
Supported: entry-set checksum validation, NoFatChain and FAT-chained files, allocation-bitmap allocation, up-case-table-driven name matching/hashing, create / overwrite / delete / rename / mkdir, directory growth (converting NoFatChain directories), streaming writes with rollback.
Safety: refuses writes if VolumeDirty or MediaFailure is set.
Limitations: new data always uses FAT chains (never contiguous allocation) so large files may fragment; backup boot region and PercentInUse are not maintained (PercentInUse set to "unavailable"); TexFAT / multiple FATs unsupported; no ACLs; `valid_data_length < data_length` files report the valid length; rename not atomic (same ordering note as FAT).

## Not yet implemented anywhere
Mounting, permissions, extended attributes, symlinks, snapshots, encryption, recovery, repair.
