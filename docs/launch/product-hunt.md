# Product Hunt

Launch Tuesday to Thursday at 12:01am Pacific. Line up a few people to try it and comment honestly on launch day. Do not ask for upvotes in public posts.

**Name:** VOLUMA
**Tagline (60 chars):** Read and write every drive on your Mac
**Alternative:** Write to NTFS on Mac, free and open source

**Description (260 chars):**
A native macOS explorer that writes to NTFS, exFAT and FAT and browses ext4, APFS and HFS+. Copy between drives, see where your space went, and eject safely. Free, open source, installs with pip install nolo-voluma.

**Topics:** Mac, Developer Tools, Open Source, Productivity

**Gallery (in order):** hero Disks view; the NTFS read-only banner; Enable Writing dialog; the writing session; Storage Lens; benchmark chart. Use `site/assets/img/*.webp` converted to PNG.

**Maker's first comment:**
Hi, I'm Festo. I built VOLUMA because every external drive I own is in a different filesystem and macOS only really speaks some of them. It writes to NTFS (the thing people ask about most), reads ext4 Linux drives, and shows where your disk space went.

What I would like feedback on: real Windows and Linux drives, Apple Silicon Macs (I could only test Intel), and the NTFS write flow. It is not notarised yet, and NTFS writing is verified against ntfs-3g rather than Windows chkdsk, so please back up anything you cannot replace.

Install with `pip install nolo-voluma` then `voluma`, or the zip from the site.

**Links:** https://festomanolo.com/voluma/ · https://github.com/festomanolo/voluma
