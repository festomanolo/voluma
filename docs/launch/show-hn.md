# Show HN

## Title options (pick one, under 80 characters)
1. Show HN: VOLUMA, a Mac app that writes to NTFS with its own Rust driver
2. Show HN: Read and write NTFS, ext4 and exFAT on a Mac, free and open source
3. Show HN: I wrote filesystem drivers in Rust and let ntfs-3g's checker judge them

Recommended: 1 (specific, verifiable, no hype).

## URL
https://festomanolo.com/voluma/   (or the GitHub repo: https://github.com/festomanolo/voluma)

## First comment (post immediately after submitting)

Hi HN, I'm Festo. macOS reads NTFS but will not write to it, and I kept running into Windows drives and Linux disks I could not touch. VOLUMA is a native macOS app (SwiftUI over a Rust engine) with its own filesystem drivers:

- Read and write: NTFS, exFAT, FAT12/16/32
- Read: ext2/3/4, APFS, HFS+

To write to an NTFS drive it unmounts the volume from macOS, starts a small privileged helper (one admin prompt) that serves only that device over a user-only socket, writes with the Rust NTFS driver, then remounts. It refuses volumes that are dirty, hibernated (Fast Startup) or have an unclean $LogFile.

How I tried to make that trustworthy instead of just saying it is:

- Every driver is judged by someone else's implementation. NTFS images are created and checked by ntfs-3g (ntfsresize -i -f walks every MFT record and the cluster bitmap; ntfscat reads each file back through its own index search). ext4 images come from mke2fs and must pass e2fsck. FAT, exFAT, HFS+ and APFS are checked against fsck and real macOS mounts.
- That caught real bugs. NTFS run lengths are signed, so a 171 cluster file whose length byte is 0xAB looks like a negative number to ntfs-3g. My encoder did not sign-extend and my own tests passed anyway. The independent checker did not.
- The benchmark caught a quadratic FAT read (it re-walked the cluster chain on every call). FAT32 went from 25 to 1,842 MB/s reads.
- Writes run on 150 deliberately corrupted volumes in the suite, to check nothing panics.

Numbers (Intel i5-10310U, disk images, warm cache, so this is driver efficiency, not drive speed): NTFS write 2,140 MB/s vs 199 for ntfs-3g's ntfscp; scanning a 195 GB volume with 3.3M files takes 50 s vs 243 s for du. It loses somewhere too: macOS creates many small files on exFAT about 3x faster than I do. Charts and raw JSON are on the site.

Honest limits: NTFS writing is verified against ntfs-3g, not against Windows chkdsk, so back up anything you cannot replace. The app is not notarised yet. The universal build was built and tested on an Intel Mac; I have not run the Apple Silicon half on real hardware. No compressed or encrypted NTFS files, hard links or alternate streams on write. The privileged helper is launched from the app bundle, which is development grade; the proper fix is a Developer ID signed helper installed with SMAppService.

Install: `pip install nolo-voluma` then `voluma`, or the zip from the releases page. MIT licensed. I would especially like bug reports from real Windows and Linux drives.

Source: https://github.com/festomanolo/voluma

## Prepared answers for likely comments
- **Why not macFUSE + ntfs-3g?** That works, but it needs a system extension you approve and can break across macOS updates. VOLUMA needs no kernel or system extension. The tradeoff: you work inside its window, it does not mount the drive in Finder.
- **Why not Paragon or Tuxera?** They are commercial drivers that mount NTFS read-write in Finder, which is more convenient. VOLUMA is free and open source and does not integrate with Finder.
- **Is it safe?** It is checked against ntfs-3g and corrupted images, and it refuses unsafe volumes. It has not been checked against Windows chkdsk. I would not run it on the only copy of something yet.
- **How does it write without a kernel driver?** It unmounts the volume and talks to the raw partition itself through the helper, so the Rust driver is the only writer.
- **Rust or Swift?** Rust for the drivers and the helper, SwiftUI for the app, joined by a small JSON-over-C-ABI boundary.
- **Why is read so fast?** It reads the image straight from the page cache. On a real drive you are limited by the drive. The numbers are for comparing drivers, not disks.
- **Apple Silicon?** The download is universal but I could only test Intel so far. If you try it on an M-series Mac, please tell me what happens.
- **Windows or Linux version?** The engine is portable Rust; the helper and the UI are macOS today.
