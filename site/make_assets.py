#!/usr/bin/env python3
"""Turns the raw app screenshots in assets/raw into the web images, and draws the app icon and social card.
Screenshots are real captures of the app (window-only, 1250x900) taken against demo volumes, never real user data."""
import os, subprocess
from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
RAW, IMG = os.path.join(HERE, "assets/raw"), os.path.join(HERE, "assets/img")
os.makedirs(IMG, exist_ok=True)
ACCENT, INK, BG = (255, 138, 61), (242, 243, 245), (11, 12, 14)

def webp(img, name, q=88):
    tmp = os.path.join(IMG, name + ".png"); img.convert("RGB").save(tmp)
    subprocess.run(["cwebp", "-quiet", "-q", str(q), "-m", "6", tmp, "-o", os.path.join(IMG, name + ".webp")], check=True)
    os.remove(tmp)
    return os.path.getsize(os.path.join(IMG, name + ".webp"))

def shot(name): return Image.open(os.path.join(RAW, name + ".png")).convert("RGBA")
def flat(im):
    bg = Image.new("RGBA", im.size, (244, 245, 247, 255)); bg.alpha_composite(im); return bg

sizes = {}
for src, dst in [("disks", "disks"), ("ntfs-readonly", "ntfs-readonly"), ("ntfs-confirm", "ntfs-confirm"), ("ntfs-writing", "ntfs-writing"), ("ext4-browse", "ext4-browse"), ("lens", "lens")]:
    sizes[dst] = webp(flat(shot(src)), dst)
sizes["disks-tree"] = webp(flat(shot("disks-expanded-full")).crop((222, 100, 1250, 760)), "disks-tree")
sizes["lens-crop"] = webp(flat(shot("lens")).crop((222, 112, 1250, 196)), "lens-crop")

# ---- app icon: graphite tile, concentric arcs like tracks on a platter, solid hub
def icon(px=1024):
    k = 4; S = px * k
    im = Image.new("RGBA", (S, S), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
    tile = Image.new("RGBA", (S, S)); td = ImageDraw.Draw(tile)
    for y in range(S):
        t = y / S; td.line([(0, y), (S, y)], fill=(int(30 - 14 * t), int(32 - 15 * t), int(37 - 17 * t), 255))
    mask = Image.new("L", (S, S), 0); ImageDraw.Draw(mask).rounded_rectangle([0, 0, S - 1, S - 1], radius=int(S * .225), fill=255)
    im.paste(tile, (0, 0), mask)
    cx = cy = S // 2
    # three tracks, each almost a full turn with its gap at a different angle, so it reads as a spinning platter
    for i, (r, w) in enumerate([(0.36, 0.06), (0.27, 0.055), (0.18, 0.05)]):
        R, W = int(S * r), int(S * w)
        start = -70 + 125 * i
        d.arc([cx - R, cy - R, cx + R, cy + R], start=start, end=start + 292, fill=ACCENT + (255,), width=W)
    hub = int(S * .06); d.ellipse([cx - hub, cy - hub, cx + hub, cy + hub], fill=ACCENT + (255,))
    hole = int(S * .02); d.ellipse([cx - hole, cy - hole, cx + hole, cy + hole], fill=(20, 22, 26, 255))
    return im.resize((px, px), Image.LANCZOS)

ic = icon(1024)
for px, name in [(512, "icon-512.png"), (180, "apple-touch-icon.png"), (128, "icon-128.png"), (56, "icon-56.png")]:
    ic.resize((px, px), Image.LANCZOS).save(os.path.join(IMG, name))
ic.save(os.path.join(HERE, "assets/icon-1024.png"))
open(os.path.join(IMG, "favicon.svg"), "w").write(
 '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="14" fill="#16181c"/>'
 '<g fill="none" stroke="#ff8a3d" stroke-width="3.6" stroke-linecap="butt"><path d="M51.2 40A21 21 0 1 0 22.5 50.8"/><path d="M42 44A15 15 0 0 1 21 38.5"/><path d="M37.5 33.5A9.5 9.5 0 1 0 25.5 40.5"/></g><circle cx="32" cy="32" r="3.8" fill="#ff8a3d"/></svg>')

# ---- social card 1200x630
def font(size, weight="Bold"):
    for path in ("/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/Helvetica.ttc"):
        try:
            f = ImageFont.truetype(path, size)
            try: f.set_variation_by_name(weight)
            except Exception: pass
            return f
        except Exception: continue
    return ImageFont.load_default()

W, H = 1200, 630
og = Image.new("RGB", (W, H), BG)
g = Image.new("RGB", (W, H)); gd = ImageDraw.Draw(g)
for x in range(W):
    t = x / W; gd.line([(x, 0), (x, H)], fill=(int(11 + 12 * t), int(12 + 13 * t), int(14 + 16 * t)))
og.paste(g)
glow = Image.new("RGBA", (W, H), (0, 0, 0, 0)); ImageDraw.Draw(glow).ellipse([560, 60, 1260, 700], fill=ACCENT + (46,))
og = Image.alpha_composite(og.convert("RGBA"), glow.filter(ImageFilter.GaussianBlur(90)))
win = flat(shot("disks")).convert("RGB")
tw = 780; win = win.resize((tw, int(win.height * tw / win.width)), Image.LANCZOS)
m = Image.new("L", win.size, 0); ImageDraw.Draw(m).rounded_rectangle([0, 0, win.width - 1, win.height - 1], radius=26, fill=255)
sh = Image.new("RGBA", (win.width + 120, win.height + 120), (0, 0, 0, 0)); ImageDraw.Draw(sh).rounded_rectangle([60, 70, win.width + 60, win.height + 70], radius=26, fill=(0, 0, 0, 170))
sh = sh.filter(ImageFilter.GaussianBlur(28))
px, py = 560, 118
og.alpha_composite(sh, (px - 60, py - 60)); og.paste(win, (px, py), m)
d = ImageDraw.Draw(og)
og.alpha_composite(ic.resize((64, 64), Image.LANCZOS), (64, 64))
d.text((144, 76), "VOLUMA", font=font(34, "Bold"), fill=INK)
y = 190
for line in ["Read and write", "every drive on", "your Mac"]:
    d.text((64, y), line, font=font(70, "Bold"), fill=INK); y += 82
d.text((64, 470), "NTFS, exFAT and FAT read and write.", font=font(27, "Regular"), fill=(155, 162, 173))
d.text((64, 508), "ext4, APFS and HFS+ browse.", font=font(27, "Regular"), fill=(155, 162, 173))
d.rounded_rectangle([64, 566, 330, 606], radius=20, fill=ACCENT); d.text((92, 573), "Free for macOS", font=font(24, "Semibold"), fill=(27, 15, 4))
og.convert("RGB").save(os.path.join(IMG, "og-image.png"), optimize=True)
print({k: f"{v/1024:.0f} KB" for k, v in sizes.items()}, "og", os.path.getsize(os.path.join(IMG, "og-image.png")) // 1024, "KB")
