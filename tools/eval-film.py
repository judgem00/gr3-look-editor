"""Camera test of a fitted film (docs/plan-spektrafilm-fitter.md, phase 6).

Per scene: a Standard shot and shots with the test looks, same exposure.
core/examples/film_eval.rs renders every shot from its own DNG (own white
balance; input scale, tone gain and lens shading fitted on the Standard shot):
the shot's look as the core models it, and the LUT on its scene colours. No
alignment is needed: render and camera JPEG are the same frame. Compared on
flat 6x6 blocks of the camera JPEG (not clipped), ΔE2000:

    model      camera JPEG vs the core's model of the look (fine match: the
               Standard shot's residual and highlight chroma applied)
    film       camera JPEG vs the LUT -- how close the camera gets to the film
    fit        model vs LUT -- the fit's own error on this scene

Needs numpy, Pillow and colour-science (e.g. the spektrafilm venv).

usage: python -I tools/eval-film.py DIR PRESET LUT OUT STD,SHOT,SHOT,... [STD,SHOT,...]
    (shot names without .DNG, the Standard shot first in each scene)
"""
import io
import json
import os
import subprocess
import sys

import colour
import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
CORE = os.path.join(HERE, "..", "core")
BIN = os.path.join(CORE, "target", "release", "examples", "film_eval")


def tiff_jpeg(path):
    """the largest embedded JPEG (the camera JPEG)"""
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
    return best


def lab(rgb01):
    lin = colour.cctf_decoding(np.clip(rgb01, 0, 1), "sRGB")
    return colour.XYZ_to_Lab(colour.RGB_to_XYZ(lin, "sRGB"))


def blocks(a, n=6):
    h, w = a.shape[0] // n * n, a.shape[1] // n * n
    v = a[:h, :w].reshape(h // n, n, w // n, n, 3)
    return v.mean((1, 3)).reshape(-1, 3), v.std((1, 3)).max(-1).reshape(-1)


def fine(g, info):
    """the preview's fine match (app.js render): residual per channel, then highlight chroma"""
    res = np.array(info["residual"], float)
    f = np.clip(g, 0, 1) * 255
    i = np.minimum(np.floor(f), 254).astype(int)
    g = res[i] + (res[i + 1] - res[i]) * (f - i)
    ch = np.array(info["highlight_chroma"], float)
    y = (np.clip(g, 0, 1) * [0.299, 0.587, 0.114]).sum(-1, keepdims=True)
    f = np.clip(y * 16 - 0.5, 0, 15)
    i = np.minimum(np.floor(f), 14).astype(int)
    k = ch[i] + (ch[i + 1] - ch[i]) * (f - i)
    return y + (g - y) * k


def scene(dirname, preset, lut, out, names):
    sizes = {}
    dngs = [os.path.join(dirname, f"{n}.DNG") for n in names]
    for line in subprocess.run([BIN, "size", *dngs], capture_output=True, text=True, check=True).stdout.splitlines():
        f, w, h, _ = line.rsplit(" ", 3)
        sizes[f] = (int(w), int(h))
    pairs = []
    os.makedirs(out, exist_ok=True)
    for d in dngs:
        im = tiff_jpeg(d).convert("RGBA").resize(sizes[d], Image.LANCZOS)
        r = os.path.join(out, os.path.basename(d) + ".rgba")
        open(r, "wb").write(im.tobytes())
        pairs.append(f"{d}={r}")
    subprocess.run([BIN, "render", out, preset, lut, *pairs], check=True)
    info = json.load(open(os.path.join(out, "shots.json")))
    print(f"\nscene {names[0]}: gain {info['gain']}, scale {info['scale']:.0f}")
    print(f"  {'shot':10s} {'look':14s} {'blocks':>6s}   {'model':>12s} {'model fine':>12s} {'film':>12s} {'fit':>12s}   (ΔE2000 mean / p95)")
    rows = {}
    for s in info["shots"]:
        w, h, n = s["width"], s["height"], s["name"]
        cam = np.fromfile(os.path.join(out, f"{n}.camera.u8"), np.uint8).reshape(h, w, 3) / 255.0
        lutv = np.fromfile(os.path.join(out, f"{n}.lut.f32"), np.float32).reshape(h, w, 3).astype(float)
        m = 8
        C, sd = blocks(cam[m:-m, m:-m])
        flat = (sd < 2.5 / 255) & (C.min(1) > 4 / 255) & (C.max(1) < 250 / 255)
        L = blocks(lutv[m:-m, m:-m])[0][flat]
        C = C[flat]
        lc = lab(C)
        d = lambda a, b=None: colour.delta_E(lab(a), lc if b is None else lab(b), method="CIE 2000")
        r = {"film": d(L)}
        if s["model"]:
            mod = np.fromfile(os.path.join(out, f"{n}.model.f32"), np.float32).reshape(h, w, 3).astype(float)
            M = blocks(mod[m:-m, m:-m])[0][flat]
            r["model"], r["model fine"], r["fit"] = d(M), d(blocks(fine(mod, info)[m:-m, m:-m])[0][flat]), d(M, L)
        cell = lambda k: f"{r[k].mean():5.2f} / {np.percentile(r[k], 95):4.1f}" if k in r else f"{'–':>12s}"
        print(f"  {n:10s} {s['look']:14s} {flat.sum():6d}   {cell('model')} {cell('model fine')} {cell('film')} {cell('fit')}")
        rows[n] = {"look": s["look"], "blocks": int(flat.sum()),
                   **{k: {"mean": float(v.mean()), "p95": float(np.percentile(v, 95))} for k, v in r.items()}}
    return {"gain": info["gain"], "scale": info["scale"], "shots": rows}


def main():
    dirname, preset, lut, out = sys.argv[1:5]
    report = {}
    for group in sys.argv[5:]:
        names = group.split(",")
        report[names[0]] = scene(dirname, preset, lut, os.path.join(out, names[0]), names)
    json.dump(report, open(os.path.join(out, "eval-film.json"), "w"), indent=1)


if __name__ == "__main__":
    main()
