# Plan: LUT fitter (phase 5 of the post matrix)

Goal: the editor fits a GR III look to a 3D LUT: look matrix, RGB curves,
HSL and the post matrix P, for one image control, editable afterwards and
exported as usual (v2 preset, `"targets"`). Two kinds of LUT feed it:

- **spektrafilm films** (negative + print paper): a Python tool renders each
  film once into a LUT;
- **the user's own LUTs** (`.cube`), uploaded in the editor.

Status 2026-10-15: plan agreed (fit in the core against a LUT, spektrafilm
only makes LUTs, normalised like a print scan, test flash Portra 400 full /
without P / without HSL). Phases 0–2 done: the LUT costs nothing
measurable; tool, core fitter and CLI built and tested (no editor UI yet).

Background: [spektrafilm-fit.md](spektrafilm-fit.md) (offline prototype,
mean ΔE2000 1.68 with everything), [plan-post-matrix.md](plan-post-matrix.md)
(P on the camera, confirmed on the GR III).

## Why the fit runs in the core, against a LUT

- One fitter for both sources. A spektrafilm film is just a LUT that a tool
  made; a user LUT goes the same way.
- The fit uses the core's own camera model: the same tone curve, the same
  HSL encoding (steps, banks, Q10), the same quantisation of matrix (Q13)
  and P (9-bit registers) as export and preview. Fit and camera cannot
  drift apart, and the report is what the export will give.
- spektrafilm (GPLv3, Python, numba) stays outside the editor. Only its
  output, numbers in a LUT, reaches it.
- The cost: the LUT's interpolation error. Phase 0 measures it before any
  core code.

## The LUT

### Input space

A LUT says nothing about what it expects. The editor asks on upload; the
spektrafilm tool writes it into the file (`# GR-INPUT: ...` comment, read by
the editor, ignored by other programs):

| Input | Meaning | Target for scene colour L |
|---|---|---|
| `display` (default for uploads) | sRGB-encoded image, a "creative LUT" | LUT(Standard(L)): the LUT on the camera's Standard rendering, at default sliders |
| `linear` | linear sRGB | LUT(L) |
| `grlog` (spektrafilm tool) | log2 shaper: 0..1 = 18 % grey · 2^(−8 … +4) | LUT(log(L)) |

Output is always sRGB-encoded (display), as the JPEG. Log camera spaces
(S-Log, V-Log, ...) can be added later as further input encodings.

### Format

`.cube` (Resolve / Adobe): `LUT_3D_SIZE`, optional `DOMAIN_MIN/MAX`, values
R fastest. Lookup tetrahedral. A 1D-only `.cube` is refused. Sizes up to 65
(65³ · 3 floats = 3.3 MB in memory, fine in WASM).

### spektrafilm → LUT

`tools/spektrafilm-lut.py FILM:PAPER [--size 65] [--raw] -o film.cube`, in
the existing Python 3.13 venv: `lut_mode`, linear sRGB in and out, `grlog`
input. Normalised **like a print scan** (default):

- print exposure such that 18 % grey lands where Standard puts it (sRGB
  0.46), so a film look keeps the camera's exposure;
- output scaled so that paper white maps to sRGB 0.98; paper black and the
  paper's tint stay (same scale on all channels, no colour correction).

`--raw` writes spektrafilm's output as it is (paper white ≈ 0.70 linear).
The header records film, paper, spektrafilm version, print exposure and
scale. Whether the editor ships some film LUTs (e.g. the 7 of the
prototype) is decided after phase 4; they are spektrafilm's output, not
its code.

## Model

```
L               scene-linear sRGB after the sensor CCM (camera RGB · WB · sensor_ccm)
x = M L         look matrix = the target's matrix (Q13, i16)
x = HSL(x)      MultiAxial: lane and banks as the core encodes the HSL curves
b = base(x)     Standard's tone curve at default sliders (the target's source)
o_c = C_c(b_c)  channel curve per channel, 256 × u16, interpolated on b >> 6
o = P o         post matrix, rows sum to 1, as the 9-bit registers give it
```

- The target's **source is Standard**: its tone curve is the one the model
  uses. Bleach Bypass (P on three tables) and Cross Process (other curve
  sets) are not offered as LUT targets.
- Assumption to check once: sensor_ccm gives linear sRGB (D65), so L is the
  input of M. Check: a ColorChecker DNG with an identity look matrix through
  the core against the chart's reference.

### Fit

As the prototype (`tools/spektrafilm-fit.py`), ported to Rust:

- samples: grey ramp (25× weight), ColorChecker at 5 exposures (6×),
  random colours, hue sweep; a second set with other colours and a shifted
  exposure for the report;
- parameters: M (6, rows sum to 1), 3 × 33 monotone curve knots, 12 HSL
  sectors × (hue, sat, lum), P (6, rows sum to 1): up to 147;
- least squares on per-sample ΔE2000, Levenberg–Marquardt with bounds
  (own small solver; the core has no linear-algebra dependency and does
  not need one: 147 × 147 normal equations);
- passes: A (matrix + curves) → full (+ HSL + P on 12 sectors) → HSL curve
  points at the sector centres → **core encoding** → refit M, curves and P
  with the encoded lane fixed → quantise M and P → report on the held-out
  set through the exact export path.

Options: without P, without HSL (for the test flash and for comparison).
Report: ΔE2000 all, p95, ColorChecker, skin, warm, saturated, grey; for the
sector model and the encoded result.

Runtime: the Python prototype needs ~7 s per round and film. The Rust fit
runs in the web worker with a progress message; target under 30 s.

### What the fit writes into the target

| Part | Written as | Limit |
|---|---|---|
| Look matrix M | `edit.matrix.base`, sliders neutral | i16 Q13: −4…+4 |
| Channel curves | `edit.channel_edits[i] = {base: 256 × u16, points: identity}` | 0…16383, monotone |
| HSL | `hsl.curves` points at the 12 sector centres, default protection | hue ±45°, sat/lum ±100; core encoding |
| P | `post` | 9-bit registers over all 9 hue steps |
| Where it came from | `edit.lut = {name, sha256, input, options, report}` | the LUT itself is not stored |

Everything stays editable afterwards; the fit only sets the start.

## Phases

0. **Measure the LUT error, in Python, before core code.** For Portra 400,
   Ektar 100 and Vision3 250D: the prototype's variant D fitted against
   spektrafilm directly and against its `grlog` LUT at 33, 49, 65; both
   evaluated against spektrafilm directly. Also the print-scan
   normalisation in the prototype. Result decides the LUT size (goal: LUT
   costs < 0.1 ΔE). Python first, because the comparison needs spektrafilm
   at both ends.

   **Done 2026-10-15** (`tools/lut-error.py`, spektrafilm 0.3.4, print-scan
   normalised). Variant D, held-out samples, ΔE2000 against spektrafilm
   directly; "LUT itself" = the LUT against spektrafilm on the same samples:

   | Film (print) | print exp. | direct | LUT 33 | LUT 49 | LUT 65 | LUT 33 itself mean / max |
   |---|---|---|---|---|---|---|
   | Portra 400 (Portra Endura) | 1.114 | 1.66 | 1.68 | 1.65 | 1.66 | 0.11 / 0.50 |
   | Ektar 100 (Portra Endura) | 1.111 | 1.89 | 1.89 | 1.88 | 1.86 | 0.12 / 0.62 |
   | Vision3 250D (2383) | 1.079 | 1.84 | 1.84 | 1.83 | 1.84 | 0.10 / 0.49 |

   - The LUT costs nothing measurable: every size is within ±0.03 of the
     direct fit, which is the fit's own noise. 33³ is enough; the tool
     writes 33, the fitter takes any size up to 65.
   - The print-scan normalisation does not cost accuracy (Portra 1.66 vs
     1.67 raw in the prototype). It needs spektrafilm's `lut_mode` switches
     set by hand, because `lut_mode` resets the print exposure to 1.
   - Paper white is 0.71 linear (Vision3/2383: 0.76/0.73/0.65, warm), so the
     scale is 1.35 (1.26).
1. **`tools/spektrafilm-lut.py`**: films → `.cube`, header, normalisation.

   **Done 2026-10-15.** `python -I tools/spektrafilm-lut.py FILM:PAPER ...
   -o DIR [--size 33] [--raw]` writes `FILM.PAPER.cube` (33³, ~1 MB) with
   `# GR-INPUT: grlog -8 12` and the print exposure, scale and paper
   white/black in the header. `tools/lut-error.py` uses its target.
2. **Core `lutfit.rs`**: `.cube` parser, tetrahedral lookup, input spaces,
   samples, the forward model on the core's encodings, solver, passes,
   report. Tests: parser (sizes, domain, comments, refusals), a LUT made
   from a known look (matrix + curves + P from the core) is fitted back to
   within 0.3 ΔE, identity LUT with `display` input gives Standard, the
   result exports and passes the space check.
   CLI: `core/examples/lut_fit.rs film.cube=NegativeFilm
   [--no-post] [--no-hsl] > preset.json` (for the tools and the test
   firmware).

   **Done 2026-10-15.** `core/src/lutfit.rs`, `core/examples/lut_fit.rs`
   (`film.cube[=Target] ... [--no-post] [--no-hsl] [--input ...]`, one preset,
   report on stderr and in `edit.lut`), `core/tests/lutfit.rs` (9 tests).
   - Model as above, on the core's code: Standard's tone curve (gamma.rs),
     `chan::compose` for the channel tables, `multiaxial::encode` /
     `Encoded::apply` for HSL, `postmatrix::encode` + `registers` for P
     (hue 0), Q13 for M. `Fit::renderer` renders a fitted look exactly so.
   - HSL goes to the editor as plateau curves (two points per sector, 8
     units inside its edges), so the encoder finds the sector edges as
     steps. 12 sectors become 11 banks (one merge); the refit with the
     encoded lane makes up for it: model and encoded result differ by
     ≤ 0.01 ΔE.
   - Curve knots: 16383 · (cumulative exp steps) / (sum incl. a headroom
     term). A hard cap at 16383 (as in the prototype) stalls the fit where
     a knot hits white: grey then clipped to 1.0 from +2.5 EV while the
     film rolls off at 0.92–0.96 (Ektar: ΔE 4.6 there).
   - Tests: parser and refusals, tetrahedral lookup exact on linear LUTs,
     ΔE2000 against Sharma et al., unsupported targets refused; identity
     LUT on display input → Standard (final 0.01 ΔE); a LUT made from a
     known look (Standard matrix, per-channel gamma, Portra P) fitted back
     to 0.12 ΔE; fitted targets export alone and as three, space report
     `fits` (812 bytes free), a fourth film is refused ("not enough free
     space"). `cargo test` all green, clippy clean, wasm32 builds.
   - Speed: 4.7–6.8 s per film natively (release, 2 240 samples).

   Results, held-out samples against the LUT (ΔE2000; prototype = the
   Python fit against spektrafilm directly, phase 0):

   | Film (target) | matrix + curves | full: all | p95 | chart | skin | grey | prototype |
   |---|---|---|---|---|---|---|---|
   | Portra 400 | 2.39 | **1.61** | 3.58 | 1.20 | 0.95 | 0.41 | 1.66 |
   | Ektar 100 | 2.71 | **1.79** | 4.03 | 1.47 | 1.13 | 0.47 | 1.89 |
   | Vision3 250D | 2.60 | **1.74** | 4.00 | 1.50 | 1.40 | 0.40 | 1.84 |

   Portra variants (the test flash): full 1.61, without HSL 1.79, without
   P 2.02, matrix + curves only 2.39.
3. **Session / WASM**: `fit_lut(lut bytes, {target, input, post, hsl})` →
   the target spec (as a v2 preset's target) + report; progress through the
   worker.
4. **Editor**: in a target's Advanced section, "Fit to LUT…": file, input
   space (pre-set from `GR-INPUT`), with P / with HSL, run, report, apply.
   Apply sets source Standard and the parts above, so preview, space meter
   and export work unchanged. The preview can show the LUT itself as the
   "before" side (LUT on the shot's Standard render or linear data), so the
   user sees how close the fit is on their own photo. `edit.lut` survives
   load and export. Docs: `editor.md` (English), i18n for the dialog.
5. **Checks** (done when all pass): `cargo test`; a spektrafilm LUT fitted
   in the browser loads, previews plausibly (grey ramp, skin, saturated
   colours, checked in the browser) and exports; `tools/verify-tuning.py
   --post IMAGE PRESET` and `tools/verify-targets.py --patched`; three
   complete films fit into the pool, a fourth is refused with the core's
   space error.
6. **Test firmware** `core/examples/film_flash.rs` → `work/film-test/`
   (FWDC239B.BIN, unpackedFW, preset, HOW_TO_SHOOT.txt), checked with
   `fw-pack.py check`, `verify-tuning.py --post`, `verify-targets.py
   --patched`. Flashing and shooting is the user's.

## Test flash (phase 6)

One flash, Portra 400 on Portra Endura (print-scan normalised):

| Image control | Content | Shows |
|---|---|---|
| Standard | unchanged | control |
| Negative Film | M + curves + HSL + P | the full film look |
| Positive Film | M + curves + HSL, fitted without P | what P buys on the camera |
| Vivid | M + curves + P, fitted without HSL | what HSL buys on top of P |

Shots: handheld is fine (alignment with ORB + RANSAC), same light, Daylight
WB, fixed exposure, RAW + JPEG, every slider at 0, grain off, shadow
correction as in the last test; a grey ramp, the ColorChecker if at hand,
skin, a saturated scene. Order: Standard, Negative Film, Positive Film,
Vivid, Standard.

Evaluation (as `tools/eval-post-matrix.py`): each camera JPEG against the
LUT applied to the Standard DNG's linear data, and against the core's model
of that look; ΔE2000 per variant. Expected: Negative Film < Vivid <
Positive Film against the LUT, and model vs camera near the floor
(≈ 0.7–1.0 ΔE).

## Open points

- Display LUTs: "LUT on Standard" assumes the LUT was made for an image
  like the camera's Standard JPEG. LUTs made for log footage need their
  input encoding (later).
- The camera's highlight chroma roll-off is not modelled (as before).
- Shipping film LUTs with the editor (size, which films): after phase 4.
