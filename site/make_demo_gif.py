#!/usr/bin/env python3
"""Builds assets/img/demo.gif from a frame capture of the real app (see docs/launch/README.md for how it was recorded).
Usage: make_demo_gif.py FRAMES_DIR OUT.gif   (FRAMES_DIR holds f_NNNN.png, f_NNNN.png.t timestamps and markers.txt)"""
import glob, os, sys
from PIL import Image, ImageChops, ImageDraw, ImageFont, ImageStat

src, out = sys.argv[1], sys.argv[2]
W = int(os.environ.get("GIF_W", 860)); COLORS = int(os.environ.get("GIF_COLORS", 96))
frames = sorted(glob.glob(os.path.join(src, "f_*.png")))
times = [float(open(f + ".t").read()) for f in frames]
marks = []
for line in open(os.path.join(src, "markers.txt")):
    t, rest = line.strip().split(" ", 1); n, txt = rest.split("|", 1); marks.append((float(t), n, txt))

def font(size, weight="Semibold"):
    for p in ("/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/Helvetica.ttc"):
        try:
            f = ImageFont.truetype(p, size)
            try: f.set_variation_by_name(weight)
            except Exception: pass
            return f
        except Exception: pass
    return ImageFont.load_default()

def caption(im, text, n):
    if not text: return im
    im = im.convert("RGBA"); ov = Image.new("RGBA", im.size, (0, 0, 0, 0)); d = ImageDraw.Draw(ov)
    f = font(22); tw = d.textlength(text, font=f); pad = 18
    box_w = tw + pad * 2 + 44; x0 = (im.width - box_w) / 2; y0 = im.height - 62
    d.rounded_rectangle([x0, y0, x0 + box_w, y0 + 42], radius=21, fill=(20, 22, 26, 230))
    d.ellipse([x0 + 10, y0 + 8, x0 + 36, y0 + 34], fill=(255, 138, 61, 255))
    nf = font(17, "Bold"); d.text((x0 + 23 - d.textlength(n, font=nf) / 2, y0 + 12), n, font=nf, fill=(27, 15, 4, 255))
    d.text((x0 + 44 + pad - 6, y0 + 8), text, font=f, fill=(242, 243, 245, 255))
    return Image.alpha_composite(im, ov).convert("RGB")

start = marks[0][0] - 0.2
kept = []   # (image, t)
prev_small = None
for f, t in zip(frames, times):
    if t < start: continue
    im = Image.open(f).convert("RGB"); im = im.resize((W, int(im.height * W / im.width)), Image.LANCZOS)
    small = im.resize((120, 86))
    if prev_small is not None and sum(ImageStat.Stat(ImageChops.difference(small, prev_small)).mean) < float(os.environ.get("GIF_DIFF", 0.06)): continue
    prev_small = small
    m = [x for x in marks if x[0] <= t]
    n, txt = (m[-1][1], m[-1][2]) if m else ("", "")
    kept.append((caption(im, txt, n), t))
print("frames kept:", len(kept), "of", len(frames))

# one shared palette from a sample of the kept frames, so colours do not flicker between frames
sample = Image.new("RGB", (W, kept[0][0].height * min(len(kept), 6)))
for i, (im, _) in enumerate(kept[:: max(1, len(kept) // 6)][:6]): sample.paste(im, (0, i * im.height))
pal = sample.quantize(colors=COLORS, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
imgs = [im.quantize(palette=pal, dither=Image.Dither.NONE) for im, _ in kept]
dur = []
for i, (_, t) in enumerate(kept):
    nxt = kept[i + 1][1] if i + 1 < len(kept) else t + 2.2
    dur.append(int(max(0.08, min(2.4, nxt - t)) * 1000))
dur[-1] = 2600
imgs[0].save(out, save_all=True, append_images=imgs[1:], duration=dur, loop=0, optimize=True, disposal=1)
kept[0][0].save(out.replace(".gif", "-poster.png"))
print(out, os.path.getsize(out) // 1024, "KB,", sum(dur) / 1000, "s")
