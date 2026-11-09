# VOLUMA for macOS

Read and write every drive on your Mac. VOLUMA is a native macOS app that **writes to NTFS**, exFAT and FAT drives and **browses ext4, APFS and HFS+**, copies between drives (and with Finder), and shows where your disk space went.

```bash
pip install nolo-voluma
voluma
```

![VOLUMA listing a Mac's disks, partitions and free space](https://festomanolo.com/voluma/assets/img/og-image.png)

## What you get

- **Write to NTFS** on drives macOS mounts read-only. VOLUMA unmounts the volume, asks for your administrator password once, writes with its own NTFS driver, and gives the drive back to macOS. It refuses drives Windows left dirty or hibernated.
- **Read** ext2/3/4, APFS and HFS+. **Read and write** exFAT and FAT12/16/32.
- **Storage Lens**: scans 3.3 million files in about 50 seconds (4.8x faster than `du` in our test), by type, with Finder-style drill-down.
- **Copy and paste** between any two drives. A name clash never overwrites.
- **Eject** external drives from the same screen.

## Commands

| Command | What it does |
|---|---|
| `voluma` | open the app |
| `voluma install` | copy `VOLUMA.app` to `/Applications` (`--user` for `~/Applications`) |
| `voluma where` | print where the bundled app lives |
| `voluma --version` | print the version |

pip does not mark the files it installs as downloaded from the internet, so macOS normally opens the app without the "unidentified developer" warning that a downloaded zip triggers. If you do see one, right-click the app and choose Open.

## Requirements

macOS 14 or later, Apple Silicon or Intel (universal build). The Apple Silicon half was built and tested on an Intel Mac and has not yet been run on real Apple Silicon hardware.

## Be careful with NTFS writes

NTFS writing is checked against ntfs-3g's own consistency tools and against deliberately corrupted volumes, but **not against Windows chkdsk**. Back up anything you cannot replace.

## Links

- Website, benchmarks and guides: https://festomanolo.com/voluma/
- Source and releases: https://github.com/festomanolo/voluma
- How to write to NTFS on a Mac: https://festomanolo.com/voluma/guides/write-to-ntfs-on-mac/

MIT licensed.
