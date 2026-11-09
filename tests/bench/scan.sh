#!/bin/bash
# Folder-scan benchmark: VOLUMA's scanner (the Storage Lens engine) vs `du`, same folders, same warm cache.
# Usage: tests/bench/scan.sh OUT.json LABEL=PATH[:RUNS] ...
set -u
OUT="$1"; shift
HERE="$(cd "$(dirname "$0")/../.." && pwd)"; W="$(mktemp -d /private/tmp/voluma-scan.XXXXXX)"; trap 'rm -rf "$W"' EXIT
cp "$HERE/tests/bench/scan_main.swift" "$W/main.swift"   # top-level code must live in main.swift
swiftc -O -o "$W/scan" "$HERE/apps/macos/Sources/Voluma/Core/StorageScanner.swift" "$HERE/apps/macos/Sources/Voluma/Core/DiskDiscovery.swift" "$W/main.swift" 2>/dev/null || { echo "swiftc failed" >&2; exit 1; }
: > "$W/lines"
for spec in "$@"; do
  label="${spec%%=*}"; rest="${spec#*=}"; path="${rest%%:*}"; runs="${rest##*:}"; [ "$runs" = "$rest" ] && runs=3
  echo "== $label ($path, $runs runs)" >&2
  "$W/scan" "$path" > /dev/null   # warm the metadata cache for both tools
  for i in $(seq 1 "$runs"); do
    out=$(/usr/bin/time -l "$W/scan" "$path" 2> "$W/t")
    rss=$(awk '/maximum resident/ {print $1}' "$W/t")
    python3 - "$label" "$out" "$rss" >> "$W/lines" <<'PY'
import json, sys
o = json.loads(sys.argv[2]); o.update({"label": sys.argv[1], "tool": "VOLUMA", "peak_rss_mb": round(int(sys.argv[3]) / 1048576)})
print(json.dumps(o))
PY
    s=$(python3 -c "import time,subprocess;t=time.perf_counter();o=subprocess.run(['du','-sk','$path'],capture_output=True,text=True).stdout.split()[0];print(round(time.perf_counter()-t,3),int(o)*1024)")
    set -- $s 2>/dev/null; python3 -c "import json;print(json.dumps({'label':'$label','tool':'du','seconds':$1,'bytes':$2}))" >> "$W/lines"
  done
done
python3 - "$W/lines" "$OUT" <<'PY'
import json, statistics, sys, subprocess
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
out = {}
for r in rows:
    d = out.setdefault(r["label"], {}).setdefault(r["tool"], {"seconds": [], "bytes": r["bytes"]})
    d["seconds"].append(r["seconds"])
    if r["tool"] == "VOLUMA": d.update(files=r["files"], peak_rss_mb=r["peak_rss_mb"], unreadable=r["unreadable"])
res = [{"label": k, **{t: {**v, "median": statistics.median(v["seconds"])} for t, v in tools.items()}} for k, tools in out.items()]
json.dump({"results": res}, open(sys.argv[2], "w"), indent=1)
print(json.dumps(res)[:300])
PY
