# Reddit

Read each subreddit's rules before posting. Some require a weekly self-promotion thread or an established account.

## r/rust
**Title:** I wrote NTFS, ext4, APFS, HFS+, FAT and exFAT drivers in Rust and used ntfs-3g to fail my tests

**Body:**
I built VOLUMA, a macOS app on a Rust storage engine (`fs-ntfs`, `fs-ext`, `fs-apfs`, `fs-hfsplus`, `fs-fat`, `fs-exfat`, plus a `BlockDevice` trait, partition parsing, and a small privileged helper).

The part I think is interesting for this sub is how I tested the drivers. A driver that passes tests written by the same person who wrote the driver proves very little, so every driver is judged by another implementation:

- NTFS: images made by `mkntfs`, populated by the real ntfs-3g library, then `ntfsresize -i -f` (walks every MFT record and the cluster bitmap), `ntfscat` and `ntfsls` after each kind of write.
- ext4: `mke2fs -d` images that must pass `e2fsck`.
- FAT, exFAT, HFS+, APFS: macOS `fsck` and a real read-only mount, compared byte for byte.

It caught things my tests missed. NTFS data runs encode the length as a signed integer, so a 171 cluster file with a one byte length of 0xAB reads as negative. My encoder passed my own round trip tests and ntfs-3g rejected the volume. There is now a regression test that fails if you reintroduce the bug.

Other bits: the NTFS index B-tree insert and split path, MFT growth, a corruption fuzz that runs writes on 150 damaged volumes, and a helper that serves exactly one `/dev/disk` over a peer-uid-checked Unix socket.

Numbers, caveats and charts: https://festomanolo.com/voluma/#benchmarks
Source (MIT): https://github.com/festomanolo/voluma

Limits: NTFS write is verified against ntfs-3g, not Windows chkdsk. Feedback on the driver design and the test approach is very welcome.

## r/macapps and r/MacOS
**Title:** I made a free app that writes to NTFS drives on a Mac (and reads ext4, APFS, HFS+)

**Body:**
macOS can read NTFS but not write to it, so Paste is greyed out on every Windows drive. VOLUMA is a native app that fixes that without macFUSE or a kernel extension.

- Open the drive, press Enable Writing, paste. It unmounts the volume from macOS, asks for your password once, writes with its own NTFS driver, and gives the drive back when you finish.
- It refuses drives Windows left dirty or hibernated.
- Also: read ext2/3/4 (Linux drives), APFS and HFS+; read and write exFAT and FAT; copy between any two drives and with Finder; a Storage Lens that shows where your space went (3.3M files in about 50 s); eject external drives from the same screen.
- Install: `pip install nolo-voluma` then `voluma`, or download the zip from the releases page.

[demo GIF: site/assets/img/demo.gif, also https://raw.githubusercontent.com/festomanolo/voluma/main/site/assets/img/demo.gif]

Honest bits: it is not notarised yet (right-click Open the first time, or install with pip), NTFS writing is verified against ntfs-3g but not Windows chkdsk so back up important drives, and it works inside its own window rather than mounting in Finder. Free and open source (MIT).

https://festomanolo.com/voluma/ · https://github.com/festomanolo/voluma

## r/DataHoarder (and r/HomeLab)
**Title:** Free Mac app to read your Linux (ext4) drives and write to your Windows (NTFS) drives

**Body:**
If you move disks between Mac, Windows and Linux boxes, you know macOS cannot read ext4 and will not write to NTFS. VOLUMA does both, read-only for ext2/3/4, APFS and HFS+, and read/write for NTFS, exFAT and FAT.

For a Linux drive: it appears as an unmounted partition, press Browse, enter your password, copy files out. It never writes to ext4. For a Windows drive: Enable Writing, paste.

It checks before writing: dirty bit, NTFS journal, Windows hibernation. It refuses an ext4 volume that needs journal recovery. NTFS writes are verified against ntfs-3g's own checker, but not against Windows chkdsk, so keep a backup of anything irreplaceable.

How to guides: https://festomanolo.com/voluma/guides/read-ext4-linux-drive-on-mac/ and https://festomanolo.com/voluma/guides/write-to-ntfs-on-mac/
Source: https://github.com/festomanolo/voluma
