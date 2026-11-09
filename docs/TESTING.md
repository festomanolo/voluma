# Testing

`cargo test --workspace` (macOS required for fixture-backed tests; elsewhere they are skipped with a message).

Fixtures are never hand-built for filesystem drivers: `tests/fixtures/make_fat_fixture.sh` uses `hdiutil` to let macOS create and populate FAT12/16/32 and exFAT images. After VOLUMA writes, the independent judge is macOS: `fsck_msdos -n` / `fsck_exfat -n` on the attached device and a read-only OS mount compared byte-for-byte. The verifiers were confirmed able to fail by injecting an orphan cluster (fsck_msdos exit 206) and a bitmap error (fsck_exfat exit 203).

Read-only drivers: `tests/fixtures/make_mac_fixture.sh` makes HFS+/HFSX/APFS images with macOS and the tests compare the driver's whole tree (names, sizes, bytes, symlink targets) with a real read-only `hdiutil` mount. ext images come from `mke2fs -d` (build the tools with `tests/fixtures/build_e2fsprogs.sh`, then set `VOLUMA_E2FSPROGS`); the source directory is the ground truth and `e2fsck -fn` must pass on the fixture. Each driver also has a randomized-corruption test that only asserts "no panic". The ext tests were confirmed able to fail by breaking the indirect-block arithmetic.

Covered: long/Unicode names, nested directories, directory growth, 2.5–3 MB multi-cluster files (SHA-256 / byte compare), overwrite, rename/move, delete, out-of-space rollback, interrupted source, dirty-flag refusal, read-only refusal, partition-window confinement, randomized corruption (no panics).

Not covered yet: fuzzing, power-loss simulation, device disconnection, concurrency, performance benchmarks, UI automation tests, Windows/Linux.
