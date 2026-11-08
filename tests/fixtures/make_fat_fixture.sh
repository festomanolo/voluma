#!/bin/sh
# Usage: make_fat_fixture.sh OUT.img "MS-DOS FAT32|FAT16|FAT12" SIZE_MB
# Builds a raw FAT image with macOS's own FAT implementation (independent of VOLUMA),
# then fills it with edge-case content. Requires macOS (hdiutil).
set -eu
OUT="$1"; FS="$2"; MB="$3"
W="$(mktemp -d)"; trap 'hdiutil detach "$W/mnt" -force >/dev/null 2>&1 || true; rm -rf "$W"' EXIT
hdiutil create -size "${MB}m" -fs "$FS" -volname FIXTURE -layout NONE -type UDIF "$W/a.dmg" >/dev/null
mkdir "$W/mnt"
hdiutil attach "$W/a.dmg" -mountpoint "$W/mnt" -nobrowse >/dev/null
M="$W/mnt"
printf 'hello voluma\n' > "$M/README.TXT"
printf 'long name\n' > "$M/A Rather Long File Name With Spaces.txt"
printf 'unicode\n' > "$M/héllo-日本語.txt"
printf 'lower\n' > "$M/lowercase.txt"
mkdir -p "$M/docs/nested/deep" "$M/empty-dir"
printf 'nested\n' > "$M/docs/nested/deep/file.bin"
i=0; while [ $i -lt 40 ]; do printf 'file %s\n' $i > "$M/docs/f$i.txt"; i=$((i+1)); done
# 3 MB deterministic multi-cluster file (skipped for tiny FAT12 volumes)
if [ "$MB" -ge 8 ]; then
  /usr/bin/perl -e 'for my $i (0..(3*1024*1024/16)-1){ print pack("a16", sprintf("%015d\n",$i)); }' > "$M/big.bin"
fi
sync
hdiutil detach "$M" >/dev/null
hdiutil convert "$W/a.dmg" -format UDTO -o "$W/out" >/dev/null
mv "$W/out.cdr" "$OUT"
