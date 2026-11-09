#!/bin/bash
# Filesystem driver benchmarks. Everything runs on throwaway disk images in a temp dir; nothing touches a real disk.
# Reads and writes go through the page cache on both sides, so these numbers compare driver efficiency, not media speed.
# Usage: tests/bench/run.sh OUT.json   (needs macOS hdiutil; VOLUMA_NTFS3G / VOLUMA_E2FSPROGS for NTFS and ext4)
set -u
OUT="${1:-bench-results.json}"
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
BENCH="$HERE/target/release/examples/bench"
W="$(mktemp -d /private/tmp/voluma-bench.XXXXXX)"
MB=$((1024*1024)); PAYLOAD_MB=256; N_FILES=2000; REPS=3
trap 'hdiutil detach "$W/mnt" -force >/dev/null 2>&1; rm -rf "$W"' EXIT
: > "$W/lines.jsonl"
log() { printf '%s\n' "$1" >> "$W/lines.jsonl"; }
tool() { for d in "" bin sbin; do [ -x "$1/$d/$2" ] && { echo "$1/$d/$2"; return; }; done; }
head -c $((PAYLOAD_MB*MB)) /dev/urandom > "$W/payload.bin"

# seconds for a command, via python so we get sub-ms resolution without GNU time
timeit() { python3 - "$@" <<'PY'
import subprocess, sys, time
t = time.perf_counter(); subprocess.run(sys.argv[1:], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); print(f"{time.perf_counter()-t:.4f}")
PY
}
pycopy_many() { python3 - "$1" "$2" <<'PY'
import os, sys, time
d, n = sys.argv[1], int(sys.argv[2]); os.makedirs(d + "/bench", exist_ok=True)
t = time.perf_counter()
for i in range(n):
    with open(f"{d}/bench/file-{i:05d}.txt", "w") as f: f.write(f"small file {i}\n")
print(f"{time.perf_counter()-t:.4f}")
PY
}
emit() { # name tool metric value unit
  log "{\"fs\":\"$1\",\"tool\":\"$2\",\"metric\":\"$3\",\"value\":$4,\"unit\":\"$5\"}"; }
mbps() { python3 -c "print(round($PAYLOAD_MB*1.048576/$1,1))"; }   # MiB payload / seconds -> MB/s (decimal)
fps() { python3 -c "print(round($N_FILES/$1))"; }

native_fs() { # label hdiutilfs kind canwrite
  local label="$1" hfs="$2" kind="$3" canwrite="$4"
  echo "== $label" >&2
  rm -f "$W"/*.dmg "$W"/*.cdr "$W"/clone*.raw; mkdir -p "$W/mnt"
  hdiutil create -size 1300m -fs "$hfs" -volname BENCH -layout NONE -type UDIF "$W/full.dmg" >/dev/null
  hdiutil create -size 1300m -fs "$hfs" -volname BENCH -layout NONE -type UDIF "$W/empty.dmg" >/dev/null
  # native write timings and the payload files for the read test
  hdiutil attach "$W/full.dmg" -mountpoint "$W/mnt" -nobrowse >/dev/null
  if [ "$canwrite" = 1 ]; then
    for i in 1 2 3; do t=$(timeit cp "$W/payload.bin" "$W/mnt/payload.$i"); emit "$label" macOS write_mb_s "$(mbps $t)" MB/s; done
  else cp "$W/payload.bin" "$W/mnt/payload.1"; fi
  sync; hdiutil detach "$W/mnt" >/dev/null
  hdiutil convert "$W/full.dmg" -format UDTO -o "$W/full" >/dev/null; mv "$W/full.cdr" "$W/full.raw"
  # native read
  hdiutil attach "$W/full.raw" -imagekey diskimage-class=CRawDiskImage -readonly -nobrowse -mountpoint "$W/mnt" >/dev/null
  for i in 1 2 3; do t=$(timeit dd if="$W/mnt/payload.1" of=/dev/null bs=1m); emit "$label" macOS read_mb_s "$(mbps $t)" MB/s; done
  hdiutil detach "$W/mnt" >/dev/null
  # VOLUMA read
  for i in 1 2 3; do emit "$label" VOLUMA read_mb_s "$("$BENCH" read "$W/full.raw" "$kind" /payload.1 | python3 -c 'import json,sys;print(json.load(sys.stdin)["mb_s"])')" MB/s; done
  if [ "$canwrite" = 1 ]; then
    hdiutil attach "$W/empty.dmg" -mountpoint "$W/mnt" -nobrowse >/dev/null
    t=$(pycopy_many "$W/mnt" $N_FILES); emit "$label" macOS small_files_s "$(fps $t)" files/s
    sync; hdiutil detach "$W/mnt" >/dev/null
    hdiutil attach "$W/empty.dmg" -mountpoint "$W/mnt" -nobrowse >/dev/null; hdiutil detach "$W/mnt" >/dev/null
    hdiutil create -size 1300m -fs "$hfs" -volname BENCH -layout NONE -type UDIF "$W/e2.dmg" >/dev/null
    hdiutil convert "$W/e2.dmg" -format UDTO -o "$W/e2" >/dev/null; mv "$W/e2.cdr" "$W/empty.raw"
    for i in 1 2 3; do cp -c "$W/empty.raw" "$W/clone$i.raw"; emit "$label" VOLUMA write_mb_s "$("$BENCH" write "$W/clone$i.raw" "$kind" /payload.bin "$W/payload.bin" | python3 -c 'import json,sys;print(json.load(sys.stdin)["mb_s"])')" MB/s; rm -f "$W/clone$i.raw"; done
    cp -c "$W/empty.raw" "$W/clone.raw"; emit "$label" VOLUMA small_files_s "$("$BENCH" many "$W/clone.raw" "$kind" $N_FILES | python3 -c 'import json,sys;print(json.load(sys.stdin)["files_s"])')" files/s; rm -f "$W/clone.raw"
  fi
  rm -f "$W"/*.dmg "$W"/*.raw
}

want() { [ -z "${ONLY:-}" ] || [[ " $ONLY " == *" $1 "* ]]; }   # ONLY="FAT32 ext4" runs a subset
want FAT32 && native_fs "FAT32" "MS-DOS FAT32" fat 1
want exFAT && native_fs "exFAT" "ExFAT" exfat 1
want HFS+ && native_fs "HFS+" "HFS+" hfs 0
want APFS && native_fs "APFS" "APFS" apfs 0

if [ -n "${VOLUMA_E2FSPROGS:-}" ] && want ext4; then
  echo "== ext4" >&2
  E="$(tool "$VOLUMA_E2FSPROGS" mke2fs)"; D="$(tool "$VOLUMA_E2FSPROGS" debugfs)"
  mkdir -p "$W/src" && cp "$W/payload.bin" "$W/src/payload.bin"
  "$E" -q -t ext4 -d "$W/src" "$W/ext.img" 700M >/dev/null 2>&1
  for i in 1 2 3; do t=$(timeit "$D" -R "dump /payload.bin /dev/null" "$W/ext.img"); emit "ext4" "e2fsprogs debugfs" read_mb_s "$(mbps $t)" MB/s; done
  for i in 1 2 3; do emit "ext4" VOLUMA read_mb_s "$("$BENCH" read "$W/ext.img" ext /payload.bin | python3 -c 'import json,sys;print(json.load(sys.stdin)["mb_s"])')" MB/s; done
  rm -rf "$W/src" "$W/ext.img"
fi

if [ -n "${VOLUMA_NTFS3G:-}" ] && want NTFS; then
  echo "== NTFS" >&2
  MK="$(tool "$VOLUMA_NTFS3G" mkntfs)"; POP="$(tool "$VOLUMA_NTFS3G" ntfs_populate)"; CAT="$(tool "$VOLUMA_NTFS3G" ntfscat)"; CP="$(tool "$VOLUMA_NTFS3G" ntfscp)"
  mkdir -p "$W/src" && cp "$W/payload.bin" "$W/src/payload.bin"
  dd if=/dev/zero of="$W/ntfs.raw" bs=1m count=0 seek=1300 2>/dev/null; "$MK" -F -Q -L BENCH "$W/ntfs.raw" >/dev/null 2>&1; "$POP" "$W/ntfs.raw" "$W/src"
  for i in 1 2 3; do t=$(timeit "$CAT" -f "$W/ntfs.raw" /payload.bin); emit "NTFS" "ntfs-3g ntfscat" read_mb_s "$(mbps $t)" MB/s; done
  hdiutil attach "$W/ntfs.raw" -imagekey diskimage-class=CRawDiskImage -readonly -nobrowse -mountpoint "$W/mnt" >/dev/null 2>&1 && {
    for i in 1 2 3; do t=$(timeit dd if="$W/mnt/payload.bin" of=/dev/null bs=1m); emit "NTFS" "macOS (read-only)" read_mb_s "$(mbps $t)" MB/s; done; hdiutil detach "$W/mnt" >/dev/null; }
  for i in 1 2 3; do emit "NTFS" VOLUMA read_mb_s "$("$BENCH" read "$W/ntfs.raw" ntfs /payload.bin | python3 -c 'import json,sys;print(json.load(sys.stdin)["mb_s"])')" MB/s; done
  rm -f "$W/ntfs.raw"
  dd if=/dev/zero of="$W/empty.raw" bs=1m count=0 seek=1300 2>/dev/null; "$MK" -F -Q -L BENCH "$W/empty.raw" >/dev/null 2>&1
  for i in 1 2 3; do cp -c "$W/empty.raw" "$W/clone.raw"; t=$(timeit "$CP" -f "$W/clone.raw" "$W/payload.bin" /payload.bin); emit "NTFS" "ntfs-3g ntfscp" write_mb_s "$(mbps $t)" MB/s; rm -f "$W/clone.raw"; done
  for i in 1 2 3; do cp -c "$W/empty.raw" "$W/clone.raw"; emit "NTFS" VOLUMA write_mb_s "$("$BENCH" write "$W/clone.raw" ntfs /payload.bin "$W/payload.bin" | python3 -c 'import json,sys;print(json.load(sys.stdin)["mb_s"])')" MB/s; rm -f "$W/clone.raw"; done
  cp -c "$W/empty.raw" "$W/clone.raw"; emit "NTFS" VOLUMA small_files_s "$("$BENCH" many "$W/clone.raw" ntfs $N_FILES | python3 -c 'import json,sys;print(json.load(sys.stdin)["files_s"])')" files/s; rm -f "$W/clone.raw"
fi

python3 - "$W/lines.jsonl" "$OUT" <<'PY'
import json, statistics, sys, platform, subprocess
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
agg = {}
for r in rows: agg.setdefault((r["fs"], r["tool"], r["metric"], r["unit"]), []).append(r["value"])
out = [{"fs": k[0], "tool": k[1], "metric": k[2], "unit": k[3], "median": statistics.median(v), "runs": v} for k, v in agg.items()]
meta = {"payload_mb": 256, "small_files": 2000, "reps": 3, "machine": subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip(),
        "os": platform.mac_ver()[0], "note": "disk images, warm page cache: compares driver efficiency, not media speed"}
json.dump({"meta": meta, "results": out}, open(sys.argv[2], "w"), indent=1)
print(open(sys.argv[2]).read()[:200])
PY
