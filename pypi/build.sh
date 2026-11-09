#!/bin/bash
# Builds the nolo-voluma wheel + sdist around the app bundle produced by apps/macos/package.sh.
# Usage: pypi/build.sh   -> pypi/dist/*.whl and *.tar.gz   (upload with: twine upload pypi/dist/*)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(dirname "$HERE")"
APP="$ROOT/target/dist/VOLUMA.app"
[ -d "$APP" ] || { echo "run apps/macos/package.sh first" >&2; exit 1; }
rm -rf "$HERE/src/nolo_voluma/VOLUMA.app" "$HERE/dist" "$HERE/build" "$HERE"/src/*.egg-info
ditto "$APP" "$HERE/src/nolo_voluma/VOLUMA.app"
cd "$HERE" && python3 -m build
python3 -m twine check dist/*
ls -la dist
