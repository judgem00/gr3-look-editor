"""spektrafilm films as 3D LUTs for the editor's LUT fitter (docs/plan-spektrafilm-fitter.md).

Each film is rendered once in spektrafilm's per-pixel mode (no grain, halation or other
spatial effects) and written as a .cube with `grlog` input:

    u = (log2(L / 0.18) + 8) / 12        per channel, L = scene-linear sRGB, clipped to 0..1

Output is sRGB-encoded. A negative is printed on a print medium (paper or cine print film); a
slide (positive film) is scanned directly. Normalised like a scan (default): the exposure -- the
print exposure for a negative, the film exposure for a slide -- puts 18 % grey at sRGB 0.46
(where the camera's Standard puts it), and one scale for all channels takes the white (paper or
clear film) to sRGB 0.98; black and the medium's tint stay. --raw keeps spektrafilm's output as it
is (paper white about 0.70 linear).

The header holds `# GR-INPUT: grlog -8 12` (read by the editor) and how the LUT was made.

--all renders every film on every print medium (and every slide) in parallel and writes
index.json next to the .cube files (tools/film_library: core/examples/film_library.rs fits them).

Needs spektrafilm (https://github.com/andreavolpato/spektrafilm; code GPLv3, profiles and LUTs
CC BY-SA 4.0) in a Python 3.13 venv (docs/spektrafilm-fit.md). Usage:
    python -I tools/spektrafilm-lut.py kodak_portra_400:kodak_portra_endura fujifilm_velvia_100 [...] -o DIR
    python -I tools/spektrafilm-lut.py --all -o DIR [--jobs N]
"""
import argparse
import importlib.metadata
import json
import os
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

EV_LO, EV_SPAN = -8.0, 12.0
GREY_OUT = 0.46
WHITE_OUT = 0.98


def srgb():
    import colour
    return colour.RGB_COLOURSPACES["sRGB"]


def grlog(L):
    import numpy as np
    return np.clip((np.log2(np.maximum(L, 1e-12) / 0.18) - EV_LO) / EV_SPAN, 0, 1)


def grlog_inv(u):
    return 0.18 * 2.0 ** (u * EV_SPAN + EV_LO)


def simulate(L, film, paper, print_exposure=1.0, film_ev=0.0):
    """linear sRGB in and out; paper None: the film scanned directly (a slide)"""
    import numpy as np
    from spektrafilm import init_params, simulate as run
    p = init_params(film, paper or "kodak_portra_endura")
    # lut_mode's switches by hand: lut_mode itself resets the print exposure to 1
    p.debug.lut_mode = False
    p.debug.deactivate_spatial_effects = True
    p.debug.deactivate_stochastic_effects = True
    p.camera.auto_exposure = False
    p.camera.exposure_compensation_ev = 0.0
    p.enlarger.print_exposure_compensation = False
    p.film_render.halation.boost_ev = 0.0
    p.film_render.halation.active = False
    p.scanner.white_correction = False
    p.scanner.black_correction = False
    p.scanner.unsharp_mask = (0.0, 0.0)
    p.io.input_color_space = "sRGB"
    p.io.input_cctf_decoding = False
    p.io.output_color_space = "sRGB"
    p.io.output_cctf_encoding = False
    p.io.scan_film = paper is None
    p.enlarger.print_exposure = print_exposure
    # the film's exposure (slides): the scene scaled, as a camera exposure would
    return run(np.asarray(L, np.float64)[None] * 2.0 ** film_ev, p)[0]


class Target:
    """spektrafilm, normalised like a scan (or raw): linear sRGB in, sRGB-encoded out"""

    def __init__(self, film, paper, raw=False):
        import numpy as np
        self.film, self.paper, self.raw = film, paper, raw
        self.print_exposure, self.film_ev = 1.0, 0.0
        self.white = simulate(np.full((1, 3), 0.18 * 2.0 ** 8), film, paper)[0]
        self.black = simulate(np.full((1, 3), 0.18 * 2.0 ** -10), film, paper)[0]
        self.scale = 1.0
        if not raw:
            self.scale = srgb().cctf_decoding(WHITE_OUT) / self.white.max()
            # bisection on the grey patch: more print exposure = darker, more film exposure = brighter
            lo, hi = -4.0, 4.0
            for _ in range(30):
                mid = (lo + hi) / 2
                g = self.grey(2.0 ** mid, 0.0) if paper else self.grey(1.0, mid)
                if (g > GREY_OUT) == (paper is not None):
                    lo = mid
                else:
                    hi = mid
            if paper:
                self.print_exposure = 2.0 ** ((lo + hi) / 2)
            else:
                self.film_ev = (lo + hi) / 2

    def grey(self, print_exposure=None, film_ev=None):
        import numpy as np
        e = self.print_exposure if print_exposure is None else print_exposure
        ev = self.film_ev if film_ev is None else film_ev
        out = simulate(np.full((1, 3), 0.18), self.film, self.paper, e, ev)[0] * self.scale
        return srgb().cctf_encoding(np.clip(out, 0, 1)).mean()

    def __call__(self, L):
        import numpy as np
        out = simulate(L, self.film, self.paper, self.print_exposure, self.film_ev) * self.scale
        return srgb().cctf_encoding(np.clip(out, 0, 1))


def make_lut(target, n):
    """n^3 x 3, R fastest (.cube order)"""
    import numpy as np
    u = np.linspace(0, 1, n)
    b, g, r = np.meshgrid(u, u, u, indexing="ij")
    grid = np.stack([r, g, b], -1).reshape(-1, 3)
    return target(grlog_inv(grid))


def cube_name(film, paper, raw=False):
    return f"{film}.{paper or 'scan'}{'.raw' if raw else ''}.cube"


def write_cube(path, target, n):
    rows = make_lut(target, n)
    version = importlib.metadata.version("spektrafilm")
    how = f"paper {target.paper}" if target.paper else "scanned directly (slide)"
    head = [
        f'TITLE "{target.film} / {target.paper or "scan"}"',
        f"# GR-INPUT: grlog {EV_LO:g} {EV_SPAN:g}",
        "# input: u = (log2(L / 0.18) + 8) / 12, L = scene-linear sRGB; output: sRGB-encoded",
        f"# spektrafilm {version}: film {target.film}, {how}, no spatial effects",
        "# spektrafilm by Andrea Volpato, https://github.com/andreavolpato/spektrafilm, licensed CC BY-SA 4.0",
        f"# normalisation: {'raw' if target.raw else 'scan'}, print exposure {target.print_exposure:.4f}, "
        f"film exposure {target.film_ev:+.3f} EV, scale {target.scale:.4f}, grey {target.grey():.4f}",
        f"# white (linear, before scale) {' '.join(f'{v:.4f}' for v in target.white)}, "
        f"black {' '.join(f'{v:.4f}' for v in target.black)}",
        f"LUT_3D_SIZE {n}",
        "DOMAIN_MIN 0 0 0",
        "DOMAIN_MAX 1 1 1",
    ]
    body = "\n".join(f"{r:.6f} {g:.6f} {b:.6f}" for r, g, b in rows)
    Path(path).write_text("\n".join(head) + "\n" + body + "\n")


def profiles():
    """spektrafilm's stocks: {id: info}"""
    import spektrafilm
    root = Path(spektrafilm.__file__).parent / "data" / "profiles"
    return {p.stem: json.load(open(p))["info"] for p in sorted(root.glob("*.json"))}


def combos():
    """every negative on every print medium, every slide scanned; the film's own print first"""
    info = profiles()
    prints = [k for k, v in info.items() if v["stage"] == "printing"]
    out = []
    for k, v in info.items():
        if v["stage"] != "filming" or v.get("channel_model") != "color":
            continue
        if v["type"] == "positive":
            out.append((k, None, True))
            continue
        default = v.get("target_print") or "kodak_portra_endura"
        for p in sorted(prints, key=lambda p: p != default):
            out.append((k, p, p == default))
    return info, out


def job(film, paper, default, size, raw, out):
    t = Target(film, paper, raw)
    path = Path(out) / cube_name(film, paper, raw)
    write_cube(path, t, size)
    return {"film": film, "print": paper, "default": default, "cube": path.name,
            "print_exposure": round(t.print_exposure, 4), "film_ev": round(t.film_ev, 3),
            "scale": round(float(t.scale), 4), "grey": round(float(t.grey()), 4),
            "white": [round(float(v), 4) for v in t.white], "black": [round(float(v), 4) for v in t.black]}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("films", nargs="*", help="FILM:PAPER, or FILM alone for a slide (spektrafilm stock names)")
    ap.add_argument("-o", "--out", default=".", help="directory for the .cube files")
    ap.add_argument("--size", type=int, default=33)
    ap.add_argument("--raw", action="store_true", help="spektrafilm's output without normalisation")
    ap.add_argument("--all", action="store_true", help="every film on every print medium, every slide")
    ap.add_argument("--jobs", type=int, default=os.cpu_count(), help="parallel processes (--all)")
    a = ap.parse_args()
    Path(a.out).mkdir(parents=True, exist_ok=True)
    if not a.all:
        for spec in a.films:
            film, _, paper = spec.partition(":")
            r = job(film, paper or None, True, a.size, a.raw, a.out)
            print(f"{r['cube']}: print exposure {r['print_exposure']}, film {r['film_ev']:+} EV, "
                  f"scale {r['scale']}, grey {r['grey']}")
        return
    info, todo = combos()
    print(f"{len(todo)} looks on {a.jobs} processes")
    done = []
    with ProcessPoolExecutor(a.jobs) as pool:
        futures = [pool.submit(job, f, p, d, a.size, a.raw, a.out) for f, p, d in todo]
        for k, fut in enumerate(as_completed(futures), 1):
            r = fut.result()
            done.append(r)
            print(f"  {k:3d}/{len(todo)} {r['cube']}", flush=True)
    order = {(f, p): k for k, (f, p, _) in enumerate(todo)}
    done.sort(key=lambda r: order[(r["film"], r["print"])])
    names = {k: v["name"] for k, v in info.items()}
    index = {"spektrafilm": importlib.metadata.version("spektrafilm"), "size": a.size,
             "normalisation": "raw" if a.raw else "scan", "names": names, "looks": done}
    json.dump(index, open(Path(a.out) / "index.json", "w"), indent=1)
    print(f"index.json: {len(done)} looks")


if __name__ == "__main__":
    # one thread per process: the pool runs one process per core
    for v in ("NUMBA_NUM_THREADS", "OMP_NUM_THREADS", "OPENBLAS_NUM_THREADS", "VECLIB_MAXIMUM_THREADS"):
        os.environ.setdefault(v, "1")
    main()
