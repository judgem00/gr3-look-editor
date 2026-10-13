# Look Editor

Edits the colour matrices and tone curves of the GR III image controls with a
live preview on a GR III DNG, and writes a flashable update file.

Runs entirely in the browser: drop in Ricoh's update file `fwdc239b.bin`,
then a DNG. The Rust core (`core/`, as WebAssembly) reads the firmware and the
DNG, including the shot's image control and settings from the makernote.
Nothing is uploaded.

## What the preview computes

```
camera RGB -> WB (AsShotNeutral or colour temperature) -> sensor CCM -> look matrix -> A[saturation] -> (mono mix)
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
copied into a separate matrix for Cross Process, or an extra base, see
below), **2 Colour**, **3 Curves**
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
- **Extra bases:** besides the image controls of the camera, the base list
  offers **Positive Film (GR 1)** (rich red, muted green and yellow, deep
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
  makernote (for a Cross Process shot also its Color Tone). The fine-match
  to the camera JPEG is fitted with the shot's settings, so "Compare camera"
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

**Loading a Lightroom preset:** `.xmp` (Lightroom, Camera Raw) and
`.lrtemplate` (Lightroom Classic) go the same ways. As in Lightroom, a preset
sets only what it contains and does not replace everything: point curves (RGB
and R/G/B) and colour wheels (split toning, Color Grading) go into the
selected variant, calibration and saturation into the image control being
edited. The RGB curve alone becomes the shared points of the three channels;
together with channel curves it is baked into the base (Lightroom applies the
RGB curve first, then the channel curves) and the channel points stay
editable. HSL, the Basic panel (exposure, contrast, highlights/shadows,
whites/blacks), the parametric curve, grain, masks and profiles with colour
tables (LUT) have no counterpart in the camera; the note lists them as
skipped. Parser and mapping: `editor/static/lightroom.js`; check with
`node editor/lightroom-check.js [preset files …]`.
