"""Highlight range of a shot series (docs/plan-highlight-correction.md, step 5).

Every shot is measured in its own frame, so nothing needs aligning: scene EV
from its own RAW (linear, lens shading corrected, normalised to the first
shot's exposure by shutter time and highlight correction, which reads out
one stop lower), over the reference grey (the chart's background on the
first shot); camera JPEG luma per block, binned over scene EV in 1/8 EV.
Where the RAW clips, the scene EV is unknown: a shot's curve ends there.

usage: python -I tools/eval-highlight.py DIR LUT VIGNETTE SHOT,SHOT,...
    VIGNETTE: the lens shading fitted by film_eval (shots.json "vignette"), as a,b,c
"""
import io
import subprocess
import sys

import numpy as np
import rawpy
from PIL import Image

STEP = 8                          # block size on the half-size image
CROP = (0.17, 0.82, 0.15, 0.9)    # the screen in the frame: y0, y1, x0, x1 (fractions)
BINS = np.arange(-1, 5.51, 0.125)


def camera_jpeg(path, size):
    d = open(path, "rb").read()
    best, i = None, d.find(b"\xff\xd8\xff")
    while i >= 0:
        try:
            im = Image.open(io.BytesIO(d[i:]))
            if best is None or im.size[0] > best.size[0]:
                im.load()
                best = im
        except Exception:
            pass
        i = d.find(b"\xff\xd8\xff", i + 3)
    return np.asarray(best.convert("RGB").resize(size, Image.LANCZOS), float) / 255


def meta(path):
    out = subprocess.run(["exiftool", "-s3", "-n", "-ExposureTime", "-DynamicRangeExpansion", path],
                         capture_output=True, text=True, check=True).stdout.split("\n")
    return float(out[0]), out[1].split()[0] == "1"


def linear(path, vig):
    with rawpy.imread(path) as r:
        a = r.postprocess(half_size=True, gamma=(1, 1), no_auto_bright=True, output_bps=16,
                          use_camera_wb=True, user_flip=0).astype(float) / 65535
    h, w = a.shape[:2]
    y, x = np.indices((h, w))
    r2 = ((x + 0.5 - w / 2) ** 2 + (y + 0.5 - h / 2) ** 2) / ((w * w + h * h) / 4)
    gain = 1 + r2 * (vig[0] + r2 * (vig[1] + r2 * vig[2]))
    return a, a * gain[..., None]


def blocks(a):
    h, w = a.shape[0] // STEP * STEP, a.shape[1] // STEP * STEP
    v = a[:h, :w].reshape(h // STEP, STEP, w // STEP, STEP, -1)
    return v.mean((1, 3)), v.std((1, 3)).max(-1), v.max((1, 3)).max(-1)


def lut_grey(path):
    rows = [l.split() for l in open(path) if l[:1] in "0123456789.-" and len(l.split()) == 3]
    n = round(len(rows) ** (1 / 3))
    lo, span = -8.0, 12.0
    for l in open(path):
        if l.startswith("# GR-INPUT: grlog"):
            lo, span = map(float, l.split()[3:5])
    d = np.array([list(map(float, rows[k + k * n + k * n * n])) for k in range(n)])
    return lambda e: np.interp(e, lo + span * np.arange(n) / (n - 1), d[:, 1])


def grey_of(Y):
    """the chart background: the most common level on the screen, at least 3 EV under its white"""
    h, w = Y.shape
    c = np.log2(np.maximum(Y[int(CROP[0] * h):int(CROP[1] * h), int(CROP[2] * w):int(CROP[3] * w)], 1e-6))
    top = np.percentile(c, 99.5)
    hist, e = np.histogram(c[c < top - 3], bins=200)
    hist = np.convolve(hist, np.ones(9) / 9, "same")
    k = np.argmax(hist)
    return 2 ** ((e[k] + e[k + 1]) / 2), 2 ** top


def main():
    dirname, lut, vig, names = sys.argv[1], sys.argv[2], [float(v) for v in sys.argv[3].split(",")], sys.argv[4].split(",")
    g = lut_grey(lut)
    t0, hc0 = meta(f"{dirname}/{names[0]}.DNG")
    curves, notes, grey = {}, {}, None
    for n in names:
        path = f"{dirname}/{n}.DNG"
        t, hc = meta(path)
        k = (t0 / t) * (2.0 if hc else 1.0) / (2.0 if hc0 else 1.0)   # to the first shot's exposure
        raw, lin = linear(path, vig)
        Y = (lin @ [0.2126, 0.7152, 0.0722]) * k
        if grey is None:
            grey, white = grey_of(Y)
            print(f"reference grey (linear, {names[0]}): {grey:.4f}; brightest on screen {np.log2(white / grey):+.2f} EV")
        luma = camera_jpeg(path, (Y.shape[1], Y.shape[0])) @ [0.299, 0.587, 0.114]
        Yb, sd, _ = blocks((Y / grey)[..., None])
        _, _, rawmax = blocks(raw)
        Lb, lsd, _ = blocks(luma[..., None])
        Yb, Lb = Yb[..., 0], Lb[..., 0]
        hh, ww = Yb.shape
        by, bx = np.indices(Yb.shape)
        inside = (by >= CROP[0] * hh) & (by < CROP[1] * hh) & (bx >= CROP[2] * ww) & (bx < CROP[3] * ww)
        clipped = rawmax >= 0.98
        ok = inside & ~clipped & (sd < 0.04 * np.maximum(Yb, 0.05)) & (lsd < 0.02)
        ev = np.log2(np.maximum(Yb, 1e-6))
        idx = np.digitize(ev[ok], BINS)
        curves[n] = np.array([np.median(Lb[ok][idx == j]) * 255 if (idx == j).sum() >= 5 else np.nan
                              for j in range(1, len(BINS))])
        top = np.percentile(ev[ok], 99.9)
        notes[n] = (hc, t, k, 100 * (inside & clipped).sum() / inside.sum(), top)
    print(f"\nJPEG luma (0-255) over scene EV (relative to the reference grey)")
    print(f"{'EV':>6s} " + " ".join(f"{n:>9s}" for n in names) + "   LUT grey")
    for j, b in enumerate(BINS[:-1]):
        c = b + 0.0625
        if all(np.isnan(curves[n][j]) for n in names):
            continue
        cells = " ".join(f"{curves[n][j]:9.1f}" if not np.isnan(curves[n][j]) else f"{'':9s}" for n in names)
        print(f"{c:+6.2f} {cells}   {255 * g(c):6.1f}")
    print()
    for n in names:
        hc, t, k, clip, top = notes[n]
        v = curves[n]
        hit = np.where(v >= 250)[0]
        reach = f"reaches 250 at {BINS[hit[0]] + 0.0625:+.2f} EV" if len(hit) else "stays below 250"
        print(f"{n}: HC {'on ' if hc else 'off'} 1/{1 / t:.0f} s (x{k:.2f}), RAW clipped on screen {clip:4.1f} %, "
              f"data up to {top:+.2f} EV, {reach}")


if __name__ == "__main__":
    main()
