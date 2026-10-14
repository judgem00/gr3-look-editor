# Plan: target-first editor

Status: Phase 0 done (2026-10-14), Phase 1 next.

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

Phase 0 settled the variants and the placement (results below): one private
set per target, six new ones (Standard, Vivid, Positive Film, Bleach Bypass,
Retro, Negative Film) next to the existing Cross Process and HDR sets.

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

**Result (2026-10-14): the approach holds, with three rules.**
`tools/verify-targets.py` builds test images in Python (inheritance, a
private copy of the set and matrix at `0x53003000`, a saturation edit of
the copy) and runs Make for every colour look and table variant, still and
live view, saturation 0 and +4: 385 checks pass.

- *Independence:* every look that is not a target stays bit-identical to
  stock, including Negative Film when Vivid takes its data, and HDR and
  Cross Process when Standard gets its own matrix.
- *Exactness:* a target with an unedited copy is bit-identical to its
  source; with an edited copy it is bit-identical to the source in an image
  where the source's own matrix got the same edit.
- *Table variants* (setting filler `0x536dc26c`): Bleach Bypass selects
  table 18/22/23 by SI+0x786 = 1/2/3, three full sets with different
  matrices; HDR selects table 24 (identity matrix, luma saturation) when
  SI+0x788 is 1 or 2, and always 20 with its effects off. All other entries
  of the three look tables are the same for these variants.
- *Toning:* SI+0x786 (Bleach Bypass) and Retro's own value 4 also reach
  setting+0x60, which indexes four tables of the set (words 11–14, read at
  `0x536e73d0` ff., info+0x1674). A target without that setting reads
  index 0, so Positive Film from Bleach Bypass 3 came out 2 bytes off (a
  −40 offset). Moving those four pointers by 2 × the source's index in the
  private copy makes the target's index 0 read the source's entry; with
  that the case is bit-identical.
- *Space:* a set with its matrix takes 0x68 bytes. The tail of the reserved
  block `0x53003ea8–0x53004000` (344 B, zero in stock, after the HDR set)
  holds three; the other three come out of the shared pool (312 B of
  13.2 KB). Cross Process and HDR keep their fixed sets.

Rules for Phase 1 from this:

1. **Bleach Bypass as a target** is one look: tables 18, 22 and 23 all point
   at its one private set. The camera's Bleach Bypass variant setting then
   only picks a toning entry of that set.
2. **A source with toning** (Bleach Bypass 1/2/3, Retro) shifts the private
   set's toning pointers by its index (1/2/3, 4). Bleach Bypass 1, 2 and 3
   stay separate sources.
3. **HDR as a target** gets its own matrix only with its effects off
   (otherwise strengths 1/2 use table 24). Its curves and HSL work either
   way.

Not covered by Make and not carried by a source: Retro's edge darkening and
HDR's tone mapping, which hang on the look index outside the colour data
(`0x535131c0`). A look built from Retro has Retro's colours without the
vignette. To be confirmed on the camera in Phase 4.

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
| Positive Film | Bleach Bypass 3 | calibration red hue shift (toning shift) |
| Retro | Retro | own matrix (Retro's vignette must stay) |
| Bleach Bypass | Standard | own matrix; all three variants the same |
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
