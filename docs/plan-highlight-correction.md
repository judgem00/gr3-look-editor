# Highlight correction with a film look: camera test

Question: does the camera's own **highlight correction** give the fitted film
looks the highlight range they need, without moving the rest of the look?

## Why

- Standard's tone curve reaches white at +3.4 EV over 18 % grey; the 12-bit
  tone input ends at +3.45 EV (core emulator, default sliders, gain 6144).
- Spektrafilm's shoulder runs on: grey on Portra 400 / 2383 is 0.887 at
  +2.5 EV, 0.928 at +3.25, 0.948 at +4.0 (Ektar, Vision3 alike; Gold on
  Portra Endura 0.920 / 0.957 / 0.971). The camera cuts the film's curve at
  about 0.93–0.95 while it still rises: a hard edge into a flat white patch.
- Highlight correction is said to expose one stop less (ISO 200 minimum)
  and lift the image through the tone curve. If so, the sensor keeps data up
  to about +4.4 EV, where the film's shoulder is nearly flat. The editor does
  not model it yet (docs/editor.md: "probably works through exposure and the
  tone curve's gain").

## Prerequisites

- No flash. The camera runs the film firmware from phase 6
  (plan-spektrafilm-fitter.md: Negative Film = full film look; main repo
  `work/film-test/FWDC239B.BIN`, Portra 400 / Portra Endura). If it runs
  another build, the Negative Film slot is whatever that build put there;
  write the build down.
- RAW + JPEG, every slider at 0, grain off, **shadow correction off**,
  Daylight WB, aperture priority at f/5.6 (not M: HC may set its −1 EV
  through the metering), spot metering on the reference grey in scene A,
  **ISO 200 fixed** in every shot
  (highlight correction needs ISO ≥ 200; the same ISO keeps noise
  comparable), exposure compensation 0 unless the table says otherwise.
- Tripod or a steady rest helps but is not needed: each shot is rendered
  from its own DNG.

## Scenes

- **A, controlled:** the screen chart from `tools/highlight-chart.py`
  (PNGs in `work/highlight-test/` of the main repo), full screen in a dark
  room, screen at full brightness, night mode, True Tone, auto brightness
  and HDR off. Everything sits relative to a reference grey at 1/32 of the
  screen white, so with **spot metering on the reference patch** (AE linked to the AF point;
  the patch is a checkerboard for the AF, its linear mean the reference grey) the white
  is +5 EV over grey: past the clip point with and without HC. Grey wedge
  −4 … +5 EV in 1/3 EV, a smooth log ramp 0 … +5 EV, and R, G, B, skin,
  yellow, cyan at +1 … +5 EV. Slightly defocused or from a distance against
  moiré, shutter 1/60 s or longer against banding. On the MacBook Pro the
  "Photography (P3-D65)" preset (fixed brightness, no HDR); the reference
  grey is then dim, so f/2.8 and about 1/10 s on a rest for scene A. The chart's EV labels are
  what it sends; the evaluation reads scene EV from the RAW, so the
  screen's response need not be exact.
- **B, everyday:** backlit sky with structured clouds, a sunlit bright wall
  or façade, and skin or a mid-tone object in the frame. That is where the
  clipping shows.

## Shot list (per scene, in this order)

| # | Image control | Highlight correction | Exp. comp. | Purpose |
|---|---|---|---|---|
| 1 | Standard | Off | 0 | control, fit reference for the Off shots |
| 2 | Negative Film | Off | 0 | the film as it is today |
| 3 | Standard | On | 0 | fit reference for the On shots; what HC does to the curve |
| 4 | Negative Film | On | 0 | the film with HC |
| 5 | Negative Film | Off | −1 | HC = only −1 EV? (compare with 4) |
| 6 | Standard | Off | −1 | fit reference for 5 |
| 7 | Standard | Off | 0 | repeat of 1: has the light changed? |

14 shots for both scenes. Write down which file number is which; the
makernote check below should confirm the HC column.

## Evaluation

All on the core's model; `core/examples/film_eval.rs` and
`tools/eval-film.py` stay as they are. Each HC state is its own "scene" for
eval-film.py, with its own Standard reference: `1,2`, `3,4`, `6,5`.

1. **Does the DNG record HC?** Read makernote `0x0079` (byte 1 is shadow
   correction; byte 0 or a neighbouring tag is expected to be highlight
   correction) and the exposure tags for shots 1/3/6. If it is recorded,
   `dng.rs` gets the field, as with `shadow_correction`.
2. **What HC does to the RAW.** Shot 3 against shot 1 on the linear data:
   the median ratio of pixels that clip in neither shot (expected 0.5, so
   −1 EV), and the share of clipped pixels (white level, at the smallest
   gain as `fit::highlight_clip`) in each shot.
3. **What HC does to the tone curve.** Fitted gain and scale of shot 3 against
   shot 1 (`fit_camera_tone`), and camera vs model ΔE2000 for shot 3. If the
   plain GammaTuning curve with the fitted gain gets shot 3 as close as the
   control (≈ 0.7–1.2 mean), HC is exposure + gain and the editor already
   covers it through the per-shot fit. If not, HC has a curve of its own
   (the table variant at `0x53dc1488` is the first suspect) and needs
   modelling.
4. **Does the film look stay the same?** eval-film.py on shots 4 and 2:
   camera vs model and camera vs LUT, mean / p95. Then shot 4 against shot 2
   directly, on blocks with JPEG Y < 0.7 (midtones and shadows): how much
   does HC move the look where it should not?
5. **How much highlight is gained.** New small script
   `tools/eval-highlight.py`: per shot, camera JPEG Y over scene EV
   (log2 of the shot's linear luminance relative to grey, corrected for the
   exposure difference from step 2), binned in 1/8 EV, plotted for shots 2,
   4, 5 against the LUT's grey curve. Read off where each reaches 250/255
   and the slope just before it. Plus: of the pixels that are ≥ 250 in
   shot 2, the share still ≥ 250 in shot 4.
6. **The cost.** Noise (standard deviation on flat blocks of the dark
   object) for shots 2, 4, 5: the shadows are one stop under.

## What decides

- **HC is the way:** clip point at least 0.7 EV higher (step 5), midtones
  within about 1 ΔE of shot 2 (step 4), camera vs model as good as the
  control (step 3). Then HC On becomes the recommendation for the film
  looks, and the fitter's highlight roll-off (variant 2) is fitted to the
  clip point with HC.
- **HC helps, the look moves:** midtones more than about 1.5 ΔE away. Then
  the fitter fits at the HC gain (the gain fitted on shot 3) instead of
  6144, perhaps as a second variant of the film.
- **HC = −1 EV only:** shot 4 ≈ shot 5 with the brightness lifted. Then
  "expose −1 EV + a brighter curve" is the lever we can build ourselves
  (a gain or key on the film targets), without the camera's HC.
- **No gain:** the clip point does not move. Then only the fitter's
  roll-off is left, with the clip at +3.45 EV.

## Result (MR029426–432, 2026-10-16)

Scene A only (the screen chart on the MacBook Pro, "Photography (P3-D65)",
in a bright room), f/2.8, ISO 200, spot metering on the checkerboard,
Auto WB (all shots within 0.4 %). The order differs from the list above:

| Shot | Look | HC | Shutter | Exp. comp. |
|---|---|---|---|---|
| MR029426 | Standard | off | 1/8 | 0 |
| MR029427 | Negative Film | off | 1/8 | 0 |
| MR029428 | Standard | on | 1/8 | 0 |
| MR029429 | Negative Film | on | 1/8 | 0 |
| MR029430 | Standard | off | 1/15 | −1 |
| MR029431 | Negative Film | off | 1/15 | −1 |
| MR029432 | Standard | off | 1/10 | 0 (control) |

The control metered 1/3 EV differently (1/10 instead of 1/8), probably a
slightly different spot; the evaluation normalises by shutter time.

1. **The DNG records HC** in makernote `0x0069` (exiftool:
   DynamicRangeExpansion, "On; Enabled" vs "Off"), not in `0x0079`.
2. **HC reads the sensor out one stop lower at the same exposure.** Same
   shutter, aperture and ISO label, but RAW green median 1878 → 936 (×0.498)
   and black level 128 → 64. Saturated RAW pixels in the frame 4.9 % → 0 %.
   Unlike −1 EV exposure compensation it keeps the exposure time, so the
   same light reaches the sensor.
3. **HC is exposure + GammaTuning gain.** Fitted gain 7689 → 4037 (scale
   3950 → 3646); camera vs model (eval-film.py, ΔE2000 mean / p95):

   | Shot | Look | model | model fine | film (LUT) |
   |---|---|---|---|---|
   | MR029426 | Standard, HC off | 1.19 / 2.5 | 0.79 / 1.6 | 3.98 / 6.7 |
   | MR029427 | Negative Film, HC off | 1.37 / 3.9 | 1.08 / 1.8 | 2.12 / 5.0 |
   | MR029428 | Standard, HC on | 0.97 / 1.8 | 0.73 / 1.6 | 3.56 / 6.0 |
   | MR029429 | Negative Film, HC on | 1.18 / 1.8 | 0.89 / 1.5 | 1.93 / 3.8 |
   | MR029430 | Standard, −1 EV | 0.91 / 2.0 | 0.71 / 1.4 | 3.90 / 9.0 |
   | MR029431 | Negative Film, −1 EV | 0.94 / 2.1 | 0.75 / 1.4 | 1.65 / 2.8 |
   | MR029432 | Standard (control) | 1.28 / 3.3 | 0.82 / 1.6 | 4.03 / 10.3 |

   The HC shots fit as well as the control: the editor's per-shot fit
   (gain, scale) covers HC, no curve of its own (`0x53dc1488` not needed).
   Negative Film with HC is also a little closer to the film (1.93 vs 2.12).
   The gain is not an HC constant: it varies per scene with the metering
   anyway (phase 6: 6545 and 4037 without HC).
4. and 5. **Highlight range** (`tools/eval-highlight.py`, each shot in its
   own frame, scene EV from its own RAW with lens shading corrected, relative
   to the chart background, which sits about 0.3 EV above the camera's grey;
   JPEG luma 0–255, a few bins are disturbed by patch edges):

   | Scene EV | Std off | NF off | Std on | NF on | LUT grey |
   |---|---|---|---|---|---|
   | −0.94 | 90.7 | 89.5 | 86.2 | 87.3 | 72.0 |
   | +0.06 | 133.7 | 137.8 | 130.3 | 135.5 | 121.2 |
   | +1.06 | 183.2 | 198.9 | 175.0 | 190.9 | 178.1 |
   | +1.94 | 224.4 | 234.4 | 209.0 | 228.0 | 219.6 |
   | +2.44 | 249.9 | 248.2 | 227.6 | 237.3 | 233.9 |
   | +2.56 | 253.0 | 248.8 | 234.1 | 238.7 | 236.4 |
   | +2.94 | clipped | clipped | 242.5 | 245.5 | 241.9 |
   | +3.31 | | | 248.2 | 248.2 | 245.1 |
   | +3.69 | | | 253.9 | (clipped) | 247.1 |

   - Without HC the RAW clips at **+2.5 EV** (about +2.8 over the camera's
     grey): this is the bottleneck, not the tone curve. Standard runs into
     255, Negative Film into its fitted white (≈ 249) and stays flat: the
     hard edge.
   - With HC there is RAW data up to **+3.8 EV**; Standard reaches 250 at
     +3.44, Negative Film rolls off gently from 237 (+2.4) to 248 (+3.3),
     close to the film's own shoulder (LUT 234 → 245). **About +1.1 to
     +1.3 EV more highlight range.**
   - Visually (camera JPEGs): without HC the wedge is white from about
     +3 (chart label), skin, yellow and cyan go white from +3.5 and blue
     shifts towards light blue; with HC the wedge stays distinct up to +5
     and the colour patches keep their hue.
   - **The cost in the look:** with HC the midtones are 2–3 levels darker,
     the upper midtones (+1 to +2 EV) 8–15 levels; the shoulder starts
     earlier, the image gets a little flatter at the top.
6. Noise was not measured (the screen's blacks bloom). HC keeps the
   exposure time, so shot noise stays as without HC; the shadows get the
   digital lift of one stop from a lower readout gain. −1 EV compensation
   (MR029430/431) halves the light and is darker overall: HC is not the
   same as −1 EV.

### Decision

The first case with a caveat: HC gains more than a stop of highlights and
the core models it as well as the plain look, but it moves the upper
midtones (beyond the 1 ΔE asked above). Recommendation: **HC on for the
film looks**, and the fitter learns about it: fit the film on the camera
model at an HC gain (or across a gain range, since the gain varies per
scene), with the clip at the HC RAW limit, so the upper midtones match the
film again and the highlight roll-off lands where the sensor ends. A second
scene (B, outdoors) is still open.

## Editor option: films fitted for highlight correction

Goal: the user picks whether a film target is fitted for highlight
correction off (as now) or on; the editor offers a second, precomputed film
library for "on", and a loaded DNG sets the choice from its makernote.

### Model

- HC reads the sensor out one stop lower and halves the GammaTuning gain
  (result above). The fitter's camera model takes Standard's tone curve at
  the default gain 6144; with HC it takes gain **3072**. `Camera` places 18 %
  grey at sRGB 0.46 on that curve as before, so the scale follows by itself:
  grey at tone index 11.6 instead of 23.3, the 12-bit input ends at
  +4.45 EV over grey instead of +3.46.
- The samples reach that far: the grey ramp and the random colours get
  the extra headroom on top (log2 of the two scales, ≈ 1.0 EV), so the fit
  sees the film's shoulder up to the HC input end. With HC off nothing
  changes (extra 0): the existing library and the camera-confirmed fits stay
  bit-identical.
- Not modelled: the sensor clips a little before the 12-bit input ends
  (without HC measured at about +2.8 EV over the camera's grey, with HC
  about +4.1), and the camera's gain varies per scene with the metering.

### Parts

| Part | Change |
|---|---|
| `lutfit.rs` | `Camera::with_hc(img, target, hc)` (gain 3072), `Camera::new` = off; `Samples::with_extra`; `fit` uses the camera's extra headroom |
| `session.rs` | `lut_fit` takes `hc` from the body; `edit.lut.hc` in the spec; shot meta `highlight_correction` |
| `dng.rs` | makernote `0x0069` byte 0 → `highlight_correction: Option<bool>` |
| `film_library.rs` | `--hc`: fits every film with HC, writes `films/spektrafilm-hc.json` (`"highlight_correction": true`) |
| `lut.js`, `index.html`, `i18n.js` | a "Highlight correction" row in the pick panel (off / on) for LUT targets; a film target takes its looks from the matching library and records `hc` in `edit.lut`; switching re-applies the film (like a print change); the LUT dialog fits with the same setting; a loaded DNG sets the row from its makernote and warns when the current film is fitted for the other setting |
| `films/CHANGELOG.txt`, help text | the second library |

### Checks

- `cargo test`: HC off fits unchanged (the existing tests and golden
  answers); `Camera::with_hc` puts grey at 0.46 with about +1 EV more input
  headroom; a fit with HC runs and records it; `dng.rs` reads the flag on
  MR029426 (off) and MR029428 (on).
- In the browser: a DNG with HC sets the row to on; a film picked then
  comes from the HC library; switching re-applies it; the preview of
  MR029429 with the HC fit against the camera JPEG.
- Camera test of an HC-fitted film: one flash, later (flash protocol).
