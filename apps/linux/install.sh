#!/bin/sh
# Installs VOLUMA into a Linux system root (default: /, or pass DESTDIR=/path/to/rootcopy).
set -e

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$HERE/../.." && pwd)"
DESTDIR="${1:-${DESTDIR:-/}}"
DESTDIR="${DESTDIR%/}"

DIST_DIR="$HERE/dist"
if [ ! -f "$DIST_DIR/libvoluma_ffi.so" ] || [ ! -f "$DIST_DIR/voluma-helper" ]; then
    sh "$HERE/build.sh"
fi

mkdir -p \
    "$DESTDIR/usr/local/bin" \
    "$DESTDIR/usr/local/lib/voluma" \
    "$DESTDIR/usr/share/applications" \
    "$DESTDIR/usr/share/pixmaps" \
    "$DESTDIR/usr/share/icons/hicolor/128x128/apps"

cp -f "$HERE/voluma" "$DESTDIR/usr/local/bin/voluma"
cp -f "$DIST_DIR/voluma-helper" "$DESTDIR/usr/local/bin/voluma-helper"
cp -f "$DIST_DIR/libvoluma_ffi.so" "$DESTDIR/usr/local/lib/voluma/libvoluma_ffi.so"
cp -f "$HERE/voluma.desktop" "$DESTDIR/usr/share/applications/voluma.desktop"
chmod 0755 "$DESTDIR/usr/local/bin/voluma" "$DESTDIR/usr/local/bin/voluma-helper"

if [ -f "$REPO_DIR/site/assets/icon-1024.png" ]; then
    cp -f "$REPO_DIR/site/assets/icon-1024.png" "$DESTDIR/usr/share/pixmaps/voluma.png"
    cp -f "$REPO_DIR/site/assets/icon-1024.png" "$DESTDIR/usr/share/icons/hicolor/128x128/apps/voluma.png"
fi

echo "[VOLUMA Linux] Installed into ${DESTDIR:-/}"
