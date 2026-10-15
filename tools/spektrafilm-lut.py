"""spektrafilm films as 3D LUTs for the editor's LUT fitter (docs/plan-spektrafilm-fitter.md).

Each film (negative + print paper) is rendered once in spektrafilm's per-pixel mode (no grain,
halation or other spatial effects) and written as a .cube with `grlog` input:

    u = (log2(L / 0.18) + 8) / 12        per channel, L = scene-linear sRGB, clipped to 0..1

Output is sRGB-encoded. Normalised like a print scan (default): the print exposure puts 18 %
grey at sRGB 0.46 (where the camera's Standard puts it), and one scale for all channels takes
paper white to sRGB 0.98; paper black and the paper's tint stay. --raw keeps spektrafilm's
output as it is (paper white about 0.70 linear).

The header holds `# GR-INPUT: grlog -8 12` (read by the editor) and how the LUT was made.

Needs spektrafilm (GPLv3, https://github.com/andreavolpato/spektrafilm) in a Python 3.13 venv
(docs/spektrafilm-fit.md). Usage:
    python -I tools/spektrafilm-lut.py kodak_portra_400:kodak_portra_endura [...] -o DIR [--size 33] [--raw]
"""
import argparse
import importlib.metadata
from pathlib import Path

import colour
import numpy as np

EV_LO, EV_SPAN = -8.0, 12.0
GREY_OUT = 0.46
WHITE_OUT = 0.98
SRGB = colour.RGB_COLOURSPACES["sRGB"]


def grlog(L):
    return np.clip((np.log2(np.maximum(L, 1e-12) / 0.18) - EV_LO) / EV_SPAN, 0, 1)


def grlog_inv(u):
    return 0.18 * 2.0 ** (u * EV_SPAN + EV_LO)


def simulate(L, film, paper, print_exposure):
    """linear sRGB in and out"""
    from spektrafilm import init_params, simulate as run
    p = init_params(film, paper)
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
    p.enlarger.print_exposure = print_exposure
    return run(np.asarray(L, np.float64)[None], p)[0]


class Target:
    """spektrafilm, normalised like a print scan (or raw): linear sRGB in, sRGB-encoded out"""

    def __init__(self, film, paper, raw=False):
        self.film, self.paper, self.raw = film, paper, raw
        self.white = simulate(np.full((1, 3), 0.18 * 2.0 ** 8), film, paper, 1.0)[0]
        self.black = simulate(np.full((1, 3), 0.18 * 2.0 ** -10), film, paper, 1.0)[0]
        self.scale, self.print_exposure = 1.0, 1.0
        if not raw:
            self.scale = SRGB.cctf_decoding(WHITE_OUT) / self.white.max()
            # print exposure: bisection in log space on the grey patch (more exposure = darker)
            lo, hi = -4.0, 4.0
            for _ in range(30):
                mid = (lo + hi) / 2
                if self.grey(2.0 ** mid) > GREY_OUT:
                    lo = mid
                else:
                    hi = mid
            self.print_exposure = 2.0 ** ((lo + hi) / 2)

    def grey(self, print_exposure=None):
        e = self.print_exposure if print_exposure is None else print_exposure
        out = simulate(np.full((1, 3), 0.18), self.film, self.paper, e)[0] * self.scale
        return SRGB.cctf_encoding(np.clip(out, 0, 1)).mean()

    def __call__(self, L):
        out = simulate(L, self.film, self.paper, self.print_exposure) * self.scale
        return SRGB.cctf_encoding(np.clip(out, 0, 1))


def make_lut(target, n):
    """n^3 x 3, R fastest (.cube order)"""
    u = np.linspace(0, 1, n)
    b, g, r = np.meshgrid(u, u, u, indexing="ij")
    grid = np.stack([r, g, b], -1).reshape(-1, 3)
    return target(grlog_inv(grid))


def write_cube(path, target, n):
    rows = make_lut(target, n)
    version = importlib.metadata.version("spektrafilm")
    head = [
        f'TITLE "{target.film} / {target.paper}"',
        f"# GR-INPUT: grlog {EV_LO:g} {EV_SPAN:g}",
        "# input: u = (log2(L / 0.18) + 8) / 12, L = scene-linear sRGB; output: sRGB-encoded",
        f"# spektrafilm {version}: film {target.film}, paper {target.paper}, no spatial effects",
        f"# normalisation: {'raw' if target.raw else 'print scan'}, print exposure {target.print_exposure:.4f}, "
        f"scale {target.scale:.4f}, grey {target.grey():.4f}",
        f"# paper white (linear, before scale) {' '.join(f'{v:.4f}' for v in target.white)}, "
        f"paper black {' '.join(f'{v:.4f}' for v in target.black)}",
        f"LUT_3D_SIZE {n}",
        "DOMAIN_MIN 0 0 0",
        "DOMAIN_MAX 1 1 1",
    ]
    body = "\n".join(f"{r:.6f} {g:.6f} {b:.6f}" for r, g, b in rows)
    Path(path).write_text("\n".join(head) + "\n" + body + "\n")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("films", nargs="+", help="FILM:PAPER, spektrafilm stock names")
    ap.add_argument("-o", "--out", default=".", help="directory for the .cube files")
    ap.add_argument("--size", type=int, default=33)
    ap.add_argument("--raw", action="store_true", help="spektrafilm's output without normalisation")
    a = ap.parse_args()
    Path(a.out).mkdir(parents=True, exist_ok=True)
    for spec in a.films:
        film, paper = spec.split(":")
        t = Target(film, paper, a.raw)
        path = Path(a.out) / f"{film}.{paper}{'.raw' if a.raw else ''}.cube"
        write_cube(path, t, a.size)
        print(f"{path}: print exposure {t.print_exposure:.3f}, scale {t.scale:.3f}, grey {t.grey():.3f}")


if __name__ == "__main__":
    main()
