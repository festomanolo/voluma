# I wrote an NTFS driver in Rust and let ntfs-3g's checker try to break it

Publish this on https://festomanolo.com first, then cross-post to dev.to. Add the demo GIF and the benchmark chart.

---

macOS can read NTFS drives. It will not write to them. Every few months that wall hits me: a Windows backup drive, a client's disk, a USB stick formatted by someone else. The usual answers are a paid driver, or macFUSE plus ntfs-3g and a system extension you have to approve and re-approve. I wanted a third option: a free app with no kernel extension. So I wrote one. It is called VOLUMA, and this is the story of how I tried to make its filesystem code trustworthy.

## The trust problem

A filesystem driver that writes to a disk can destroy data. And a driver tested only by its author proves very little, because the same misunderstanding that produces a bug also produces the test that passes it.

So I made a rule: no driver is judged by tests I wrote alone. Something else has to say the result is correct.

- **NTFS:** `mkntfs` creates the image, the real ntfs-3g library populates it, and after every kind of write I run `ntfsresize -i -f`, which walks every MFT record and cross-checks the cluster bitmap. `ntfscat` then reads each file back through ntfs-3g's own B-tree search, so my index ordering is verified by someone else's code.
- **ext4:** `mke2fs -d` builds the image from a host folder, and `e2fsck -fn` must call it clean. The host folder is the ground truth.
- **FAT, exFAT, HFS+, APFS:** macOS creates the volumes, and macOS `fsck` and a real mount judge my writes and reads.

## What the independent checker caught

**1. A signed length.** NTFS stores where a file lives as "data runs": a length and an offset, packed into the fewest bytes. I packed the length as an unsigned number. But ntfs-3g reads it as signed. A file of 171 clusters has a length byte of `0xAB`, which has its top bit set, so ntfs-3g saw a negative length and declared the whole volume corrupt:

```
ERROR(5): ntfs_decompress_mapping_pairs: Input/output error
```

My own tests passed, because my reader and my writer agreed with each other. The fix is one line: a length whose top byte has the high bit set needs an extra `0x00` byte. There is now a regression test, and I confirmed it fails if I put the bug back.

**2. Two files, one set of clusters.** On volumes created by ntfs-3g without `$Secure`, a directory's security descriptor can be a separate non-resident attribute. I copied it into each new file as raw bytes, runs included. Every new file then claimed the same two clusters. `ntfsresize` reported "Cluster 3075 is referenced multiple times" and showed me exactly which files.

**3. A root that never got written.** When an index root overflowed, I pushed its entries down into a new block and returned as soon as that block fit, without writing the new, smaller root. The tree looked fine until a later insert, when 12 entries had vanished.

None of these would have shown up in a test I wrote alone.

## The benchmark found one too

I benchmarked every driver against the reference tool for that filesystem. FAT32 reads came out at 25 MB/s while macOS did 2,000. The cause: each 1 MiB read walked the cluster chain from the start of the file, which is quadratic. Resuming from a cursor and reading contiguous clusters in one go took FAT32 to 1,842 MB/s, and batching cluster allocation took writes from 32 to 583 MB/s. macOS `fsck_msdos` still passes.

## How it writes to a real drive

You cannot open a raw device without root, so VOLUMA starts a small helper, once, with an administrator prompt. The helper serves exactly one device over a Unix socket that only your user can open, and touches files only by name inside a staging folder, with `O_NOFOLLOW`. Before any of that, the app unmounts the volume from macOS so nothing else is writing, and it checks the NTFS dirty bit, the journal and Windows hibernation, refusing if any of them says the drive is not in a clean state.

## Numbers, and where I lose

On disk images with a warm cache (so this measures driver efficiency, not how fast a disk spins): scanning a 195 GB volume with 3.3 million files takes 50 s against 243 s for `du`; writing a 256 MB file to NTFS runs at 2,140 MB/s against 199 for ntfs-3g's copy tool. And one honest loss: macOS creates 2,000 small files on exFAT about three times faster than I do. The charts and the raw JSON are on the site.

## What it does not do yet

NTFS writing is verified against ntfs-3g, **not** against Windows `chkdsk`, so back up anything you cannot replace. It is not notarised. The universal build was tested on an Intel Mac only. No compressed or encrypted NTFS files, no hard links or alternate streams on write, no Btrfs, XFS or ZFS.

## Try it

```bash
pip install nolo-voluma
voluma
```

Source (MIT): https://github.com/festomanolo/voluma · Website and benchmarks: https://festomanolo.com/voluma/

If you try it on a real Windows or Linux drive, or on Apple Silicon, I would love to hear what happens.
