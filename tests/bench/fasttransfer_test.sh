#!/bin/sh
# Correctness tests for FastTransfer (the parallel copy/move engine): content, metadata, symlinks, name clashes,
# same-volume rename/clone, cross-volume move, FAT/exFAT sidecars. Builds the harness, makes a scratch exFAT image.
set -eu
cd "$(dirname "$0")/../.."
