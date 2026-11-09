#!/bin/sh
# Builds the VOLUMA Rust engine (libvoluma_ffi.so) and privileged helper (voluma-helper) for Linux x86_64.
# On Linux, builds natively with cargo; on macOS, cross-builds using Docker (linux/amd64).
set -e

REPO_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
OUT_DIR="$REPO_DIR/apps/linux/dist"
mkdir -p "$OUT_DIR"

if [ "$(uname -s)" = "Linux" ]; then
    cd "$REPO_DIR"
    cargo build --release -p voluma-ffi -p voluma-helper
    cp -f "$REPO_DIR/target/release/libvoluma_ffi.so" "$OUT_DIR/libvoluma_ffi.so"
    cp -f "$REPO_DIR/target/release/voluma-helper" "$OUT_DIR/voluma-helper"
else
    WORK="$HOME/.cache/voluma-linux-build"
    CARGO_CACHE="$HOME/.cache/voluma-cargo-cache"
    mkdir -p "$WORK" "$CARGO_CACHE"
    cp -f "$REPO_DIR/Cargo.toml" "$REPO_DIR/Cargo.lock" "$WORK/"
    rm -rf "$WORK/crates"
    cp -R "$REPO_DIR/crates" "$WORK/crates"
    docker run --rm --platform linux/amd64 \
        -v "$WORK:/work" \
        -v "$CARGO_CACHE:/usr/local/cargo/registry" \
        -w /work rust:1-slim-bookworm \
        cargo build --release -p voluma-ffi -p voluma-helper
    cp -f "$WORK/target/release/libvoluma_ffi.so" "$OUT_DIR/libvoluma_ffi.so"
    cp -f "$WORK/target/release/voluma-helper" "$OUT_DIR/voluma-helper"
fi

cp -f "$REPO_DIR/apps/linux/voluma" "$OUT_DIR/voluma"
cp -f "$REPO_DIR/apps/linux/voluma.desktop" "$OUT_DIR/voluma.desktop"
chmod +x "$OUT_DIR/voluma" "$OUT_DIR/voluma-helper"

echo "[VOLUMA Linux] Built in $OUT_DIR:"
ls -lh "$OUT_DIR"
