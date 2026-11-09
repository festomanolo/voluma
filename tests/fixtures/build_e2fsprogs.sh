#!/bin/sh
# Builds e2fsprogs (mke2fs, e2fsck, debugfs) from source into PREFIX (default: ./target/e2fsprogs)
# so the fs-ext tests have an independent judge. Then: export VOLUMA_E2FSPROGS=PREFIX
set -eu
PREFIX="${1:-$(pwd)/target/e2fsprogs}"
VER=1.47.1
W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT
curl -fsSL -o "$W/e2.tgz" "https://mirrors.edge.kernel.org/pub/linux/kernel/people/tytso/e2fsprogs/v$VER/e2fsprogs-$VER.tar.gz"
mkdir "$W/src"; tar xzf "$W/e2.tgz" -C "$W/src" --strip-components=1
cd "$W/src"
./configure --prefix="$PREFIX" --disable-nls --disable-fuse2fs --disable-uuidd >/dev/null
make -j"$(sysctl -n hw.ncpu 2>/dev/null || nproc)" >/dev/null
make install >/dev/null
echo "installed to $PREFIX; run: export VOLUMA_E2FSPROGS=$PREFIX"
