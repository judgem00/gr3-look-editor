"""Prototype: fit spektrafilm film targets with what the GR III look path can express,
with and without a free post-gamma 3x3, and compare dE2000.

Camera model (per sample, L = scene-linear sRGB after the sensor CCM, mid grey 0.18):
    x   = M L                                  look matrix, rows sum to 1
    x   = S[sector(x)] x                       HSL (MultiAxial): 12 hue sectors, hue/sat/lum each,
                                               matrix as multiaxial::band_matrix, hard edges, weight 1
    b   = base(SCALE x)                        Standard tone curve (GammaTuning, default sliders),
                                               256 entries over the 12-bit input in steps of 16
    o_c = C_c(b_c / 64) / 16384                channel curve per channel (256 entries)
    o   = P o                                  post-gamma 3x3 (CSP), rows sum to 1   [optional]
o is taken as sRGB-encoded output (JPEG).

Target: spektrafilm in lut_mode (no grain/halation/spatial effects), linear sRGB in/out,
sRGB-encoded for comparison.

Results and assumptions: docs/spektrafilm-fit.md. Needs spektrafilm (GPLv3,
https://github.com/andreavolpato/spektrafilm) and its runtime dependencies in a Python 3.13 venv.

Usage:
    cargo run --release --example camera_model --manifest-path core/Cargo.toml > work/spektrafilm-fit/camera.json
    python -I tools/spektrafilm-fit.py work/spektrafilm-fit/camera.json out.json kodak_portra_400:kodak_portra_endura ...
"""
import json
import sys
import time

import colour
import numpy as np
from scipy.optimize import least_squares

cam = json.load(open(sys.argv[1]))
BASE = np.array(cam["curve"], float)          # 256 x u16, 0..16383
YCC = np.array(cam["ycc"], float).reshape(3, 3)
YCC_INV = np.linalg.inv(YCC)
IDX = np.arange(256.0)
# 18% grey -> sRGB 0.46 on the Standard curve (as the camera's AE puts it)
MID_IDX = np.interp(0.46 * 16384, BASE, IDX)
SCALE = MID_IDX * 16 / 0.18                   # linear -> 12-bit input
KNOTS = np.arange(0, 257, 8.0)                # 33 knots over C's domain (base >> 6)
NK = len(KNOTS)
SECTORS = 12
CIRCLE = 8192.0
QUADRANT = 2048.0
SRGB = colour.RGB_COLOURSPACES["sRGB"]


# ---------------------------------------------------------------------------- samples
def colorchecker():
    cc = colour.CCS_COLOURCHECKERS["ColorChecker24 - After November 2014"]
    xyz = colour.xyY_to_XYZ(np.array(list(cc.data.values())))
    xyz = colour.chromatic_adaptation(xyz, colour.xy_to_XYZ(cc.illuminant), colour.xy_to_XYZ(SRGB.whitepoint))
    return np.clip(colour.XYZ_to_RGB(xyz, SRGB, apply_cctf_encoding=False), 0, None)


NRND, NSWEEP = 1200, 800
MAX_NFEV = 60


def sample_set(seed):
    rng = np.random.default_rng(seed)
    off = rng.uniform(-0.25, 0.25)
    grey = np.repeat((0.18 * 2.0 ** (np.linspace(-6, 3.3, 120) + off))[:, None], 3, 1)
    cc = colorchecker()
    evs = np.array([-2, -1, 0, 1, 2]) + off
    chart = np.concatenate([cc * 2.0 ** e for e in evs])
    # random scene colours: moderate chroma, many exposures
    rnd = rng.random((NRND, 3)) ** 2.2
    rnd = rnd / np.maximum(rnd.max(1, keepdims=True), 1e-3) * 0.18 * 2.0 ** rng.uniform(-4, 2.2, (NRND, 1)) * 1.3
    # hue sweep: every hue at several saturations and exposures (where HSL acts)
    n = NSWEEP
    hsv = np.stack([rng.uniform(0, 1, n), rng.uniform(0.2, 0.9, n), np.ones(n)], 1)
    sweep = SRGB.cctf_decoding(colour.HSV_to_RGB(hsv))
    sweep = sweep / sweep.max(1, keepdims=True) * 0.18 * 2.0 ** rng.uniform(-2, 1.5, (n, 1)) * 1.5
    L = np.concatenate([grey, chart, rnd, sweep])
    tag = np.concatenate([np.full(len(grey), "grey"), np.full(len(chart), "chart"),
                          np.full(len(rnd), "random"), np.full(len(sweep), "sweep")])
    # skin = first two chart patches, chart at the middle exposure
    is_cc0 = np.zeros(len(L), bool)
    start = len(grey) + 2 * 24
    is_cc0[start:start + 24] = True
    return L, tag, is_cc0


# ---------------------------------------------------------------------------- target
def spektrafilm(L, film, paper):
    from spektrafilm import init_params, simulate
    p = init_params(film, paper)
    p.debug.lut_mode = True
    p.io.input_color_space = "sRGB"
    p.io.input_cctf_decoding = False
    p.io.output_color_space = "sRGB"
    p.io.output_cctf_encoding = False
    out = simulate(L[None].astype(np.float64), p)[0]
    return SRGB.cctf_encoding(np.clip(out, 0, 1))


def to_lab(enc):
    lin = SRGB.cctf_decoding(np.clip(enc, 0, 1))
    xyz = colour.RGB_to_XYZ(lin, SRGB, apply_cctf_decoding=False)
    return colour.XYZ_to_Lab(xyz, SRGB.whitepoint)


# ---------------------------------------------------------------------------- model
def coordinate(cb, cr):
    """multiaxial::coordinate, vectorised"""
    q = np.where(cr >= 0, np.where(cb >= 0, 0, 3), np.where(cb >= 0, 1, 2))
    a = np.select([q == 0, q == 3, q == 1], [cr, -cb, cb], -cr)
    b = np.select([q == 0, q == 3, q == 1], [cb, cr, -cr], -cb)
    s = a + b
    x = np.where(s > 0, QUADRANT * b / np.where(s > 0, s, 1), 0)
    return q * QUADRANT + x


def direction(g):
    quarter = min(int(g // QUADRANT), 3)
    r = (g - quarter * QUADRANT) / QUADRANT
    a, b = 1 - r, r
    cb, cr = {0: (b, a), 3: (a, -b), 2: (-b, -a), 1: (-a, b)}[[0, 3, 2, 1][quarter]]
    n = np.hypot(cb, cr)
    return cb / n, cr / n


SECTOR_DIR = [direction((k + 0.5) * CIRCLE / SECTORS) for k in range(SECTORS)]


def band_matrix(k, hue, sat, lum):
    dcb, dcr = SECTOR_DIR[k]
    th = -np.radians(hue)
    s = 1 + sat
    c, sn = np.cos(th), np.sin(th)
    inner = np.array([[1, lum * dcb, lum * dcr], [0, s * c, s * sn], [0, -s * sn, s * c]])
    return YCC_INV @ inner @ YCC


def rows_sum1(v):
    """6 values -> 3x3 with rows summing to 1 (off-diagonals free)"""
    m = np.zeros((3, 3))
    m[0, 1], m[0, 2], m[1, 0], m[1, 2], m[2, 0], m[2, 1] = v
    m[np.diag_indices(3)] = 1 - m.sum(1)
    return m


class Layout:
    def __init__(self, hsl, post):
        self.hsl, self.post = hsl, post
        self.n = 6 + 3 * NK + (3 * SECTORS if hsl else 0) + (6 if post else 0)

    def split(self, p):
        i = 0
        M = rows_sum1(p[i:i + 6]); i += 6
        U = p[i:i + 3 * NK].reshape(3, NK); i += 3 * NK
        H = None
        if self.hsl:
            H = p[i:i + 3 * SECTORS].reshape(SECTORS, 3); i += 3 * SECTORS
        P = rows_sum1(p[i:i + 6]) if self.post else np.eye(3)
        return M, U, H, P

    def bounds(self):
        lo = np.full(self.n, -np.inf); hi = np.full(self.n, np.inf)
        lo[:6], hi[:6] = -1.5, 1.5
        lo[6:6 + 3 * NK], hi[6:6 + 3 * NK] = -6.0, 9.0          # exp(9) ~ 8100 per knot step
        if self.hsl:
            i = 6 + 3 * NK
            lim = np.tile([30.0, 0.9, 0.9], SECTORS)        # hue deg, sat, lum
            lo[i:i + 3 * SECTORS], hi[i:i + 3 * SECTORS] = -lim, lim
        return lo, hi


def channel_curves(U):
    """knot values: cumulative positive steps, black may sit above 0"""
    v = np.cumsum(np.exp(U), axis=1)
    return np.clip(v, 0, 16383)


def render(p, lay, L, sector):
    M, U, H, P = lay.split(p)
    x = L @ M.T
    if H is not None:
        out = x.copy()
        for k in range(SECTORS):
            sel = sector == k
            if sel.any():
                out[sel] = x[sel] @ band_matrix(k, *H[k]).T
        x = np.maximum(out, 0)
    b = np.interp(np.clip(x * SCALE, 0, 4095) / 16, IDX, BASE)
    C = channel_curves(U)
    o = np.stack([np.interp(b[:, c] / 64, KNOTS, C[c]) for c in range(3)], 1) / 16384
    return np.clip(o @ P.T, 0, 1)


def sectors_of(L, M):
    # the hue sector is decided after M; M changes slowly, so it is refreshed per round
    ycc = (L @ M.T) @ YCC.T
    return np.minimum((coordinate(ycc[:, 1], ycc[:, 2]) // (CIRCLE / SECTORS)).astype(int), SECTORS - 1)


def de2000(a, b):
    return colour.delta_E(a, b, method="CIE 2000")


def fit(lay, L, lab_t, w, p0, rounds=2):
    p = p0.copy()
    lo, hi = lay.bounds()
    p = np.clip(p, lo + 1e-9, hi - 1e-9)
    sw = np.sqrt(w)
    for r in range(rounds):
        t = time.time()
        sector = sectors_of(L, lay.split(p)[0])
        f = lambda q: sw * np.sqrt(de2000(to_lab(render(q, lay, L, sector)), lab_t) ** 2 + 1e-4)
        p = least_squares(f, p, bounds=(lo, hi), max_nfev=MAX_NFEV).x
        print(f"    fit n={lay.n} round {r}: {time.time() - t:.0f} s", flush=True)
    return p


def initial(lay, L, lab_t, enc_t, tag):
    """identity matrix; curves so that grey matches the target"""
    p = np.zeros(lay.n)
    grey = tag == "grey"
    x = L[grey][:, 1]
    b = np.interp(np.clip(x * SCALE, 0, 4095) / 16, IDX, BASE) / 64
    U = np.zeros((3, NK))
    for c in range(3):
        t = enc_t[grey][:, c] * 16384
        order = np.argsort(b)
        want = np.interp(KNOTS, b[order], np.maximum.accumulate(t[order]))
        steps = np.diff(np.concatenate([[0], want]))
        U[c] = np.log(np.maximum(steps, 1.0))
    p[6:6 + 3 * NK] = U.ravel()
    return p


def metrics(lay, p, L, lab_t, tag, is_cc0):
    M = lay.split(p)[0]
    lab = to_lab(render(p, lay, L, sectors_of(L, M)))
    d = de2000(lab, lab_t)
    lch_t = colour.Lab_to_LCHab(lab_t)
    warm = (lch_t[:, 1] > 20) & (lch_t[:, 2] > 30) & (lch_t[:, 2] < 100)
    sat = lch_t[:, 1] > 40
    skin = np.zeros(len(L), bool)
    skin[np.flatnonzero(is_cc0)[:2]] = True
    return {
        "all": d.mean(), "p95": np.percentile(d, 95), "chart": d[is_cc0].mean(),
        "skin": d[skin].mean(), "warm": d[warm].mean(), "saturated": d[sat].mean(),
        "grey": d[tag == "grey"].mean(),
    }


def weights(tag):
    w = np.ones(len(tag))
    w[tag == "grey"] = 25.0
    w[tag == "chart"] = 6.0
    return w


VARIANTS = [("A  matrix+curves", False, False), ("B  +HSL", True, False),
            ("C  +post 3x3", False, True), ("D  +HSL +post", True, True)]


def main():
    out_path = sys.argv[2]
    films = [a.split(":") for a in sys.argv[3:]]
    L, tag, cc0 = sample_set(1)
    Lv, tagv, cc0v = sample_set(2)          # held out: other exposures, other colours
    w = weights(tag)
    results = {}
    for film, paper in films:
        t0 = time.time()
        both = spektrafilm(np.concatenate([L, Lv]), film, paper)
        enc_t, enc_v = both[:len(L)], both[len(L):]
        lab_t, lab_v = to_lab(enc_t), to_lab(enc_v)
        res = {}
        lay_a = Layout(False, False)
        p_a = fit(lay_a, L, lab_t, w, initial(lay_a, L, lab_t, enc_t, tag))
        for name, hsl, post in VARIANTS:
            lay = Layout(hsl, post)
            if not hsl and not post:
                p = p_a
            else:
                p0 = np.zeros(lay.n)
                p0[:6 + 3 * NK] = p_a
                p = fit(lay, L, lab_t, w, p0)
            m = metrics(lay, p, Lv, lab_v, tagv, cc0v)
            M, U, H, P = lay.split(p)
            res[name] = {"held_out": m, "train_all": metrics(lay, p, L, lab_t, tag, cc0)["all"],
                         "post": P.round(4).tolist(), "matrix": M.round(4).tolist(),
                         "hsl": None if H is None else H.round(3).tolist()}
            print(f"{film:22s} {name:18s} " + " ".join(f"{k} {v:5.2f}" for k, v in m.items()), flush=True)
        results[f"{film}:{paper}"] = res
        print(f"  ({time.time() - t0:.0f} s)", flush=True)
        json.dump(results, open(out_path, "w"), indent=1)


if __name__ == "__main__":
    main()
