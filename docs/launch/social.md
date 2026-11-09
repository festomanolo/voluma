# X, Mastodon, LinkedIn, Bluesky

## Thread (X / Mastodon / Bluesky)
1. macOS can read NTFS drives but will not write to them. I built a free app that does, and it also reads ext4, APFS and HFS+. VOLUMA, open source. [demo GIF: site/assets/img/demo.gif, also https://raw.githubusercontent.com/festomanolo/voluma/main/site/assets/img/demo.gif]
2. How writing works: it unmounts the volume, asks for your password once, writes with its own Rust NTFS driver, then gives the drive back. It refuses drives Windows left dirty or hibernated.
3. I did not want "trust me" as the safety story, so every driver is judged by someone else's implementation: ntfs-3g checks the NTFS volumes, e2fsck checks ext4, macOS fsck checks FAT and exFAT.
4. That caught a real bug my own tests missed: NTFS data run lengths are signed, so a 171 cluster file looked like a negative length to ntfs-3g. There is now a regression test for it.
5. The benchmark caught another: my FAT driver re-walked the cluster chain on every read. FAT32 reads went from 25 to 1,842 MB/s after the fix.
6. Numbers, on disk images with a warm cache: scanning 3.3M files takes 50 s vs 243 s for du. It loses on small files on exFAT (macOS is about 3x faster). All charts and raw data are on the site.
7. Honest limits: NTFS write is verified against ntfs-3g, not Windows chkdsk. Not notarised yet. Apple Silicon half untested on real hardware. Back up anything you cannot replace.
8. `pip install nolo-voluma` then type `voluma`. https://festomanolo.com/voluma/ · https://github.com/festomanolo/voluma

## LinkedIn
I just released VOLUMA, a free, open source macOS app that reads and writes NTFS, exFAT and FAT drives and browses ext4, APFS and HFS+.

The part I am proudest of is not a feature, it is the testing. Every filesystem driver is judged by an independent implementation (ntfs-3g, e2fsprogs, macOS fsck), because a driver that only passes its author's tests proves very little. That approach found two real bugs my own tests missed, and a benchmark found a third that made FAT32 reads 70x slower than they should be.

It is honest about its limits too: NTFS writing is verified against ntfs-3g but not Windows chkdsk, and the app is not notarised yet.

Install: pip install nolo-voluma, then type voluma. Details and benchmarks: https://festomanolo.com/voluma/
