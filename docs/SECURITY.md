# Safety model (as implemented today)

1. Everything opens read-only by default. A writable open happens only inside `voluma_mutate`, per call.
2. The UI keeps write access locked per browser window and requires a confirmation to unlock; deletes need a second confirmation. The Rust side does not trust the UI: each driver independently refuses writes on read-only devices and dirty volumes.
3. Drivers receive a `PartitionWindow`, not the whole device. A test proves bytes outside the window are unchanged after heavy writes (FAT and exFAT).
4. All on-disk geometry is validated before use; corrupt images yield errors, never out-of-range access or endless loops (cluster chains are loop-bounded). 200-round randomized corruption tests run for FAT and exFAT.
5. No overwrite without an explicit flag; export refuses to clobber an existing host file.
6. The frontend only receives JSON; no device handle crosses the FFI boundary.
7. The engine opens image files directly. Real partitions are opened only by `voluma-helper`, one device per session, fixed at launch, and only after the volume has been unmounted from macOS (or was never mounted). The helper refuses anything but `/dev/diskN` or `/dev/diskNsM`, so it cannot be pointed at an arbitrary file.

## Privileged helper (`crates/voluma-helper`)

Raw device nodes need root, so the app starts the helper through an administrator prompt (skipped if the device node is already writable by the user, as for disk images).

- Socket: mode 0600, owned by the invoking user, in a 0700 staging folder; the peer uid is checked on every connection.
- Host files: requests name a bare file inside the staging folder, opened with `O_NOFOLLOW`, and must be regular files owned by the invoking user. Imports and exports pass through a root-private temp directory, so a request can neither read arbitrary files into the volume nor write arbitrary files out. Tests cover `../`, absolute paths, symlinks and the socket's own name.
- Lifetime: exits when the app exits or after 15 minutes idle; flushes the device after every write.
- Limits: the helper binary is launched from inside the app bundle, so its integrity is only as good as that location's permissions. A distributable build should install a Developer ID-signed helper with SMAppService and verify the caller's code signature. That is not done yet.

Known gaps: no fuzzing harness yet (randomized corruption tests only); no crash/power-loss simulation; no concurrent-access locking between processes; image files can be modified by other programs while open.
