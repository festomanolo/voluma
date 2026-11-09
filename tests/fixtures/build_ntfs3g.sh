#!/bin/sh
# Builds ntfs-3g's tools (mkntfs, ntfsresize, ntfsls, ntfscat, ...) plus tests/fixtures/ntfs_populate
# into PREFIX (default ./target/ntfs3g). Then: export VOLUMA_NTFS3G=PREFIX
set -eu
PREFIX="${1:-$(pwd)/target/ntfs3g}"; HERE="$(cd "$(dirname "$0")" && pwd)"
VER=2022.10.3
W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT
curl -fsSL -o "$W/n.tgz" "https://tuxera.com/opensource/ntfs-3g_ntfsprogs-$VER.tgz"
mkdir "$W/src"; tar xzf "$W/n.tgz" -C "$W/src" --strip-components=1
cd "$W/src"
./configure --prefix="$PREFIX" --disable-ntfs-3g --enable-ntfsprogs --disable-library --disable-mount-helper \
  --disable-crypto --disable-extras --without-uuid --disable-nfconv --disable-static >/dev/null
make -j"$(sysctl -n hw.ncpu 2>/dev/null || nproc)" >/dev/null
make install >/dev/null
cc -O1 -DHAVE_CONFIG_H -I. -Iinclude/ntfs-3g "$HERE/ntfs_populate.c" -o "$PREFIX/ntfs_populate" libntfs-3g/.libs/libntfs-3g.a
echo "installed to $PREFIX; run: export VOLUMA_NTFS3G=$PREFIX"
