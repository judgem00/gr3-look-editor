# Spektrafilm targets: is a post-gamma matrix needed?

Offline prototype (`tools/spektrafilm-fit.py`, 2026-10-15). It fits spektrafilm
film renderings with what a GR III look can express, with and without a free
3×3 matrix after the tone curve, and compares the mean ΔE2000. Nothing was
flashed. These numbers come from the model, not from the camera.

## Result

Mean over 7 films, held-out samples (other colours and exposures than the
fit), ΔE2000:

| Variant | all | p95 | ColorChecker | skin | warm | saturated | grey |
|---|---|---|---|---|---|---|---|
| A look matrix + RGB curves | 2.46 | 5.75 | 2.37 | 2.68 | 3.19 | 3.56 | 0.78 |
| B A + HSL | 2.20 | 4.85 | 1.93 | 2.24 | 2.80 | 2.99 | 0.66 |
| C A + post-gamma 3×3 | 1.84 | 3.77 | 1.44 | 2.26 | 1.92 | 2.30 | 0.47 |
| D A + HSL + post-gamma 3×3 | **1.68** | 3.55 | 1.37 | 1.53 | 1.71 | 2.17 | 0.48 |

Per film (all, A → B → C → D):

| Film (print) | A | B | C | D |
|---|---|---|---|---|
| Portra 400 (Portra Endura) | 2.45 | 2.19 | 1.84 | 1.67 |
| Ektar 100 (Portra Endura) | 2.73 | 2.47 | 2.08 | 1.88 |
| Gold 200 (Portra Endura) | 2.41 | 2.15 | 1.77 | 1.64 |
| Pro 400H (Crystal Archive II) | 2.27 | 2.00 | 1.72 | 1.63 |
| C200 (Crystal Archive II) | 2.43 | 2.26 | 1.80 | 1.56 |
| Vision3 250D (2383) | 2.57 | 2.20 | 1.95 | 1.83 |
| UltraMax 400 (Portra Endura) | 2.36 | 2.10 | 1.74 | 1.56 |

- **The post-gamma matrix is the larger lever.** It gains more than HSL on
  every film (all: −0.62 vs −0.26). It gains most on warm and saturated
  colours (warm 3.19 → 1.92). The colour mixing of a film print happens after
  the tone curve. A matrix before the curve (look matrix, MultiAxial) cannot
  reproduce that.
- **HSL still adds on top**, mainly on skin (C → D: 2.26 → 1.53) and on the
  p95. Without the post matrix, HSL makes up less than half of the gap.
- **The matrices are moderate** and similar across films: diagonals 0.74–1.0,
  off-diagonals at most 0.21, and the G and B rows always take from their
  neighbours (e.g. Portra `[0.97 0.09 −0.07; 0.08 0.81 0.11; 0.01 0.14 0.85]`).
  That fits the 9-bit CSP coefficients (256 = 1.0) with room to spare. It
  also resembles the CCA1 matrices that gr3-film-mods fitted with the same
  goal (§22g/§22i there).
- The order matches gr3-film-mods: they measured on the camera that the post
  matrix brought about 0.8–1.0 ΔE (v9 → v10) and the hue sectors 0.1–0.3
  (v16).

## Model

```
x   = M L                     look matrix, rows sum to 1 (6 free values)
x   = S[sector(x)] x          HSL: 12 equal hue sectors, hue/sat/lum each (band_matrix), hard edges, weight 1
b   = base(SCALE x)           Standard tone curve at default sliders (GammaTuning, from core)
o_c = C_c(b_c / 64) / 16384   channel curve per channel, 33 knots, monotone
o   = P o                     post-gamma 3x3, rows sum to 1 (6 free values)   [variants C, D]
```

- L = scene-linear sRGB after the sensor CCM. SCALE puts 18 % grey at sRGB
  0.46 on the Standard curve (index 23.3 of 256).
- Target: spektrafilm 0.3.4 in `lut_mode` (no grain, halation or other
  spatial effects; fixed exposure; print exposure 1.0; no scanner white or
  black correction), linear sRGB in and out, sRGB-encoded.
- Fit: least squares on the per-sample ΔE2000. Greys are weighted 25×,
  ColorChecker 6×. Samples: grey ramp, ColorChecker at 5 exposures, random
  colours, a hue sweep. Evaluation is on a second sample set with other
  colours and a shifted exposure.

## Limits

- Model only. The camera adds about 0.7 ΔE as measurement floor
  (gr3-film-mods §33b). The camera's highlight chroma roll-off is not
  modelled.
- The HSL model is more generous than the editor: 12 sectors with 12
  adjustments, while the editor keeps bank 0 unchanged (11) and quantises to
  1/1024. Sector edges are fixed.
- No quantisation of the post matrix and no hue-setting interaction: the
  camera would store H(hue)·P in the CSP for each of the 9 hue steps.
- Spektrafilm's print white is about 0.70 linear (sRGB 0.85). The curves
  reproduce that, so the looks are a little flat at the top. A real preset
  may want a print exposure or white point chosen per film.
- Per-sample ΔE2000 weighs all colours equally. Perceptual priorities (skin)
  would need their own weights.

## Next

1. Find out whether the per-look hue tables (`TONE_TABLE`, looks.rs
   `hue_matrices`) are shared between looks. If not, P can be written as data
   (9 matrices H(k)·P). Otherwise a look gets its own copy in free space, as
   with MultiAxial.
2. Flash test (one variant per image control, Standard as control): one film
   with and without P.
3. Then add the fit to the editor (target → matrix + curves + HSL + P).
