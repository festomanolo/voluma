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
