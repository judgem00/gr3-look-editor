# Plan: target-first editor

Status: planned (2026-10-14), nothing implemented.

## Goal

The editor follows one fixed order:

1. **Target**: the image control of the camera that gets the new look
   (Standard, Vivid, Positive Film, Negative Film, Bleach Bypass, Retro,
   HDR Tone, Cross Process).
2. **Source**: the look it starts from (any of the eight, or an extra base
   such as Positive Film (GR 1)). Source = target means "start from the
   target's own stock look".
3. **Edit**: every edit for that target in one vertically scrolling column.

Several targets can be edited in one session and go out in one export.
Each target is fully independent: editing one never changes another.

Not part of this plan: monochrome image controls as targets (later), new
menu entries, a tone curve for all image controls (dropped, see below).

## Why

Today the target is spread over four places: the hero box says "your look
goes into Cross Process", the Colour tab hides another target under
"advanced", the Curves tab has its own image-control selector, and the HDR
switch sits inside the Curves tab. Tabs 1–4 split what belongs together, the
tone curve for all looks sits among per-look edits, and long notes stay on
screen between the controls.

## Decisions (user, 2026-10-14)

- Several targets per export, with an overview of all eight.
- Changing the source after editing keeps curves, HSL and wheels; only the
  base underneath changes.
- The tone curve for all image controls is removed from the editor.
- Every target gets a colour matrix of its own. Without it, a target that
  takes its data from a source shares the source's matrix (see below).
- Monochrome image controls are not offered as targets for now.

## Background: why every target needs its own matrix

Taking a source (`looks::inherit`) points the target's entries in the three
per-look tables (`0x55018d94`, `0x55018cf0`, `0x55018e4c`) at the source's.
The first one is the ColorMatrix set, so target and source then share one
matrix: Vivid built from Negative Film and given more saturation would make
Negative Film more saturated too. Standard and HDR share a set already in
stock firmware.

Private sets exist today only for Cross Process (`0x53002080`) and HDR with
effects off (`0x53003e40`). `looks::private_set` copies the current set (21
words) and its matrix (18 bytes) and redirects the look's entry; the copy is
taken *after* the inheritance, so it starts from the source's matrix.

Needs: one set + matrix (0x54 + 18 → 0x68 bytes, rounded) per table:
Standard 11, Vivid 12, Positive Film 17, Bleach Bypass 18/22/23, Retro 19,
HDR 20/24, Cross Process 21, Negative Film 25 → at most 11 sets, about
1.2 KB, out of the shared pool (`space.rs`). A set is only written for a
target whose matrix differs from what it would share.

Open for Phase 0: whether the Bleach Bypass variants 22/23 and HDR variant 24
need their own private sets or can point at the target's one set (they carry
their own parameter sets; check in Unicorn what the variant tables feed).

## Design

### Layout

```
┌ Header: camera · photo · preset · [Export (3 looks changed)] ┐
├ Preview (left) ──────────────┬ Side panel (right) ───────────┤
│ Edited | Before/After |      │ ① TARGET  [Positive Film  ▾]  │  pinned
│ Compare camera | Camera JPEG │ ② SOURCE  [Negative Film  ▾]  │  pinned
│                              │ ───────────────────────────── │
│                              │ ▸ Base colour               ● │
│                              │ ▸ RGB curves                  │
│                              │ ▸ Colour wheels               │
│                              │ ▸ HSL curves                ● │
│                              │ ▸ HDR effects off (HDR only)  │
│                              │ ▸ Advanced                    │
│ ▸ Camera settings of photo   │ Space ▓▓▓▓▓░░ 9.1 / 13.0 KB   │  pinned
└──────────────────────────────┴───────────────────────────────┘
```

- **Target picker**: the eight colour image controls, each with its state
  ("stock", "from Negative Film", "edited" dot). Until a target is chosen,
  the edit column is empty and says so in one line. Changing the target
  keeps every other target's edits.
- **Source picker**: enabled once a target is chosen. Lists the eight looks
  and the extra bases (`BUILTIN_BASES`, `_local/bases.json`).
- **Edit column**: collapsible sections, in this order; each has a changed
  dot and its own reset. Sections a target cannot use are not shown.
  - *Base colour*: saturation, hue, calibration (R/G/B hue and saturation).
  - *RGB curves*: linked or per channel. Cross Process adds Color Tone 1–3
    (and the copy-between-variants tools).
  - *Colour wheels*: shadows, midtones, highlights, global, balance; they
    write the target's RGB curves as today.
  - *HSL curves*: Hue vs Hue / Sat / Lum, protect shadows and highlights.
  - *HDR effects off*: only for HDR Tone.
  - *Advanced*: matrix values, copy curves.
- **Space meter** pinned at the bottom of the side panel, for all targets
  together.
- **Notes**: help text moves into the "How it works" dialog and short
  tooltips; the side panel keeps at most one line per section.

### Preview

Always renders the selected target as the camera would: the source's look
data (tone family, effects), the target's own matrix, curves and HSL.
Before / After compares the target's stock look with the edit. With no
target selected the preview shows the photo's own image control, unedited.

### Export

A dialog from the header: list of changed targets with source and what was
changed, the space meter, the risk checkbox, "Create update file", then the
downloads (update file + preset). Also reachable when nothing is changed
(disabled button with the reason).

### Preset format (v2)

One object per target:

```json
{ "version": 2,
  "targets": {
    "Vivid": { "source": "NegativeFilm", "matrix_edits": {…}, "curves": {…},
               "wheels": {…}, "hsl": {…} },
    "HdrCuisine": { "source": "HdrCuisine", "effects_off": true, "curves": {…} } } }
```

v1 presets (`matrices`, `matrix_edits`, `inherit`, `look_curves`, `hsl`,
`effects_off`, …) are converted on load. A v1 `tone` curve is ignored with a
note, since the editor no longer writes one.

## Phases

### Phase 0: matrix per target, open questions

- Find out what the variant tables 22/23 (Bleach Bypass) and 24 (HDR) feed
  and whether they can share the target's private set.
- Unicorn run (`tools/verify-tuning.py`, `tools/emu.py`) of a test image
  where Vivid takes Negative Film's data and gets its own edited matrix:
  Vivid shows the edit, Negative Film stays stock, still and live view.
- Check that private sets for all eleven tables fit next to 5 HSL sets and
  8 curve sets, or what the worst case leaves.

### Phase 1: core

- `looks::private_set` for every colour table, placed by `space.rs` out of
  the pool (the fixed Cross Process and HDR addresses stay where they are,
  so older exports keep their layout).
- Session: per-target state `{source, matrix, effects_off}`; `inherit`
  becomes the source; a private set whenever the target's matrix is edited
  or it takes another source; the Standard/HDR sharing goes away for edited
  targets.
- Export takes the v2 structure; v1 conversion lives in the editor.
- Remove the tone curve for all image controls from the export path (base
  curves stay stock).
- Tests: every target independent (editing one leaves every other table's
  set, matrix and curves byte-identical), v1 → v2 round trip, space with
  everything used.

### Phase 2: editor

- New layout (target and source pinned, one scroll column, export dialog,
  pinned space meter), tabs removed.
- State per target in `app.js`; preview reads the selected target. Split
  `app.js` along the sections (target state, sections, export) where it
  makes the new layout easier.
- Remove the tone-curve panel and the "other image controls" and
  "image control" selectors.
- i18n EN/DE for every new string; hero box and long notes into the help
  dialog.
- Browser test with the bundled DNGs: each target × a non-trivial source,
  switching targets keeps edits, source change keeps curves/HSL/wheels,
  v1 example preset loads.

### Phase 3: docs

`editor.md` (new workflow), `look-mapping.md` (private set per target),
`flash-safety.md` (what the new sets change), README screenshot/texts.

### Phase 4: test flash

One file, flashing is the user's step ([flash protocol]: few flashes,
Standard as the control):

| Target | Source | Edit |
|---|---|---|
| Standard | — | unchanged, control |
| Vivid | Negative Film | own matrix: more saturation + marker curve |
| Negative Film | — | stock (must stay identical to stock despite Vivid) |
| Positive Film | Standard | calibration red hue shift |
| HDR Tone | Standard | effects off, own matrix |
| Cross Process | Positive Film (GR 1) | as today |

Shots of the grey ramp and the MultiAxial target per target, compared with
the preview; then live view while switching, power cycle, return to stock.

## Risks

- More private sets means more redirected entries in `0x55018d94`. Same
  mechanism as the camera-tested Cross Process and HDR sets; Phase 0 checks
  the variant tables before anything else.
- A redesign of the whole side panel can lose features that exist today.
  Checklist from `docs/editor.md` before Phase 2 is closed.
- v1 presets with a tone curve no longer reproduce exactly; the editor says
  so on load.

[flash protocol]: plan-look-curves.md#phase-4-one-test-flash
