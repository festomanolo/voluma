#!/bin/sh
# Usage: make_mac_fixture.sh OUT.img "HFS+|Case-sensitive HFS+|APFS" SIZE_MB
# Builds a raw image with macOS's own implementation (independent of VOLUMA) and fills it with
# edge-case content: unicode, nested dirs, 600 files (forces a multi-level catalog B-tree),
# a fragmented file (many extents), hard links, a symlink and an empty file.
set -eu
OUT="$1"; FS="$2"; MB="$3"
W="$(mktemp -d)"; trap 'hdiutil detach "$W/mnt" -force >/dev/null 2>&1 || true; rm -rf "$W"' EXIT
hdiutil create -size "${MB}m" -fs "$FS" -volname FIXTURE -layout NONE -type UDIF "$W/a.dmg" >/dev/null
mkdir "$W/mnt"
hdiutil attach "$W/a.dmg" -mountpoint "$W/mnt" -nobrowse >/dev/null
M="$W/mnt"
printf 'hello voluma\n' > "$M/README.TXT"
printf 'unicode\n' > "$M/héllo-日本語.txt"
printf 'mixed\n' > "$M/MixedCase.Txt"
: > "$M/empty.dat"
mkdir -p "$M/docs/nested/deep" "$M/empty-dir" "$M/many"
printf 'nested\n' > "$M/docs/nested/deep/file.bin"
i=0; while [ $i -lt 600 ]; do printf 'file %s\n' $i > "$M/many/a-fairly-long-file-name-number-$i.txt"; i=$((i+1)); done
/usr/bin/perl -e 'for my $i (0..(3*1024*1024/16)-1){ print pack("a16", sprintf("%015d\n",$i)); }' > "$M/big.bin"
# Fragmentation: interleave growth of several files so each ends up with many extents.
j=0; while [ $j -lt 60 ]; do
  for n in 1 2 3; do head -c 40000 /dev/zero | tr '\0' "$n" >> "$M/frag$n.bin"; done
  j=$((j+1))
done
ln "$M/README.TXT" "$M/docs/hardlink-to-readme.txt"
ln -s README.TXT "$M/symlink-to-readme"
sync
hdiutil detach "$M" >/dev/null
hdiutil convert "$W/a.dmg" -format UDTO -o "$W/out" >/dev/null
mv "$W/out.cdr" "$OUT"
