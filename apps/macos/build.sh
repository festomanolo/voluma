#!/bin/sh
# Builds the Rust engine static library, then the SwiftUI app linked against it.
set -e
cd "$(dirname "$0")/../.."
cargo build --release -p voluma-ffi
cd apps/macos
swift build -c release -Xlinker -L"$(cd ../.. && pwd)/target/release" "$@"
