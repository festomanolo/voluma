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
