#!/bin/sh
# Correctness tests for FastTransfer (the parallel copy/move engine): content, metadata, symlinks, name clashes,
# same-volume rename/clone, cross-volume move, FAT/exFAT sidecars. Builds the harness, makes a scratch exFAT image.
set -eu
cd "$(dirname "$0")/../.."
W=$(mktemp -d); OUT="$W/ft"; trap 'hdiutil detach "$W/mnt" >/dev/null 2>&1 || true; rm -rf "$W"' EXIT
OUT="$OUT" tests/bench/fasttransfer.sh
FT="$OUT"; fail() { echo "FAIL: $*"; exit 1; }

# a source tree: many small files, a big file (chunked path), a symlink, an xattr, odd permissions and mtime
mkdir -p "$W/src/tree/a/b" "$W/dst"
i=0; while [ $i -lt 300 ]; do head -c $((i * 37 + 1)) /dev/urandom > "$W/src/tree/a/f$i.dat"; i=$((i + 1)); done
head -c 40000000 /dev/urandom > "$W/src/tree/a/b/big.bin"; : > "$W/src/tree/empty"
ln -s a/b/big.bin "$W/src/tree/link"; xattr -w com.test.x hello "$W/src/tree/empty"
touch -t 202001010101 "$W/src/tree/a/b/big.bin"; chmod 640 "$W/src/tree/a/b/big.bin"

echo "1. same-volume copy (clone) keeps content, links, xattrs, mtime, mode"
"$FT" copy "$W/src/tree" "$W/dst" | grep -q "errors 0" || fail copy
diff -r "$W/src/tree" "$W/dst/tree" || fail content
[ "$(readlink "$W/dst/tree/link")" = "a/b/big.bin" ] || fail symlink
[ "$(xattr -p com.test.x "$W/dst/tree/empty")" = hello ] || fail xattr
[ "$(stat -f %m "$W/dst/tree/a/b/big.bin")" = "$(stat -f %m "$W/src/tree/a/b/big.bin")" ] || fail mtime
[ "$(stat -f %Lp "$W/dst/tree/a/b/big.bin")" = 640 ] || fail mode

echo "2. name clash never overwrites"
"$FT" copy "$W/src/tree" "$W/dst" >/dev/null; [ -d "$W/dst/tree copy" ] || fail clash
"$FT" copy "$W/src/tree" "$W/dst" >/dev/null; [ -d "$W/dst/tree copy 2" ] || fail clash2

echo "3. copying a folder into itself is refused"
if "$FT" copy "$W/dst/tree" "$W/dst/tree" >/dev/null; then fail into-itself; fi

echo "4. same-volume move is a rename"
mkdir "$W/mv"; "$FT" move "$W/dst/tree copy 2" "$W/mv" | grep -q "renamed 1" || fail rename
[ ! -e "$W/dst/tree copy 2" ] && [ -d "$W/mv/tree copy 2" ] || fail moved

echo "5. cross-volume copy and move (exFAT image)"
hdiutil create -size 200m -fs ExFAT -volname FTTEST -layout NONE "$W/x.dmg" >/dev/null
mkdir "$W/mnt"; hdiutil attach "$W/x.dmg" -mountpoint "$W/mnt" -nobrowse >/dev/null
"$FT" copy "$W/src/tree" "$W/mnt" | grep -q "errors 0" || fail copy-exfat
# exFAT keeps the xattr in a "._empty" sidecar file, which is expected; everything else must match
if diff -r "$W/src/tree" "$W/mnt/tree" 2>&1 | grep -v "Directory loop" | grep -v '\._empty$' | grep .; then fail content-exfat; fi
cmp "$W/src/tree/a/b/big.bin" "$W/mnt/tree/a/b/big.bin" || fail bigfile
cp -R "$W/src/tree" "$W/src/tree2"
"$FT" move "$W/mnt/tree" "$W/dst" | grep -q "errors 0" || fail move-back
[ ! -e "$W/mnt/tree" ] || fail source-removed
find "$W/dst/tree copy 3" -name '._*' | grep -q . && fail sidecars-left || true
diff -r "$W/src/tree2" "$W/dst/tree copy 3" || fail move-back-content
echo "all FastTransfer tests passed"
