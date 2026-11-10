#!/bin/sh
# Builds the standalone FastTransfer harness at $OUT (default /tmp/fasttransfer).
set -e
cd "$(dirname "$0")/../.."
tmp=$(mktemp -d)
cp tests/bench/fasttransfer_main.swift "$tmp/main.swift"
swiftc -O -swift-version 5 -o "${OUT:-/tmp/fasttransfer}" \
  apps/macos/Sources/Voluma/Core/FastTransfer.swift "$tmp/main.swift"
rm -rf "$tmp"
