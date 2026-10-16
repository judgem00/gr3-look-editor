"""Phase 0 of docs/plan-spektrafilm-fitter.md: what does fitting against a 3D LUT
of a spektrafilm film cost, compared with fitting against spektrafilm itself?

Per film: the target normalised like a print scan (18 % grey at sRGB 0.46,
paper white at sRGB 0.98, one scale for all channels), its `grlog` LUT at
several sizes (tetrahedral lookup), and the prototype's variant D (matrix +
curves + HSL + post 3x3) fitted once against spektrafilm directly and once
against each LUT. Every fit is scored on the held-out samples against
spektrafilm directly. Also the LUT's own error on those samples.

grlog: u = (log2(L / 0.18) + 8) / 12 per channel, clipped to 0..1.

Needs the spektrafilm venv (docs/spektrafilm-fit.md). Usage:
    python -I tools/lut-error.py camera.json out.json kodak_portra_400:kodak_portra_endura ...
"""
import importlib.util
import json
import sys
import time
from pathlib import Path

import numpy as np

# the prototype reads camera.json from argv[1] at import
_spec = importlib.util.spec_from_file_location("sf", Path(__file__).with_name("spektrafilm-fit.py"))
sf = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sf)

_spec = importlib.util.spec_from_file_location("sflut", Path(__file__).with_name("spektrafilm-lut.py"))
sflut = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sflut)
grlog, Target = sflut.grlog, sflut.Target

SIZES = [33, 49, 65]


def make_lut(target, n):
    return sflut.make_lut(target, n).reshape(n, n, n, 3)  # [b, g, r]


def tetra(lut, u):
    """tetrahedral lookup, lut[b, g, r], u in 0..1 (rgb)"""
    n = lut.shape[0]
    x = np.clip(u, 0, 1) * (n - 1)
    i = np.minimum(x.astype(int), n - 2)
    f = x - i
    r, g, b = i[:, 0], i[:, 1], i[:, 2]
    fr, fg, fb = f[:, 0:1], f[:, 1:2], f[:, 2:3]
    c = lambda dr, dg, db: lut[b + db, g + dg, r + dr]
    c000, c111 = c(0, 0, 0), c(1, 1, 1)
    out = np.empty((len(u), 3))
    cases = [
        ((fr >= fg) & (fg >= fb), (1, 0, 0), (1, 1, 0), fr, fg, fb),
        ((fr >= fb) & (fb > fg), (1, 0, 0), (1, 0, 1), fr, fb, fg),
        ((fb > fr) & (fr >= fg), (0, 0, 1), (1, 0, 1), fb, fr, fg),
        ((fg > fr) & (fr >= fb), (0, 1, 0), (1, 1, 0), fg, fr, fb),
        ((fg >= fb) & (fb > fr), (0, 1, 0), (0, 1, 1), fg, fb, fr),
        ((fb > fg) & (fg > fr), (0, 0, 1), (0, 1, 1), fb, fg, fr),
    ]
    done = np.zeros(len(u), bool)
    for sel, a, bb, t1, t2, t3 in cases:
        s = sel[:, 0] & ~done
        if not s.any():
            continue
        ca, cb = c(*a)[s], c(*bb)[s]
        out[s] = (1 - t1[s]) * c000[s] + (t1[s] - t2[s]) * ca + (t2[s] - t3[s]) * cb + t3[s] * c111[s]
        done |= s
    return out


def fit_d(L, enc_t, tag, w):
    """the prototype's variant D: A first, then + HSL + post"""
    lab_t = sf.to_lab(enc_t)
    lay_a = sf.Layout(False, False)
    p_a = sf.fit(lay_a, L, lab_t, w, sf.initial(lay_a, L, lab_t, enc_t, tag))
    lay = sf.Layout(True, True)
    p0 = np.zeros(lay.n)
    p0[:6 + 3 * sf.NK] = p_a
    return lay, sf.fit(lay, L, lab_t, w, p0)


def main():
    out_path = sys.argv[2]
    films = [a.split(":") for a in sys.argv[3:]]
    L, tag, _ = sf.sample_set(1)
    Lv, tagv, cc0v = sf.sample_set(2)
    w = sf.weights(tag)
    results = {}
    for film, paper in films:
        t0 = time.time()
        target = Target(film, paper)
        enc_v = target(Lv)
        lab_v = sf.to_lab(enc_v)
        res = {"print_exposure": target.print_exposure, "scale": target.scale,
               "paper_white_linear": target.white.tolist(), "grey_out": target.grey}
        print(f"{film}: print exposure {target.print_exposure:.3f}, scale {target.scale:.3f}, "
              f"white {np.round(target.white, 3)}", flush=True)
        lay, p = fit_d(L, target(L), tag, w)
        res["direct"] = sf.metrics(lay, p, Lv, lab_v, tagv, cc0v)
        print(f"  direct       " + " ".join(f"{k} {v:5.2f}" for k, v in res["direct"].items()), flush=True)
        for n in SIZES:
            t = time.time()
            lut = make_lut(target, n)
            own = sf.de2000(sf.to_lab(tetra(lut, grlog(Lv))), lab_v)
            lay, p = fit_d(L, tetra(lut, grlog(L)), tag, w)
            m = sf.metrics(lay, p, Lv, lab_v, tagv, cc0v)
            res[f"lut{n}"] = {"fit": m, "lut_own_mean": own.mean(), "lut_own_p95": np.percentile(own, 95),
                              "lut_own_max": own.max()}
            print(f"  LUT {n:2d}       " + " ".join(f"{k} {v:5.2f}" for k, v in m.items()) +
                  f"   | LUT itself mean {own.mean():.3f} p95 {np.percentile(own, 95):.3f} max {own.max():.2f}"
                  f"  ({time.time() - t:.0f} s)", flush=True)
        results[f"{film}:{paper}"] = res
        print(f"  ({time.time() - t0:.0f} s)", flush=True)
        json.dump(results, open(out_path, "w"), indent=1, default=float)


if __name__ == "__main__":
    main()
