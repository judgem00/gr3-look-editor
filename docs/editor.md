# Look Editor

Edits the colour matrices and tone curves of the GR III image controls with a
live preview on a GR III DNG, and writes a flashable update file.

Runs entirely in the browser: drop in Ricoh's update file `fwdc239b.bin`,
then a DNG. The Rust core (`core/`, as WebAssembly) reads the firmware and the
DNG, including the shot's image control and settings from the makernote.
Nothing is uploaded.

## What the preview computes

```
camera RGB -> WB (AsShotNeutral) -> sensor CCM -> look matrix -> A[saturation] -> (mono mix)
           -> camera tone curve (per channel for Cross Process) -> hue rotation (YCbCr)
           -> (residual correction)
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
- Input scaling and gain are fitted per DNG against the embedded camera JPEG;
  the **residual correction** (can be switched off, "Fine-match to camera
  JPEG") absorbs the camera's image-dependent shadow correction.

Views: **Edited**, **Before / After** (firmware original against the edit,
colour and tone curve), **Compare camera**, **Camera JPEG**.

## Editing

Layout of the interface: the editor builds a look of your own into the slot
of **Cross Process**; the other image controls stay unchanged. Tabs:
**1 Base** (the colours of an existing image control as a starting point,
copied into a separate matrix for Cross Process), **2 Colour**, **3 Curves**
(Color Tone 1–3, RGB curves linked or per channel, colour wheels; the curves
always start linear), **4 Export**. Other image controls and the tone curve
for all image controls are under "advanced". Technically Cross Process gets
its own ColorMatrix set and its own matrix in the free area `0x53002080…`
(`core/src/looks.rs`, `private_cross`; the set is 21 words long).

- **Colour matrix** per image control, directly or via saturation/hue.
- **Calibration** as in Lightroom: hue and saturation of the primaries red,
  green and blue. This can be expressed exactly as a matrix (`Look =
  Saturation · Hue · Base · Calibration`): the primaries are rotated around
  the grey axis or scaled, then each column is scaled so that white stays
  white. The other primaries keep their hue (< 1° crosstalk).
  Presets store the sliders (`matrix_edits`).
  Bleach Bypass and HDR have variants with their own matrix.
- **Tone curve:** click to add points, drag them, double-click to remove.
  The curve is applied to the 15 base curves (optionally only to individual
  contrast families); the camera then blends as usual. It **applies to all
  image controls**. The lower graph shows the camera curve that actually
  takes effect.
- **Channel curves (R, G, B)** of the Cross Process variants **Color Tone
  1–3** (the Color Tone setting in the camera menu; internally preset 1–3)
  and Random (not reachable from the menu). When a Cross Process DNG is
  loaded, the editor selects the shot's set. Edited like the tone curve; a
  click copies a single channel or a whole set from another variant. They only
  take effect with Cross Process — the preview applies them when that image
  control is selected.
- **Colour wheels** (shadows, midtones, highlights, global, with luminance
  and balance) as in Lightroom Color Grading. They are converted into the
  R/G/B channel curves of the selected Cross Process set and therefore act
  like RGB curves, not in a perceptual colour space. With "Random" the camera
  redistributes the curves per shot, so a tint changes colour. The preset
  stores the wheels; they stay editable after loading.
- **Cross Process with another look's data:** for Cross Process, "take look
  data from …" (e.g. Negative Film) plus "RGB linear" gives a look that
  computes like Negative Film and carries RGB curves
  ([docs/tone-curves.md](tone-curves.md)).
- **Camera settings** (key, contrast, highlights, shadows, exposure) affect
  the preview only; pre-filled from the DNG.

## Export

Builds the update file `FWDC239B.BIN` with recomputed checksums
([docs/firmware-packing.md](firmware-packing.md)), verifies it the way the
camera's decoder does and offers it for **download** together with a preset
(`*.looks.json`). The update file is named `FWDC239B.BIN`, as the camera
expects it in the root directory of the SD card. If you keep editing
afterwards, the download disappears until you export again. Before flashing:
sort out the recovery path.

**Loading a preset:** the `*.looks.json` written on export can be read back
via "Load preset" in the title bar or by drag & drop onto the preview; "Load
example preset" loads the bundled `editor/static/presets/example.looks.json`.
A preset replaces all current changes: sliders, tone curve, contrast
families, channel curve points, colour wheels and look takeover. The base
matrix is only stored if it was entered by hand; otherwise it comes from the
firmware on load. From there you can keep working and export again.

Standard and HdrCuisine share a matrix: a change applies to both. Cross
Process has its own copy.
