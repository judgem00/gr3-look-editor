# Plan: LUT fitter (phase 5 of the post matrix)

Goal: the editor fits a GR III look to a 3D LUT: look matrix, RGB curves,
HSL and the post matrix P, for one image control, editable afterwards and
exported as usual (v2 preset, `"targets"`). Two kinds of LUT feed it:

- **spektrafilm films** (negative + print paper): a Python tool renders each
  film once into a LUT;
- **the user's own LUTs** (`.cube`), uploaded in the editor.

Status 2026-10-15: plan agreed (fit in the core against a LUT, spektrafilm
only makes LUTs, normalised like a print scan, test flash Portra 400 full /
without P / without HSL). Phases 0–6 done: the LUT costs nothing
measurable; tool, core fitter, CLI and the editor's "Fit to a LUT" built
and tested; exported images pass the Unicorn checks; the test firmware is
built and **confirmed on the GR III** (MR029370–388): the camera renders
the fitted looks as the core models them (0.6–1.2 ΔE2000), and the full
film look comes within 1.1–1.7 ΔE2000 of the film.

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

   **Done 2026-10-15.** `Session::lut_info` (what a .cube holds, its values
   and the fitter's camera for the preview) and `Session::lut_fit` (from the
   stock image, so a target on Standard does not change the model); in
   wasm.rs `lut_info` / `lut_fit`, progress through `lutFitProgress`, which
   worker.js posts as `{id, progress}` and bridge.js hands to the caller
   (`api.lut`, `api.lutFit`, not fetch-like: the values go as a typed
   array). The fit runs in 22 s in the browser (native 6.5 s), same
   numbers as the CLI.
4. **Editor**: in a target's Advanced section, "Fit to LUT…": file, input
   space (pre-set from `GR-INPUT`), with P / with HSL, run, report, apply.
   Apply sets source Standard and the parts above, so preview, space meter
   and export work unchanged. The preview can show the LUT itself as the
   "before" side (LUT on the shot's Standard render or linear data), so the
   user sees how close the fit is on their own photo. `edit.lut` survives
   load and export. Docs: `editor.md` (English), i18n for the dialog.
   (The "Compare LUT" view was built and later removed again at the
   user's request: the report and the camera test say enough. The source
   picker shows the LUT instead.)

   **Done 2026-10-15.** `editor/static/lut.js`: button under the source
   (disabled with a hint on targets that cannot take a LUT), dialog (file,
   input pre-set from `GR-INPUT`, P and HSL switches, progress with
   seconds, report table), the fit applied through presets.js'
   `applyTarget` (split out of `applyPreset`) after the target's edits are
   cleared, `hslLoad(…, keep)` for one target. "Compare LUT": a 3D texture
   (RGB16F, trilinear) in the preview shader, input as in the fit. First
   check showed the LUT side 9 levels brighter: the photo's tone curve has
   its own gain (7689 on MR029360, the fitter 6144). The LUT side now gets
   its exposure through the photo's curve and back through Standard's (on
   luminance): 4.0 levels mean difference, equal mean brightness. In the
   browser: fit, compare, export (space ok), preset round trip (same
   export after load), refusals (1D LUT, Cross Process), German texts.
   Session test `session_lut_requests`.

   Finding for the camera: a fitted look sits on Standard's curve at the
   default gain. When the camera picks another gain for a shot, the look
   shifts in brightness the way Standard does.
5. **Checks** (done when all pass): `cargo test`; a spektrafilm LUT fitted
   in the browser loads, previews plausibly (grey ramp, skin, saturated
   colours, checked in the browser) and exports; `tools/verify-tuning.py
   --post IMAGE PRESET` and `tools/verify-targets.py --patched`; three
   complete films fit into the pool, a fourth is refused with the core's
   space error.
   **Done 2026-10-15** for the test firmware (Portra full / without P /
   without HSL) and for three films in one export (Portra 400 on Negative
   Film, Ektar 100 on Positive Film, Vision3 250D on Vivid; `lut_fit
   --out`, 812 bytes free):
   - `fw-pack.py check`: container ok;
   - `verify-tuning.py --post IMAGE PRESET --post-base IMAGE_WITHOUT_P`
     (new): every look, still and live view, hue −4/0/+4, one history,
     against the same preset exported without P. Only the nine YCbCr
     registers may differ, and those are H(k)·B·P within 1 with rows
     256/0/0. The old form (against the source in stock) cannot take
     targets with their own matrix, curves and HSL. Negative control: the
     image without P against itself fails on every target with P;
   - `verify-tuning.py --patched`: the curve selector; R, G, B = the tone
     curve composed with the target's curves;
   - `verify-targets.py --patched`: every look that is not a target equals
     stock.

   The fitted matrix and HSL themselves are checked by the core's tests
   and earlier phases (`plan-look-curves.md`, `multiaxial.md`); here the
   new part is P together with them.
6. **Test firmware** `core/examples/film_flash.rs` → `work/film-test/`
   (FWDC239B.BIN, unpackedFW, preset, HOW_TO_SHOOT.txt), checked with
   `fw-pack.py check`, `verify-tuning.py --post`, `verify-targets.py
   --patched`. Flashing and shooting is the user's.

   **Done 2026-10-15:** `work/film-test/` (FWDC239B.BIN SHA-256
   `6a423e25…e2dc36`, 3.5 KB free), checked as above, HOW_TO_SHOOT.txt in
   German. Model, held out against the LUT: Negative Film 1.61, Positive
   Film (no P) 2.02, Vivid (no HSL) 1.79.

## Test flash (phase 6)

One flash, Portra 400 on Portra Endura (print-scan normalised):

| Image control | Content | Shows |
|---|---|---|
| Standard | unchanged | control |
| Negative Film | M + curves + HSL + P | the full film look |
| Positive Film | M + curves + HSL, fitted without P | what P buys on the camera |
| Vivid | M + curves + P, fitted without HSL | what HSL buys on top of P |

Shots: handheld is fine (alignment with ORB + RANSAC), same light, Daylight
WB, fixed exposure, RAW + JPEG, every slider at 0, grain off, highlight and
shadow correction off (they change the tone curve per shot); two scenes (a
test chart with grey ramp, and an everyday scene with skin, saturated
colours and a bright sky), each in the order Standard, Negative Film,
Positive Film, Vivid, Standard.

Evaluation (as `tools/eval-post-matrix.py`): each camera JPEG against the
LUT applied to the Standard DNG's linear data, and against the core's model
of that look; ΔE2000 per variant. Expected: Negative Film < Vivid <
Positive Film against the LUT, and model vs camera near the floor
(≈ 0.7–1.0 ΔE).

### Result (MR029370–388, 2026-10-15)

Flashed and shot handheld, every slider at 0, highlight and shadow
correction off, grain off. Scene A (MR029370–379): screen chart, Daylight
WB, ISO 100. Scene B (MR029380–388): everyday scene, Auto WB (the shots'
white balance agrees within 0.4 %), ISO 200. Other image controls were
shot too and are not part of the test.

`tools/eval-film.py` with `core/examples/film_eval.rs`: every shot is
rendered from its own DNG (own white balance; input scale, tone gain and
lens shading fitted on the scene's Standard shot as the editor does), so
render and camera JPEG are the same frame and need no alignment. ΔE2000 on
flat 6×6 blocks of the camera JPEG (10 000–30 000 per shot), mean / p95:

| Shot | Look | camera vs model | camera vs film (LUT) | model vs film |
|---|---|---|---|---|
| A MR029370 | Standard (control) | 1.17 / 2.4 | 4.43 / 8.4 | – |
| A MR029372 | Negative Film: full | 0.75 / 2.0 | **1.74** / 5.0 | 1.23 / 3.9 |
| A MR029373 | Positive Film: without P | 0.81 / 2.0 | 1.93 / 5.7 | 1.42 / 4.5 |
| A MR029371 | Vivid: without HSL | 1.19 / 2.6 | 2.01 / 4.5 | 1.23 / 3.8 |
| B MR029380 | Standard (control) | 0.71 / 1.4 | 5.79 / 8.6 | – |
| B MR029382 | Negative Film: full | 0.63 / 1.2 | **1.08** / 1.7 | 1.01 / 1.6 |
| B MR029383 | Positive Film: without P | 0.72 / 1.3 | 1.32 / 2.0 | 1.50 / 2.0 |
| B MR029381 | Vivid: without HSL | 0.62 / 1.3 | 1.44 / 2.3 | 1.27 / 2.2 |

- **The camera does what the model says.** The fitted looks are as close
  to their model as Standard is to its own (0.6–1.2 against 0.7–1.2): P,
  HSL, curves and matrix reach the image as written.
- **The full look is the closest to the film** in both scenes (1.74 and
  1.08); without P or without HSL it is 0.2–0.4 further away. Standard is
  4.4–5.8 away, so the looks clearly move to the film. On these scenes P
  and HSL count about the same (the synthetic samples had HSL as the
  smaller lever).
- The fine match (residual, highlight chroma) is fitted on the Standard
  shot; it helps Standard (1.17 → 0.95, 0.71 → 0.53) but not the film
  looks in scene A (0.75 → 1.14), whose output values differ. The table
  uses the plain model for them.
- The tone gain differs per scene (6545 and 4037 against the fitter's
  6144); the evaluation maps the LUT's exposure through the shot's curve
  and back through Standard's. The film error does not grow with the
  larger gain difference (scene B).

## Film library (2026-10-15)

Every spektrafilm film on every print medium, fitted beforehand and shipped
with the editor (`editor/static/films/`), so a film is one click instead of
a LUT and a 20–60 s fit.

- `tools/spektrafilm-lut.py --all`: from spektrafilm's profiles, every
  film (`stage: filming`, colour) on every print medium (`stage: printing`:
  6 papers, 2383, 2393) for the 16 negatives, the 4 slides scanned directly
  (`io.scan_film`; normalised by the film exposure instead of the print
  exposure); the film's own print (`target_print`, else Portra Endura)
  first. 132 LUTs (33³) and `index.json`, one process per core: 15 s on the
  M5 Pro's 18 cores. Every look's grey at 0.460.
- `core/examples/film_library.rs`: the 132 fits on one thread per core
  (79 s, 15.6 cores busy), written as `spektrafilm.json` (747 KB): per look
  matrix, curves, HSL, P, report, exposure. A fit is the same for every
  target (checked: Portra on Negative Film and on Vivid bit-identical), so
  the library holds one per film and print.
- Fit quality (held out, against the LUT): mean 1.90 ΔE2000, median 1.83,
  1.23–3.36; matrix + curves alone 2.99. The films on their own print:
  Portra 160 1.48, C200 1.49, UltraMax 1.51, Gold 1.54, Pro 400H 1.56,
  Portra 400 1.61, Portra 800 1.64, Xtra 400 1.65, Kodachrome 1.67,
  Vision3 500T 1.67, Vision3 250D 1.74, Ektar 1.80, Vision3 200T 1.81,
  Provia 1.87, Vision3 50D 1.88, Ektachrome 1.93, Verita 2.00, Portra 800
  push 1 2.06, push 2 2.35, Velvia 2.37. Hardest: pushed Portra 800 and
  Xtra 400 on Ultra Endura (2.8–3.4).
- Editor (lut.js): a film group in the source picker, a Print row for
  negatives, the fit applied as a LUT fit; `edit.lut` with `library`,
  `film`, `print` keeps it through presets. Attribution in the note under
  the pickers, the help, the README and `films/CHANGELOG.txt`; the data is
  CC BY-SA 4.0 (spektrafilm's licence for profiles, LUTs and their direct
  derivatives), the code stays MIT.
- Test `film_library_exports`: every look exports on a target.

## HSL as a base (2026-10-16)

At the user's request the fit's HSL no longer shows up as the target's
HSL curves. `multiaxial::Hsl` has a `base` (curves in the same form);
`encode_curves` samples base and user curves per bin and combines them
(hue and luminance add, saturation multiplies: (1 + b)(1 + u) − 1) before
the split into steps and banks. The steps it reports for the editor are the
user's part, taken against the steps of the base alone, so a flat curve
over a film is flat. `lutfit` writes `{curves: flat, base, protect}`; the
library was converted (format 2, identical to a fresh fit). The editor
keeps the base through presets, export and Reset, and marks the section
changed only for the user's curves. Tests: `base_under_the_curves` (base
alone and under flat curves encode exactly as the same values as curves;
saturation multiplies; the user's steps). Checked in the browser: Portra
on Negative Film gives flat curves and an untouched section, the preview
keeps the film's HSL, −20 % saturation shows as −20 % steps, Reset keeps
the base, presets keep both.

## Open points

- Display LUTs: "LUT on Standard" assumes the LUT was made for an image
  like the camera's Standard JPEG. LUTs made for log footage need their
  input encoding (later).
- The camera's highlight chroma roll-off is not modelled (as before).
