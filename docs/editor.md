# Look Editor

Edits the colour matrices and tone curves of the GR III and GR IIIx image
controls with a live preview on a DNG from the camera, and writes a flashable
update file.

Runs entirely in the browser: drop in Ricoh's update file (`fwdc239b.bin` for
the GR III, `fwdc243b.bin` for the GR IIIx), then a DNG. The Rust core (`core/`, as WebAssembly) reads the firmware and the
DNG, including the shot's image control and settings from the makernote.
Nothing is uploaded.

## What the preview computes

```
camera RGB -> WB (AsShotNeutral or colour temperature), clipped at the smallest gain -> lens shading correction -> sensor CCM -> look matrix -> A[saturation] -> (mono mix)
           -> MultiAxial (colour ranges) -> shadow correction -> camera tone curve (per channel for Cross Process)
           -> post matrix P (a target's) -> hue rotation (YCbCr) -> (residual correction, highlight chroma roll-off)
```

Saturation and hue are the camera's image control settings; their matrices
come from the firmware ([docs/look-mapping.md](look-mapping.md)).

- **Matrices** come from the firmware image
  ([docs/look-mapping.md](look-mapping.md)).
- **The tone curve is computed by the camera's original code** (`GammaTuning`,
  RTOS `0x536e7b64`), run in a small ARM interpreter
  (`core/src/arm.rs`) on the (edited) base curves, with
  key/contrast/highlights/shadows
  ([docs/tone-curves.md](tone-curves.md)).
- **Lens shading (vignetting) correction:** the camera brightens the edges
  of the linear image to correct the lens. At f/2.8 that is ×1.14 at half
  the radius and ×1.43 at 80 % towards the corners, the same in every image
  control. The DNG carries no data for it, so it is fitted per DNG against the
  embedded camera JPEG (`fit::fit_vignette`: the JPEG goes back through the
  inverted tone curve, then a radial polynomial is fitted). That covers every
  aperture. Retro darkens the edges itself and HDR Tone changes them through
  its local tone mapping. Shots with these image controls get the typical
  f/2.8 correction (`DEFAULT_VIGNETTE`). The preview applies the correction
  before everything else; it can be switched off.
- Input scaling and gain are fitted per DNG against the embedded camera JPEG,
  after the lens shading correction. The **residual correction** absorbs the
  camera's image-dependent shadow correction. It is still fitted and carried
  in the photo's metadata, but the preview no longer applies it: the
  "Fine-match to camera JPEG" switch was removed, the code stays in
  `core/src/fit.rs` and `editor/static/app.js` (state `residual`). Without
  the shading correction it absorbed the brighter edges too and fitted
  worse. On MR024685 the mean
  difference to the camera JPEG drops from 6.9 to 1.4 levels (8 bit). The
  residual is fitted on all pixels. Leaving out the ones the camera clipped
  kept only the darker ones near white, and white came out at 247 instead of
  255.
- **Shadow correction:** the camera's own, from its firmware
  (`core/src/tonecontrol.rs`; analysis in gr3x-fw-hack
  `docs/tone-curves.md`). Each pixel on linear RGB is multiplied by a gain
  from a 1024-entry table, indexed by its 12-bit value / 4. The table is
  1 + S · (base − 1), with the base at `0x53dc1f5c` (2.0 at black, 1.0 from
  about 15 %) and S = 0.25 / 0.5 / 1 for Low / Medium / Strong. The DNG
  stores the level in makernote tag `0x0079`; "Auto" there is level 4, which
  the firmware treats as off. The fit against the camera JPEG uses the shot's
  level. In the preview it is one of the camera settings ("Shadow
  correction"), loaded from the DNG with the others. On MR022009 (Strong)
  the difference to the camera JPEG drops from 7.2 to 4.0 levels without the
  fine match (shadows 17.5 → 3.7). Which value of a pixel indexes the table
  is done in hardware; the largest channel fitted best. The camera's
  highlight correction is not modelled. Its table variant (`0x53dc1488`) did
  not fit MR022009, so it probably works through exposure and the tone
  curve's gain, which the fit covers.
- **Clipped highlights:** where the sensor clipped, white balance would turn
  "very bright" into a strong colour (red gain ~3: yellow or pink lights).
  As in the camera, all channels are cut at the level of the channel with the
  smallest gain, so such lights stay white (`fit::highlight_clip`).
- **Chroma roll-off in the highlights:** the camera takes saturation out of
  bright colours. From about Y 180 of 255, chroma falls to ×0.55 near white,
  measured on three shots in Standard and Negative Film. It is fitted per
  shot against the camera JPEG, one factor per 16 levels of Y
  (`fit::fit_highlight_chroma`, typical table `DEFAULT_HIGHLIGHT_CHROMA`).
  Like the residual it is fitted but no longer applied in the preview.
  Error above Y 176: MR024685
  5.1 → 3.0, MR024661 7.6 → 4.8, MR022009 5.9 → 4.4 levels. Where the
  camera does it is not known; the CSP block's chroma limits are a candidate.

- **Post matrix P:** a target can have a 3×3 after the tone curve
  ([plan-post-matrix.md](plan-post-matrix.md)), set under Advanced or from a
  preset. The preview applies it to the tone-mapped RGB before the hue
  rotation, as the camera's YCbCr stage does. When a shot was taken with a
  look that has P, the fits against the camera JPEG first undo P on the JPEG
  (`postmatrix::undo`); otherwise the highlight roll-off would learn P's
  desaturation.

Views: **Edited**, **Before / After** (firmware original against the edit,
colour and tone curve), **Compare camera**, **Camera JPEG**.

## Editing

Layout of the interface: the editor builds a look of your own into the slot
of **Cross Process**; the other image controls stay unchanged. Tabs:
**1 Base** (the colours of an existing image control as a starting point,
copied into a separate matrix for Cross Process, or an extra base, see
below), **2 Colour**, **3 Curves**
(Cross Process with Color Tone 1–3, or another colour image control; RGB
curves linked or per channel, colour wheels; the curves always start
linear), **4 Export**. Other image controls and the tone curve
for all image controls are under "advanced". Technically Cross Process gets
its own ColorMatrix set and its own matrix in the free area `0x53002080…`
(`core/src/looks.rs`, `private_set_at`; the set is 21 words long).

- **Colour matrix** per image control, directly or via saturation/hue.
- **Calibration** as in Lightroom: hue and saturation of the primaries red,
  green and blue. This can be expressed exactly as a matrix (`Look =
  Saturation · Hue · Base · Calibration`): the primaries are rotated around
  the grey axis or scaled, then each column is scaled so that white stays
  white. The other primaries keep their hue (< 1° crosstalk).
  Presets store the sliders (`matrix_edits`).
  Bleach Bypass and HDR have variants with their own matrix.
- **Colour curves (HSL)** per image control: Hue vs Hue, Hue vs Sat and Hue
  vs Lum, as in DaVinci Resolve. They act in the camera's hue-selective stage
  ([docs/multiaxial.md](multiaxial.md)). The x axis runs red → yellow →
  green → cyan → blue → magenta over the stage's hue coordinate. The
  background shows each hue after the camera's tone curve, with the curve's
  effect from bottom to top. A new curve has neutral points at the six hues,
  so that moving one point bends the curve only up to its neighbours. The
  stage cannot follow a smooth curve. The core (`/api/hsl`) therefore splits
  the curves into the steps that come closest: at most 5 per quarter of the
  circle (dotted lines) and 11 different adjustments. The editor draws these
  steps dashed over the curve, and the preview runs exactly them on the GPU.
  At most 5 image controls can have HSL. "Protect shadows and highlights"
  sets the stage's weight curve over brightness. Presets store the curve
  points (`hsl`). Presets with the earlier ranges (bands) are converted to
  points.
- **Tone curve:** click to add points, drag them, double-click to remove.
  The curve is applied to the 15 base curves (optionally only to individual
  contrast families); the camera then blends as usual. It **applies to all
  image controls**. The lower graph shows the camera curve that actually
  takes effect.
- **Channel curves (R, G, B)** of the Cross Process variants **Color Tone
  1–3** (the Color Tone setting in the camera menu; internally preset 1–3)
  and Random (not reachable from the menu). When a Cross Process DNG is
  loaded, the editor selects the shot's set. Edited like the tone curve; a
  click copies a single channel or a whole set from another variant.
- **Channel curves of other image controls:** the "Image control" selector on
  the curves tab also offers Standard, Vivid, Positive Film, Bleach Bypass,
  Retro, HDR Tone and Negative Film, with one set each (the Bleach Bypass and
  HDR variants use their look's). They are edited exactly like Cross
  Process's. In the firmware a small code patch picks each look's curves
  ([tone-curves.md](tone-curves.md#channel-curves-for-every-colour-look-selector));
  it is only written when at least one of these looks has curves. In the
  editor their channels are indices 12–32 (three per look, in the order
  above); the export turns them into `look_curves`. The preview applies the
  curves of the image control it shows.
- **Free space:** the curves of other image controls and the HSL sets share
  about 13 KB in the firmware. A meter on the curves and export tabs shows
  what is left; an export that does not fit is refused with the numbers.
- **HDR effects off:** with HDR Tone selected on the curves tab, this switch
  turns off its local tone mapping and strength, so HDR becomes a plain
  image control with a colour matrix of its own (no longer shared with
  Standard). Key, contrast, highlight and shadow then stay at 0 in the
  camera ([look-mapping.md](look-mapping.md#hdr-without-its-effects)).
- **Grain:** the third picker gives the target grain (weak, medium,
  strong; the firmware's 3, 2, 1), also where the
  camera menu has none (every colour look but Negative Film). The camera adds
  it after development, also in in-camera RAW development; the preview does
  not show it. On Negative Film it replaces the menu's grain setting
  ([grain.md](grain.md)).
- **Colour wheels** (shadows, midtones, highlights, global, with luminance
  and balance) as in Lightroom Color Grading. They are converted into the
  R/G/B channel curves of the selected set and therefore act
  like RGB curves, not in a perceptual colour space. With "Random" the camera
  redistributes the curves per shot, so a tint changes colour. The preset
  stores the wheels; they stay editable after loading.
- **Cross Process with another look's data:** for Cross Process, "take look
  data from …" (e.g. Negative Film) plus "RGB linear" gives a look that
  computes like Negative Film and carries RGB curves
  ([docs/tone-curves.md](tone-curves.md)).
- **Extra bases:** besides the image controls of the camera, the base list
  offers **Oldschool Positive Film** (rich red, muted green and yellow, deep
  blacks). It sets the Cross Process matrix (a fixed matrix times the
  Standard matrix of the loaded firmware) and one tone curve on R, G and B of
  Color Tone 1–3; the camera's own settings keep working on top. Choosing
  another base removes both again as long as the curves are untouched, and a
  preset made from it is recognised on load. Extra bases are defined in
  `editor/static/app.js` (`BUILTIN_BASES`); an optional
  `editor/static/_local/bases.json` (ignored by git) adds bases or replaces
  one with the same id.
- **Camera settings** (saturation, hue, key, contrast, highlights, shadows,
  exposure, white balance) affect the preview only. A newly loaded photo
  starts with them at their defaults; only the white balance comes from the
  DNG. **Load preview settings from DNG** takes the shot's values from the
  makernote (for a Cross Process shot also its Color Tone). The match to
  the camera JPEG is fitted with the shot's settings, so "Compare camera"
  only lines up after loading them.
- **White balance (preview only):** "as shot" uses the DNG's AsShotNeutral.
  Moving **Colour temperature** (2500–10000 K), **Blue – Amber** or
  **Green – Magenta** (14 steps each) switches to white balance by colour
  temperature, computed like the camera's Colour Temperature setting with
  fine adjustment (`editor/static/wb.js`). Each camera has its own
  calibration; the editor reads it from the DNG makernote (tag `0x0221`,
  KelvinWB) and falls back to a typical one if the tag is missing. The
  controls start from the shot's white balance (the note shows it, e.g.
  "≈ 5570 K, green – magenta M4"), so switching changes nothing until you
  move them. The gains act on camera RGB before the colour matrices, where
  the camera applies white balance too
  ([docs/look-mapping.md](look-mapping.md)). The sliders show their effect as
  a colour gradient; a low colour temperature renders the photo blue, a high
  one warm. Not exported, not stored in presets.
- **Fit to a LUT** (button under the source; Standard, Vivid, Positive
  Film, Retro, Negative Film): the core fits the target's look to a 3D LUT
  (`.cube`, up to 65³) -- look matrix, RGB curves, HSL and post matrix,
  each optional except matrix and curves -- on the camera's own encodings
  (`core/src/lutfit.rs`, [plan-spektrafilm-fitter.md](plan-spektrafilm-fitter.md)).
  The LUT's input is asked for: **a finished image** (the LUT applied to the
  camera's Standard rendering, the usual "creative LUT"), **linear sRGB**,
  or **GR log** (`tools/spektrafilm-lut.py` writes it into the file as
  `# GR-INPUT: grlog -8 12`; spektrafilm films normalised like a print
  scan). The fit takes about 20 s in the browser (the worker reports each
  pass) and replaces the target's source (Standard), colour, curves, wheels
  and HSL; everything stays editable. The dialog shows ΔE2000 against the
  LUT on test colours the fit did not see, for matrix + curves, the model
  with HSL and P, and the result as exported. The preset keeps
  `edit.lut` (file name, title, SHA-256, input, report), not the LUT.
  The source picker then shows the LUT (its title, else the file name),
  and so do the target list, the export list and the preset notice; the
  look still computes on Standard's tone curve. Picking another source
  keeps the fitted values and drops the LUT's name. The fit's HSL goes
  in as a **base** under the HSL curves (`hsl.base`, `multiaxial::Hsl`):
  the curves stay flat for the user's own adjustments, and the core
  combines both before it encodes the stage -- hue and luminance add,
  saturation multiplies. The dashed steps show the user's part (against
  the steps the base gets alone, so flat curves show flat steps); the
  section's dot and its Reset act on the user's curves only, the base
  stays. A fitted film already uses most of the stage's 11 adjustments, so
  further curves share them with the film's. The same holds for matrix,
  RGB curves and post matrix: the fit is the target's **start**
  (`edit.lut.start`, kept in presets; derived from the fit in presets
  from before), so "Reset curve" and the sections' Reset go back to the
  film, not to the source, and the dots mark only the user's changes on
  top. The fitted look sits
  on Standard's curve at the default gain; the camera sets the gain per
  shot at runtime, and the look follows it like any look does.
- **Films** (source picker, group "Films (spektrafilm)"; the same five
  targets): 20 films from spektrafilm, every negative on each of 8 print
  media (6 papers, the cine print films 2383 and 2393) and 4 slide films
  scanned directly, 132 looks, fitted beforehand
  (`editor/static/films/spektrafilm.json`, 747 KB, loaded once the firmware
  is in). Choosing a film applies its fit at once, like a LUT fit; for a
  negative a **Print** row under the source picks the print medium (the
  film's own first, each with its ΔE2000). The note under the pickers
  carries the attribution spektrafilm's licence asks for. Built by
  `tools/spektrafilm-lut.py --all` (one process per core, 15 s on 18
  cores) and `core/examples/film_library.rs` (one thread per core, 79 s);
  `core/tests/lutfit.rs` exports every look. The fit does not depend on the
  target (all colour looks share the YCbCr and hue data it uses), so one
  fit per film and print serves every target. The data is CC BY-SA 4.0
  (`films/LICENSE.txt`, `films/CHANGELOG.txt`), the editor's code MIT.
- **Film fitted for highlight correction** (a box of its own under the
  print, for the same five targets, with a short explanation: it switches
  nothing in the camera): every film is fitted twice, for the camera's highlight
  correction off and on (`films/spektrafilm-hc.json`, `film_library --hc`,
  76 s). With it on the camera reads the sensor out one stop lower and
  halves the tone curve's gain; the model does the same
  (`lutfit::Camera::with_hc`, gain 3072) and the fit's samples reach a stop
  further into the highlights, so the film rolls off where the sensor now
  ends (docs/plan-highlight-correction.md). A film target takes its looks
  from the library of its setting and records it (`edit.lut.hc`); changing
  it fits the film again from the library (like a print change). The
  LUT dialog fits a LUT for the same setting. A loaded DNG sets it
  from its makernote (`0x0069`) for new fits and notes when the selected
  target is fitted for the other setting. The preview needs nothing extra:
  its per-DNG fit of gain and scale covers the camera's highlight
  correction.

- **GR-Log (video)** (source group "Video" for Standard, Vivid, Positive
  Film, Bleach Bypass and Retro; docs/plan-gr-log.md): a log tone curve for
  video in manual exposure, on Standard's colours, the same curve on R, G
  and B (`core/src/grlog.rs`, `grlog −6 10`). The target records it like a
  film (`edit.lut.library: "grlog"`, encoding, `hc`), so resets go back to
  it and another source drops it; grain stays. The highlight correction box
  builds it on the tone curve with or without highlight correction. A box
  "Post LUTs for GR-Log" shows the range the camera stores as log and saves
  `.cube` LUTs (33³) to Standard, to Rec.709 or through a LUT of the user's
  (any `.cube` with its own input, e.g. a spektrafilm film); the core
  writes them (`/api/grlog/lut`), inverting the curve the camera applies.
  The preview shows the flat image as the camera records it.

## Export

Builds the update file (`FWDC239B.BIN` or `FWDC243B.BIN`) with recomputed checksums
([docs/firmware-packing.md](firmware-packing.md)), verifies it the way the
camera's decoder does and offers it for **download** together with a preset
(`*.looks.json`). The update file keeps the name of Ricoh's file in
capitals (`FWDC239B.BIN` for the GR III, `FWDC243B.BIN` for the GR IIIx), as
the camera expects it in the root directory of the SD card. If you keep editing
afterwards, the download disappears until you export again. Before flashing:
sort out the recovery path.

**Test without flashing (experimental):** the same export as a RAM patch,
`LOOK.PAT` (`core/src/rampatch.rs`), for a loader that writes it into the
camera's memory from the SD card at power-on. Give it what the camera runs:
the update file flashed last, or `RTOS.BIN` of a RAM dump; without a file it
assumes the official firmware. The loader is not built yet
([plan-ram-loader.md](plan-ram-loader.md)).

**Loading a preset:** the `*.looks.json` written on export can be read back
via "Load preset" in the title bar or by drag & drop onto the preview; "Load
example preset" loads the bundled `editor/static/presets/example.looks.json`.
A preset replaces all current changes: sliders, tone curve, contrast
families, channel curve points (also of other image controls, `look_curves`),
colour wheels, look takeover, HDR effects off (`effects_off`) and grain
(`grain`). The base
matrix is only stored if it was entered by hand; otherwise it comes from the
firmware on load. From there you can keep working and export again.

Standard and HdrCuisine share a matrix: a change applies to both, unless
HDR's effects are off, which gives HDR its own copy. Cross Process always has
its own copy.
