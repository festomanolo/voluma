#!/usr/bin/env python3
"""Builds the deployable /voluma/ folder for festomanolo.com from index.src.html.

Every number on the page comes from data/*.json (written by tests/bench/*.sh and the packaging step); nothing is typed
by hand. Charts are plain HTML bars, so they read without JavaScript.

    python3 build.py ../../festomanolo/voluma
"""
import html, json, os, re, shutil, sys

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "dist"))
ICONS = os.environ.get("PHOSPHOR_DIR", "")            # .../package/assets/regular
e = html.escape

fs = json.load(open(os.path.join(HERE, "data/fs.json")))
scan = json.load(open(os.path.join(HERE, "data/scan.json")))["results"]
dist = json.load(open(os.path.join(HERE, "data/dist.json")))
META = fs["meta"]
R = {}
for r in fs["results"]:
    R[(r["fs"], r["tool"], r["metric"])] = r["median"]

def v(fsn, tool, metric): return R.get((fsn, tool, metric))

def icon(name, cls="ico"):
    svg = open(os.path.join(ICONS, name + ".svg")).read().strip()
    return svg.replace("<svg ", f'<svg class="{cls}" aria-hidden="true" focusable="false" ', 1)

def fmt_mb(x):
    return f"{x:,.0f}" if x >= 100 else f"{x:.1f}"

def group(label, rows, unit, scale, fmt=fmt_mb, note=None):
    out = [f'<div class="grp"><div class="grp-label">{e(label)}</div>']
    for who, val, us in rows:
        if val is None: continue
        w = max(0.6, 100 * val / scale)
        out.append(f'<div class="row"><span class="who">{e(who)}</span><span class="track"><span class="bar{" us" if us else ""}" style="--w:{w:.1f}%"></span><span class="val">{fmt(val)} <i>{e(unit)}</i></span></span></div>')
    out.append("</div>")
    return "".join(out)

def chart(title, sub, groups_html, note="", wide=False, aria=""):
    return (f'<figure class="chart{" wide" if wide else ""}" role="group" aria-label="{e(aria or title)}">'
            f'<h3>{e(title)}</h3><p class="sub">{e(sub)}</p>{groups_html}'
            + (f'<p class="note">{note}</p>' if note else "") + "</figure>")

# ---- read throughput (reference = a tool that reads the same raw image, else macOS)
reads = []
all_reads = [v(f, t, "read_mb_s") for f, t in [("FAT32","VOLUMA"),("exFAT","VOLUMA"),("HFS+","VOLUMA"),("APFS","VOLUMA"),("ext4","VOLUMA"),("NTFS","VOLUMA")]]
scale = max(x for x in all_reads if x) * 1.0
refs = {"FAT32": ("macOS", "macOS"), "exFAT": ("macOS", "macOS"), "HFS+": ("macOS", "macOS"), "APFS": ("macOS", "macOS"),
        "ext4": ("e2fsprogs debugfs", "e2fsprogs"), "NTFS": ("ntfs-3g ntfscat", "ntfs-3g")}
for name in ["NTFS", "ext4", "FAT32", "exFAT", "HFS+", "APFS"]:
    rt, rlabel = refs[name]
    reads.append(group(name, [("VOLUMA", v(name, "VOLUMA", "read_mb_s"), True), (rlabel, v(name, rt, "read_mb_s"), False)], "MB/s", scale))
read_chart = chart("Read speed", "Streaming a 256 MB file out of the volume. Higher is better.", "".join(reads),
    "macOS reads these test images through its disk-image layer, which adds overhead, so the macOS bars are indicative. ntfs-3g and e2fsprogs read the raw image directly, like VOLUMA does.",
    aria="Read throughput in MB per second for each filesystem")

# ---- write throughput
w_scale = max(x for x in [v("FAT32","VOLUMA","write_mb_s"), v("exFAT","VOLUMA","write_mb_s"), v("NTFS","VOLUMA","write_mb_s"), v("exFAT","macOS","write_mb_s")] if x)
writes = [
    group("NTFS", [("VOLUMA", v("NTFS","VOLUMA","write_mb_s"), True), ("ntfs-3g", v("NTFS","ntfs-3g ntfscp","write_mb_s"), False)], "MB/s", w_scale),
    group("exFAT", [("VOLUMA", v("exFAT","VOLUMA","write_mb_s"), True), ("macOS", v("exFAT","macOS","write_mb_s"), False)], "MB/s", w_scale),
    group("FAT32", [("VOLUMA", v("FAT32","VOLUMA","write_mb_s"), True), ("macOS", v("FAT32","macOS","write_mb_s"), False)], "MB/s", w_scale),
]
write_chart = chart("Write speed", "Copying a 256 MB file into the volume. Higher is better.", "".join(writes),
    "macOS cannot write to NTFS at all, so ntfs-3g's own ntfscp is the reference there.", aria="Write throughput in MB per second")

# ---- small files
sf = [v("FAT32","VOLUMA","small_files_s"), v("FAT32","macOS","small_files_s"), v("exFAT","VOLUMA","small_files_s"), v("exFAT","macOS","small_files_s"), v("NTFS","VOLUMA","small_files_s")]
s_scale = max(x for x in sf if x)
small = [
    group("NTFS", [("VOLUMA", v("NTFS","VOLUMA","small_files_s"), True)], "files/s", s_scale, fmt=lambda x: f"{x:,.0f}"),
    group("exFAT", [("VOLUMA", v("exFAT","VOLUMA","small_files_s"), True), ("macOS", v("exFAT","macOS","small_files_s"), False)], "files/s", s_scale, fmt=lambda x: f"{x:,.0f}"),
    group("FAT32", [("VOLUMA", v("FAT32","VOLUMA","small_files_s"), True), ("macOS", v("FAT32","macOS","small_files_s"), False)], "files/s", s_scale, fmt=lambda x: f"{x:,.0f}"),
]
small_chart = chart("Creating small files", "2,000 tiny files in one folder. Higher is better.", "".join(small),
    "This is where VOLUMA is slower: macOS creates files faster on exFAT. Directory-heavy workloads are the next thing to tune.", aria="Small file creation rate")

# ---- scan
S = {r["label"]: r for r in scan}
def fmt_s(x): return f"{x:.1f}" if x < 100 else f"{x:,.0f}"
names = {"applications": "The Applications folder", "volume": "A whole 195 GB volume"}
sc_scale = max(S[k][t]["median"] for k in S for t in ("VOLUMA", "du"))
scan_groups = []
for k in ("volume", "applications"):
    if k not in S: continue
    d = S[k]; vo, du = d["VOLUMA"], d["du"]
    scan_groups.append(group(f'{names[k]}, {vo["files"]:,} files', [("VOLUMA", vo["median"], True), ("du", du["median"], False)], "s", sc_scale, fmt=fmt_s))
scan_chart = chart("Scanning a folder", "Time to total every file. Lower is better.", "".join(scan_groups),
    "Both tools ran back to back on the same warm folder. VOLUMA's total is the on-disk size, the same quantity du reports.", wide=True, aria="Folder scan time in seconds, VOLUMA against du")

big = S.get("volume") or list(S.values())[0]
vo, du = big["VOLUMA"], big["du"]
ratio_scan = du["median"] / vo["median"]
match = abs(vo["bytes"] - du["bytes"]) / max(du["bytes"], 1)
ntfs_ratio = v("NTFS","VOLUMA","write_mb_s") / v("NTFS","ntfs-3g ntfscp","write_mb_s")
kpis = (
    f'<div class="kpi"><div class="bignum">{vo["files"]/1e6:.1f}M files in {fmt_s(vo["median"])} s</div><p>Storage Lens scanning a whole volume. Peak memory {vo["peak_rss_mb"]} MB.</p></div>'
    f'<div class="kpi"><div class="bignum">{ratio_scan:.1f}&times;</div><p>faster than <span class="mono">du</span> on the same folder. {"Totals agree to the byte." if match == 0 else ("Totals agree to within 0.001%." if match < 1e-5 else f"Totals agree within {100*match:.2f}%.")}</p></div>'
    f'<div class="kpi"><div class="bignum">{ntfs_ratio:.0f}&times;</div><p>faster NTFS writes than ntfs-3g&rsquo;s own copy tool ({fmt_mb(v("NTFS","VOLUMA","write_mb_s"))} vs {fmt_mb(v("NTFS","ntfs-3g ntfscp","write_mb_s"))} MB/s).</p></div>'
)
intro = (f'Measured on an {META["machine"].replace("(R)","").replace("(TM)","").replace(" CPU","")} Mac running macOS {META["os"]}. '
         "Drive tests use disk images and a warm cache, so they compare driver efficiency, not how fast a disk spins. Folder scans use real folders on this Mac.")
method = ('Disk-image tests: a 256 MB random file, the median of three runs (one of which is a cold read), a fresh copy of the image for every write. '
          'Scan tests: one warm-up run, then timed runs of VOLUMA and <span class="mono">du -sk</span> back to back. '
          'Raw numbers: <a href="data/results.json">results.json</a>. The harness ships with the source as <span class="mono">tests/bench</span>.')

# ---- assemble
page = open(os.path.join(HERE, "index.src.html")).read()
page = re.sub(r"\{\{ICON_LG:([a-z-]+)\}\}", lambda m: icon(m.group(1), "ico ico-lg"), page)
page = re.sub(r"\{\{ICON:([a-z-]+)\}\}", lambda m: icon(m.group(1)), page)
sub = {
    "CHARTS": scan_chart + read_chart + '<div class="stack">' + write_chart + small_chart + "</div>", "KPIS": kpis, "BENCH_INTRO": intro, "METHOD": method,
    "VERSION": dist["version"], "DATE": dist["date"], "ZIP_NAME": dist["zip_name"], "ZIP_SIZE": dist["zip_size"], "SHA256": dist["sha256"],
    "TESTS": f'{dist["tests"]:,}', "ARCH_NOTE": dist["arch_note"], "ARCH_FAQ": dist["arch_faq"],
    "SCAN_HEADLINE": f'{vo["files"]/1e6:.1f} million files in {fmt_s(vo["median"])} seconds',
}
for k, val in sub.items(): page = page.replace("{{" + k + "}}", val)
left = re.findall(r"\{\{[A-Z_:a-z-]+\}\}", page)
assert not left, f"unfilled placeholders: {left}"
assert "—" not in page and "–" not in page, "em/en dash found: the page style forbids them"

# ---- guides
GUIDES = [
    ("write-to-ntfs-on-mac", "How to Write to an NTFS Drive on a Mac (Free, No Reformat)",
     "How to write to an NTFS drive on a Mac", "Write to NTFS on Mac",
     "macOS mounts NTFS drives read-only, so Paste is greyed out. Compare every way to write to NTFS on a Mac, then do it free with VOLUMA, no reformatting."),
    ("read-ext4-linux-drive-on-mac", "How to Read an ext4 Linux Drive on a Mac (Free)",
     "How to read an ext4 Linux drive on a Mac", "Read ext4 on Mac",
     "macOS cannot open ext4 Linux drives. Read ext2, ext3 and ext4 on a Mac, browse folders and copy files out safely, read-only, with the free VOLUMA app."),
]
gt = open(os.path.join(HERE, "guide.src.html")).read()
for slug, title, h1, crumb, desc in GUIDES:
    body = open(os.path.join(HERE, "guides", slug + ".body.html")).read().replace("{{DATE}}", dist["date"])
    body = re.sub(r"\{\{ICON:([a-z-]+)\}\}", lambda m: icon(m.group(1)), body)
    g = gt.replace("{{BODY}}", body)
    g = re.sub(r"\{\{ICON:([a-z-]+)\}\}", lambda m: icon(m.group(1)), g)
    for k, val in {"TITLE": title, "H1": h1, "CRUMB": crumb, "DESC": desc, "SLUG": slug, "DATE": dist["date"]}.items():
        g = g.replace("{{" + k + "}}", val)
    assert not re.findall(r"\{\{[A-Za-z_:-]+\}\}", g), slug
    assert "\u2014" not in g and "\u2013" not in g, f"dash in {slug}"
    os.makedirs(os.path.join(OUT, "guides", slug), exist_ok=True)
    open(os.path.join(OUT, "guides", slug, "index.html"), "w").write(g)
GUIDE_URLS = [g[0] for g in GUIDES]

os.makedirs(OUT, exist_ok=True)
for sub_dir in ("assets", "data", "downloads"):
    src = os.path.join(HERE, sub_dir)
    if os.path.isdir(src):
        shutil.copytree(src, os.path.join(OUT, sub_dir), dirs_exist_ok=True, ignore=shutil.ignore_patterns("raw", "icon-1024.png"))
shutil.copy(os.path.join(HERE, "style.css"), os.path.join(OUT, "assets/style.css"))
shutil.copy(os.path.join(HERE, "app.js"), os.path.join(OUT, "assets/app.js"))
results_public = {"meta": META, "drivers": fs["results"], "scan": scan, "version": dist["version"], "generated": dist["date"]}
os.makedirs(os.path.join(OUT, "data"), exist_ok=True)
json.dump(results_public, open(os.path.join(OUT, "data/results.json"), "w"), indent=1)
for stale in ("fs.json", "scan.json", "dist.json"):
    p = os.path.join(OUT, "data", stale)
    if os.path.exists(p): os.remove(p)
open(os.path.join(OUT, "index.html"), "w").write(page)
print("built", OUT, f"({len(page)//1024} KB html)")
