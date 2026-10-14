# Plan: target-first editor

Status: Phases 0, 1 and 2 done (2026-10-14), Phase 3 next.

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
- (Phase 2 review) HDR Tone as a target always has its effects off; there
  is no switch for them. Retro, Bleach Bypass (all three variants) and
  Cross Process are not offered as a source for another target: their
  colours do not make a good base. As targets they keep their own colours
  as the starting point.

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
  - *HDR effects off*: only for HDR Tone (dropped in Phase 2: always off).
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

One object per target, the export request as the core accepted it
(Phase 1):

```json
{ "version": 2, "firmware": "fwdc239b/unpackedFW",
  "targets": {
    "Vivid": { "source": "NegativeFilm", "effects_off": false,
               "matrices": { "Vivid": [9] }, "hsl": {…}, "curves": [R, G, B],
               "edit": {…} },
    "CrossProcess": { "source": "Retro", "channels": { "3": [256], … } },
    "HdrCuisine": { "source": "Standard", "effects_off": true } } }
```

`edit` is the editor's own state (sliders, curve points, wheels) and is kept
as it is. v1 presets (`matrices`, `matrix_edits`, `inherit`, `look_curves`,
`hsl`, `effects_off`, …) are converted on load in the editor. A v1 `tone`
curve is ignored with a note, since the editor no longer writes one.

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

1. **Bleach Bypass as a target** built from another source is one look:
   tables 18, 22 and 23 all point at its one private set. The camera's
   Bleach Bypass variant setting then only picks a toning entry of that
   set. (Phase 1: with Bleach Bypass as its own source the variants keep
   their sets.)
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

**Result (2026-10-14): done.** `core/src/targets.rs` holds the model;
`looks::private_set_at` replaces `private_set`/`private_cross`.

- *Targets:* the eight colour image controls; sources are every colour look
  and table variant except HDR's monochrome table 24. A target that is its
  own source keeps its stock structure (Bleach Bypass its variants, Retro
  its toning) and is edited in place where its matrix is its own. A private
  set is written for Cross Process (always, fixed), HDR with effects off
  (fixed), Standard (it shares its set with HDR in stock) and every target
  built from another source. Sources are read from the stock image, so
  targets never chain.
- *Toning*, refined from Phase 0 after reading the tables: only Bleach
  Bypass (index = SI+0x786) and Retro (index = its toning SI+0x787, 0..8,
  4 neutral) have tables that are not zero; every other colour look reads
  index 0. A target that reads index 0 gets the pointers moved by 2 × the
  source's index. Bleach Bypass and Retro as targets move their index with
  their own menu, so their set points at two 10-entry tables (40 bytes)
  that repeat the source's entry, or at a zero table when the source has
  none. Side effect: Cross Process built from Retro used to read Retro's
  index 0, toning −4; it now reads Retro's neutral toning.
- *Space:* the small items (sets 0x68, toning tables 0x28) go into the tail
  of the reserved block, then below the end of what the HSL sets leave.
  Phase 0's count was off: an HSL set is 0x920 bytes, so even with all five
  HSL sets five private sets fit (tail 3, the two gaps after the slots one
  each). The meter (`/api/space`, export result) counts them.
- *API:* `POST /api/target {target, source, effects_off, remove}`;
  `/api/firmware` lists `targets`, `target_names`, `sources`. The export
  takes `{"targets": {...}}` (format above); without it, the earlier
  editor's request still works on the session's targets, so the current
  editor keeps working until Phase 2. The core does not convert v1 presets;
  that stays with the editor.
- *Tone curve:* not written any more; a v1 request with one gets
  `tone_ignored: true` in the result.
- *Tests:* `tests/targets.rs` (an export without targets is Ricoh's file
  byte for byte; seven targets at once with every source rule, each with a
  set of its own, the source untouched, toning words, round trip through
  the preset; own source keeps the structure; refusals; pool with 5 HSL
  sets). The reference exports (`tests/session.rs`) stay byte-identical
  except Cross Process from Retro, which differs only in the four toning
  pointers (+8), and the two with a tone curve. `TARGETS_DUMP=1 cargo test
  --test targets every_target` writes that image to `work/targets-test/`;
  `tools/verify-targets.py --patched` runs it in Unicorn: all 56 checks pass
  (non-targets stock; each target equals its source with the target's
  matrix, Retro at toning −4/0/+4; Cross Process outside its gamma LUTs,
  which its curve path always changes). The current editor still loads,
  edits and exports in the browser with the new core.

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

**Result (2026-10-14): done.** The editor is target-first; the old tabs,
the hero box, the tone-curve panel and the "other image controls" and
"image control" selectors are gone.

- *Files:* `targets.js` (state per target, what is changed, the session
  sync), `app.js` (preview, pickers, the edit column), `presets.js` (export
  dialog, v2 request, v1 → v2 conversion and loading). `hsl.js` keeps its
  HSL per look; it is now keyed by target and does nothing without one.
- *State:* per target `{source, ext, effects_off}` and a matrix edit
  `{base | null, sat, hue, calib}`; `base` null means the source's stock
  matrix (or the extra base's), so a source change moves the base and keeps
  the sliders, curves, wheels and HSL. Hand-typed matrix values go with the
  old base. Curves and wheels stay keyed by channel index and set as before
  (Cross Process: Color Tone 1–3, every other target its own look set).
- *Preview:* the source's stock sat/hue matrices and the target's
  effective matrix; Before / After against the target's stock matrix. With
  no target: the photo's own image control, unedited.
- *Session:* the editor keeps the core's targets equal to the changed ones
  (`POST /api/target`, add or `remove`), so `/api/hsl` and `/api/space` see
  the same colour sets as the export. The export itself sends every changed
  target in the v2 format.
- *Rules in the UI* (after the user's review): HDR Tone as a target always
  has its effects off, so it always has a matrix of its own and the
  editor has no HDR section; an HDR Tone that is not changed stays stock.
  The source picker leaves out Retro, Bleach Bypass 1–3 and Cross Process
  for every target but the look itself (`NO_SOURCE` in `targets.js`); a
  preset that names one keeps the target's own colours and says so. The
  core still accepts them. Bleach Bypass as its own source applies the sliders
  to all three variants (`matrices` for Var. 2/3 too). Extra bases work for
  every target: the target keeps its own look data, the base sets matrix and
  curves.
- *Presets:* v2 round trip exact (export body before = after loading the
  exported preset, all eight targets). v1: `inherit`, `effects_off`,
  `matrix_edits`/`matrices`, `channel_edits`/`channel_curves(_pre)`,
  `look_curves`, `channel_grade`, `hsl` are mapped to their targets;
  monochrome looks and variants are left out with a note, a tone curve is
  ignored with a note. The example preset comes back as Cross Process from
  Positive Film (GR 1) with the same nine Color Tone curves and matrix as
  the earlier editor exported (difference 0).
- *Browser test* (MR024661, fwdc239b): Standard ← Vivid, Vivid ← Negative
  Film then Retro (curve, HSL, wheel, saturation), Positive Film ← Bleach
  Bypass 3 (calibration), Bleach Bypass ← Standard, Retro ← Retro
  (saturation), HDR ← Standard (effects off), Cross Process ←
  Positive Film (GR 1), Negative Film ← Positive Film; each changes the
  preview from stock. Switching targets and changing Vivid's source kept its
  edits byte for byte. The export of all eight gave `FWDC239B.BIN` (checks
  ok) and a v2 preset; the session then held all eight targets, space
  4.3 / 13.5 KB.

Checklist against `docs/editor.md` (features of the earlier editor):

| Feature | Now |
|---|---|
| Views Edited, Before / After, Compare camera, Camera JPEG | kept; Before is the target's stock look |
| Colour matrix directly or via saturation / hue | Base colour + Advanced → Matrix values |
| Calibration (R/G/B hue and saturation) | Base colour |
| Bleach Bypass and HDR variants with their own matrix | Bleach as its own source: sliders on all three variants; HDR's table 24 is monochrome and not a source |
| HSL per image control, protect shadows/highlights, 5 sets max | HSL curves section, per target |
| Tone curve for all image controls | removed (decision); v1 presets: ignored with a note |
| Cross Process channel curves Color Tone 1–3; set from a Cross Process DNG | RGB curves; variant buttons for Cross Process only; DNG still selects the set |
| Copy a single channel or a whole set | Advanced → Copy curves |
| Channel curves of the other colour image controls | RGB curves of that target |
| Free space meter, export refused when full | pinned meter (all targets), dialog repeats it |
| HDR effects off | always off for HDR Tone as a target, no switch |
| Colour wheels | own section; Cross Process per variant |
| Cross Process with another look's data | Source picker (any target; Retro, Bleach Bypass, Cross Process not offered as sources) |
| Extra bases, removed with untouched curves, recognised in presets | Source picker, "Extra bases"; leaving one removes its untouched curves; v1 detection kept |
| Camera settings drawer, Load preview settings from DNG, white balance | unchanged |
| Export: update file + preset, stale downloads hidden after an edit | Export dialog from the header |
| Load preset from header, drag & drop, example preset | kept; result in a notice under the header |
| Standard shares its matrix with HDR | no longer: Standard as a target always has its own set |

Known limits: Cross Process unedited previews with neutral curves (the
session's Cross Process target is linear, as the export writes it when it
is a target); a Cross Process that is not changed stays stock in the
export. HDR's tone mapping and Retro's vignette are not in the preview, as
before.

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
| Positive Film | Vivid | calibration red hue shift |
| Retro | Retro | own matrix (Retro's vignette must stay) |
| Bleach Bypass | Standard | own matrix; all three variants the same |
| HDR Tone | Standard | own matrix (effects always off) |
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
