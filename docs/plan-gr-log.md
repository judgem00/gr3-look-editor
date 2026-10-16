# GR-Log: a log profile for video

Goal: a look whose tone curve stores the scene logarithmically, for video
shot in manual exposure, with LUTs that take it back to a finished image in
post (the camera's Standard, Rec.709, or a spektrafilm film).

Status: phases 1–3 done (core, editor, test firmware), see the results
below. On the GR III it works in principle (user-confirmed, 2026-10-16);
the measured camera test (phase 4) comes later. Builds on the per-look curves
([plan-look-curves.md](plan-look-curves.md)), the LUT fitter's `grlog`
encoding ([plan-spektrafilm-fitter.md](plan-spektrafilm-fitter.md)) and the
video patches (`docs/video.md`: manual exposure `manual mlive7 dials`,
optionally `osd` and `bitrate`).

## What is given

- **Every look edit reaches video** (user-confirmed): matrix, per-look RGB
  curves, HSL and the post matrix act on movie frames as on stills.
- **Manual exposure in video works** (`mlive7` + `dials`): exposure stays
  fixed, so the log code of a scene value does not move with the AE.
- **Looks in video:** Standard, Vivid, Positive Film, Bleach Bypass, Retro,
  Cross Process (and the monochrome ones). Negative Film and HDR Tone cannot
  be picked in movie mode, so GR-Log goes on one of the first six.
- **Encoder:** 8-bit YCbCr 4:2:0, H.264, 16.8 (24p) to 42 Mbit/s (60p);
  `bitrate` doubles 24p/30p.
- **Video patches are built into the editor export:** the Export dialog's
  "Video" checkbox applies the tested set (`manual mlive7 dials osd
  bitrate`) directly in the core (`core/src/video.rs`), so no second step
  with `video-test.py` is needed any more. The earlier two-step build
  (`video-test.py build … --image <unpackedFW>`) still works and is what the
  camera-tested firmware was made with.

## The curve

A look's RGB curve `C` sits on Standard's tone curve `b` (`o = C(b >> 6)`,
256 × u16). For a log output the same curve on all three channels is

```
C = E ∘ b⁻¹        E(L) = (log2(L / 0.18) − lo) / span,   clamped to 0..1
```

with `L` scene-linear relative to 18 % grey, as the model places it (grey
at tone index 23.3). `b` is strictly rising up to index 241 (+3.37 EV), so
`b⁻¹` exists over the whole range the sensor delivers.

**Encoding range.** The camera's input reaches from about −8.5 EV (one step
of the 12-bit input) to +3.46 EV over grey, +4.45 with highlight correction
(`MovieHighlightCorrect` exists in the video menu). Below about −6 EV there
is only noise at video ISOs. Proposal: **`grlog −6 10`**, i.e. −6 … +4 EV,
which is also the format the editor already parses (`grlog LO SPAN`) and
`tools/spektrafilm-lut.py` can write film LUTs for.

8-bit codes per stop (full range 0–255):

| Stop over grey | −6…−5 | −4…−3 | −2…−1 | −1…0 | 0…+1 | +1…+2 | +2…+3 | +3…+4 |
|---|---|---|---|---|---|---|---|---|
| Standard (gain 6144) | 3 | 10 | 32 | 43 | 45 | 44 | 37 | 12 |
| `grlog −6 10` | 25 | 25 | 25 | 25 | 25 | 25 | 25 | 25 (to +3.46 or with HC) |
| `grlog −8 12` (current LUTs) | 21 | 21 | 21 | 21 | 21 | 21 | 21 | 21 |

Log halves the codes in the midtones and multiplies them in shadows and
highlights. With 16–235 output (to be measured) it is about 22 per stop.
That is enough for a moderate grade; strong pushes will band, and noise does
part of the dithering. A "cine" variant (soft shoulder, slightly lifted
shadows, mids nearly as Standard) is the fallback if the camera test
shows banding.

Grey lands at u = 0.6 (`−6 10`): with full-range 8 bit about code 153.

## Colour

- **Matrix:** Standard's look matrix, no HSL, no post matrix. GR-Log then
  keeps Standard's colours; the post LUTs are exact inverses of a known,
  simple chain.
- **Saturation slider:** 0. The camera's YCbCr stage after the curve
  (hue rotation, CSP) is part of the model and inverted in the LUT.
- **Sharpness −4, shadow and highlight correction off** in the movie menu
  (documented on the target, not set by the firmware).

## Post LUTs

All computed by the core from the same model the editor previews with, so
no fit is needed. The chain after the curve is invertible: per-channel
curve `C` (one curve for R, G, B), then the YCbCr stage (a 3×3 on
gamma-encoded RGB).

| LUT | From | To | How |
|---|---|---|---|
| `GR-Log → Standard` | GR-Log | the camera's Standard (sRGB/Rec.709 display) | invert YCbCr, `E⁻¹`, Standard's `b`, YCbCr |
| `GR-Log → Rec.709` | GR-Log | Rec.709 with a plain display curve | invert, `E⁻¹`, Rec.709 OETF with a soft shoulder |
| `GR-Log → film` | GR-Log | a spektrafilm film and print | `tools/spektrafilm-lut.py` with the same `grlog −6 10` input, applied after undoing the YCbCr stage and the matrix |

Size 33³ or 65³, `.cube`, with the input range in the header. The film LUTs
reuse the library's 132 looks; they are made offline, not in the browser.

## Editor

- **Source "GR-Log (video)"** in the source picker for the six video looks:
  sets matrix (Standard), the three curves `E ∘ b⁻¹`, no HSL, no P; grain
  stays. Recorded in the target (`edit.log: {lo, span}`) and the preset.
  Editing on top is allowed but the note says that the post LUTs then no
  longer invert it exactly.
- **Download LUTs** for a GR-Log target: Standard and Rec.709 generated in
  the browser; the film LUTs as a separate download (zip) from the repo.
- The preview shows the flat log image as the camera records it, with a
  toggle "through LUT" to see the result.
- Help text: manual exposure, movie menu settings, ISO, bitrate.

## Questions for the camera test

1. **Is the tone gain fixed in M?** The runtime gain (`setting+100`) comes
   from the AE and varies per scene in stills (2490–9033 at ISO 100). If the
   live-view AE keeps adjusting it at fixed exposure, the log code of grey
   moves from scene to scene. Test: one clip in M with fixed exposure, the
   chart, then a bright and a dark scene, then the chart again; grey patch
   code must stay put.
2. **Range:** does the camera write full range or 16–235, and how does the
   MOV tag it (`ffprobe color_range`)?
3. **Banding and blocking** at 30p with and without `bitrate`, on the
   smooth ramp of the chart, after `GR-Log → Standard` and a +1 EV push.
4. **Does the inverse hold?** A clip through `GR-Log → Standard` against a
   clip shot in plain Standard at the same exposure: ΔE2000 on the chart.

## Phases

1. Core: `E`, `b⁻¹` and the three curves for a target (`log::curves(lo,
   span)`), the inverse chain, `.cube` writers; tests (round trip GR-Log →
   Standard on the model to < 0.5 ΔE, monotone, range).
2. Editor: source "GR-Log (video)", preset field, LUT download, help.
3. Test firmware: editor export with GR-Log on Bleach Bypass (the least
   used of the video looks; Positive Film keeps the film test) plus
   `manual mlive7 dials osd bitrate` via `--image`. One flash.
4. Camera test with the questions above (screen chart + one scene outside),
   frames with ffmpeg, evaluation like `eval-film.py`.
5. Film LUTs for `grlog −6 10` from the library (offline), docs.

## Risks

- 8 bit: banding in skies after strong grades. Mitigation: the cine
  fallback curve, the bitrate patch, grain in post.
- The tone gain moving in M would make the encoding scene-dependent. Then
  the plan needs a fixed gain in movie mode (a firmware question of its
  own, see the "runtime gain" open point in gr3x-fw-hack tone-curves.md).
- The YCbCr stage on log data: chroma is low in the flat image, so the
  inverse must be exact or colours drift in post; the round-trip test in
  phase 1 checks it.

## Phase 1 result (2026-10-16)

`core/src/grlog.rs`, tests in `core/tests/grlog.rs`, LUTs by
`core/examples/grlog_luts.rs` (`OUT_DIR [--hc] [--size N] [--films DIR]`).

- `grlog::curve` builds C = E ∘ b⁻¹ on the fitter's camera model
  (`grlog::camera(img, hc)`: Standard's tone curve at gain 6144, or 3072
  with highlight correction). `grlog::look` / `grlog::spec` make the target
  (source Standard, Standard's matrix, the curve on R, G and B, no HSL, no
  P; `edit.log: {lo, span, hc}` instead of `edit.lut`). Targets: Standard,
  Vivid, Positive Film, Bleach Bypass, Retro; all export.
- **Where the camera can store a log.** It interpolates its tone table
  linearly between 256 steps of the 12-bit input. Below the first step it
  can only draw a straight toe, and in the next stop and a half the steps
  are far apart, so it draws chords of the log:

  | | straight toe below | chords (≤ 2.2 codes off) | log within 0.5 codes | input ends |
  |---|---|---|---|---|
  | HC off | −4.5 EV | −4.5 … −3 EV | −3 … +3.46 EV | +3.46 EV |
  | HC on | −3.5 EV | −3.5 … −2 EV | −2 … +4.46 EV | +4.46 EV |

  So the camera holds about 8 stops as log, with highlight correction
  shifted up by one. With `grlog −6 10` the toe takes codes 0 … 37 (HC off)
  and the top 5 % stay unused without highlight correction.
- **Post LUTs invert the curve the camera applies** (`grlog::decoder`:
  tone and C composed at the 256 steps, linear between), not the ideal E,
  so the toe and the chords come back exactly and code 0 is black.
  On the model, 460 scene colours from −5 to +3.2 EV:

  | LUT (33³) | ΔE2000 against | mean | p95 |
  |---|---|---|---|
  | GR-Log → Standard | the camera's Standard | 0.03 | 0.09 |
  | GR-Log → Portra 400 / Portra Endura | the film LUT on the scene | 0.09 | – |

  GR-Log → Rec.709: grey on the BT.709 curve, rising, white at the input
  end (soft shoulder from linear 0.6).
- The example writes GR-Log → Standard, → Rec.709 and one LUT per film of
  the library (132, 33³, about 1 MB each).

For phase 2: the encoding could follow the camera's range (e.g. `grlog
−4.5 8` without highlight correction, `−3.5 8` with it: about 32 codes per
stop instead of 25). It is a parameter of every function; the default
stays `−6 10` until the camera test shows how the toe and the noise look.

## Phase 2 result (2026-10-16)

- Session calls `grlog {target, hc, lo, span}` (the target and the EV range
  it stores) and `grlog_lut {to, hc, lo, span, size}` (+ the .cube for
  `to: "lut"`), in wasm, worker and bridge.
- The target keeps `edit.lut` with `library: "grlog"` (not a separate
  `edit.log`): the editor already treats such a target as fitted, so resets,
  "another source drops it" and the preset work unchanged.
- Editor: source group "Video" → "GR-Log"; the highlight correction box
  ("GR-Log made for: …") rebuilds the curve; a box "Post LUTs for GR-Log"
  with the stored range and three downloads (to Standard, to Rec.709,
  through a LUT of one's own); a note with the movie menu settings; help
  text; "GR-Log (video)" in the export list. EN and DE.
- Checked in the browser: Bleach Bypass and Retro take it; HC on rebuilds
  the curve (range −3.5 … +4.5 EV) and keeps the grain; the downloads come
  out as `GR-Log_to_Standard.cube`, `GR-Log-HC_to_<lut>.cube`; switching
  back to a look drops it. `cargo test` green (GR-Log: 8 tests).
- Not done: a preview "through the LUT" toggle (the preview shows the
  flat log image); the 132 film LUTs as a ready download (they need the
  library's .cube files, which are not shipped; `grlog_luts --films` makes
  them offline, or "through a LUT" in the editor one by one).

## Phase 3: test firmware (2026-10-16)

`core/examples/grlog_flash.rs` exports GR-Log (`grlog −6 10`, highlight
correction off) on Bleach Bypass, every other look stock, to
`work/grlog-test/` with the post LUTs. `tools/video-test.py build manual
mlive7 dials osd bitrate --image work/grlog-test/unpackedFW-looks` adds the
video patches
(`work/grlog-test/video/FWDC239B.BIN`). Checks: container (`fw-pack.py
check`), the look and the video image differ only in the 102 bytes of the
patches, `verify-tuning.py --patched` (the selector stub with the video
patches, still and live, every look), `verify-targets.py --patched` (every
other look stock). `osd` is flashed here for the first time.

On the camera (2026-10-16): GR-Log and the video patches work in
principle (the user's check, no measurements yet).
Shooting guide: `work/grlog-test/HOW_TO_SHOOT.txt` (German).
