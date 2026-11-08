# Testing

`cargo test --workspace` (macOS required for fixture-backed tests; elsewhere they are skipped with a message).

Fixtures are never hand-built for filesystem drivers: `tests/fixtures/make_fat_fixture.sh` uses `hdiutil` to let macOS create and populate FAT12/16/32 and exFAT images. After VOLUMA writes, the independent judge is macOS: `fsck_msdos -n` / `fsck_exfat -n` on the attached device and a read-only OS mount compared byte-for-byte. The verifiers were confirmed able to fail by injecting an orphan cluster (fsck_msdos exit 206) and a bitmap error (fsck_exfat exit 203).

Covered: long/Unicode names, nested directories, directory growth, 2.5–3 MB multi-cluster files (SHA-256 / byte compare), overwrite, rename/move, delete, out-of-space rollback, interrupted source, dirty-flag refusal, read-only refusal, partition-window confinement, randomized corruption (no panics).

Not covered yet: fuzzing, power-loss simulation, device disconnection, concurrency, performance benchmarks, UI automation tests, Windows/Linux.
