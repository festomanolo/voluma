# Safety model (as implemented today)

1. Everything opens read-only by default. A writable open happens only inside `voluma_mutate`, per call.
2. The UI keeps write access locked per browser window and requires a confirmation to unlock; deletes need a second confirmation. The Rust side does not trust the UI: each driver independently refuses writes on read-only devices and dirty volumes.
3. Drivers receive a `PartitionWindow`, not the whole device. A test proves bytes outside the window are unchanged after heavy writes (FAT and exFAT).
4. All on-disk geometry is validated before use; corrupt images yield errors, never out-of-range access or endless loops (cluster chains are loop-bounded). 200-round randomized corruption tests run for FAT and exFAT.
5. No overwrite without an explicit flag; export refuses to clobber an existing host file.
6. The frontend only receives JSON; no device handle crosses the FFI boundary.
7. Only disk-image files are supported. Physical devices, and any protection against touching the host OS volume, are not yet implemented, so VOLUMA does not open block devices at all.

Known gaps: no fuzzing harness yet (randomized corruption tests only); no crash/power-loss simulation; no concurrent-access locking between processes; image files can be modified by other programs while open.
