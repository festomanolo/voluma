#!/bin/bash
# Builds a universal (arm64 + x86_64) VOLUMA.app with the privileged helper inside, ad-hoc signs it, and zips it.
# Not notarised: users open it once via right-click, or clear the quarantine flag (see the download page).
# Usage: apps/macos/package.sh [OUT_DIR]   -> OUT_DIR/Voluma-<version>-macos-universal.zip (+ .sha256)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"; OUT="${1:-$ROOT/target/dist}"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)"/\1/')"
cd "$ROOT"
for T in aarch64-apple-darwin x86_64-apple-darwin; do cargo build --release --target "$T" -p voluma-ffi -p voluma-helper; done
UNI="$ROOT/target/universal"; mkdir -p "$UNI"
lipo -create target/aarch64-apple-darwin/release/libvoluma_ffi.a target/x86_64-apple-darwin/release/libvoluma_ffi.a -output "$UNI/libvoluma_ffi.a"
lipo -create target/aarch64-apple-darwin/release/voluma-helper target/x86_64-apple-darwin/release/voluma-helper -output "$UNI/voluma-helper"
cd apps/macos
swift build -c release --arch arm64 --arch x86_64 -Xlinker -L"$UNI"
EXE="$(swift build -c release --arch arm64 --arch x86_64 --show-bin-path)/Voluma"
APP="$OUT/VOLUMA.app"; rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Helpers" "$APP/Contents/Resources"
cp "$EXE" "$APP/Contents/MacOS/Voluma"; cp "$UNI/voluma-helper" "$APP/Contents/Helpers/voluma-helper"
# icon
ICONSET="$(mktemp -d)/AppIcon.iconset"; mkdir -p "$ICONSET"; SRC="$ROOT/site/assets/icon-1024.png"
for s in 16 32 128 256 512; do sips -z $s $s "$SRC" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null; sips -z $((s*2)) $((s*2)) "$SRC" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null; done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
 <key>CFBundleName</key><string>VOLUMA</string>
 <key>CFBundleDisplayName</key><string>VOLUMA</string>
 <key>CFBundleIdentifier</key><string>com.festomanolo.voluma</string>
 <key>CFBundleExecutable</key><string>Voluma</string>
 <key>CFBundleIconFile</key><string>AppIcon</string>
 <key>CFBundlePackageType</key><string>APPL</string>
 <key>CFBundleShortVersionString</key><string>$VERSION</string>
 <key>CFBundleVersion</key><string>$VERSION</string>
 <key>LSMinimumSystemVersion</key><string>14.0</string>
 <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
 <key>NSHighResolutionCapable</key><true/>
 <key>NSHumanReadableCopyright</key><string>Copyright 2026 Festo Kasalu Magembe. MIT licence.</string>
</dict></plist>
PLIST
codesign --force --sign - --timestamp=none "$APP/Contents/Helpers/voluma-helper"
codesign --force --sign - --timestamp=none --deep "$APP"
ZIP="$OUT/Voluma-$VERSION-macos-universal.zip"; rm -f "$ZIP"
(cd "$OUT" && ditto -c -k --keepParent VOLUMA.app "$ZIP")
(cd "$OUT" && shasum -a 256 "$(basename "$ZIP")" > "$ZIP.sha256")
lipo -info "$APP/Contents/MacOS/Voluma"; lipo -info "$APP/Contents/Helpers/voluma-helper"
codesign --verify --deep --strict "$APP" && echo "signature ok (ad-hoc)"
ls -la "$ZIP"; cat "$ZIP.sha256"
