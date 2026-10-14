# Plan: RGB curves and HSL for every colour look

Status: Phase 0 done (2026-10-14), Phase 1 next. Firmware `fwdc239b` (GR III
2.10); the GR IIIx 1.60 image uses the same addresses.

## Goal

Each of the eight colour image controls (Standard, Vivid, Positive Film,
Negative Film, Retro, Bleach Bypass, HDR, Cross Process) can have **its own
R/G/B channel curves** and **its own HSL set** (MultiAxial). Both share one
pool of free space (about 18.9 KB); the editor shows how much is left. Looks
without an assignment stay byte-identical to stock.

Not part of this plan: monochrome looks, compressed curves (knots), new menu
entries, grain for the Cross Process carrier.

## Background

Today only Cross Process applies channel curves
([tone-curves.md](tone-curves.md)). The chain:

1. **Variant** (`0x536dd444–0x536dd50c`, end of the context fill
   `0x536dd270`): `cmp r1,#21` (r1 = look index from SI+0x77C, Negative Film
   = 22). Not Cross Process → SI+0x38A98 = 0. Cross Process → SI+0x38A98 =
   2/3/4 for Color Tone 1/2/3 (SI+0x797). Registers there: r6 = info+0x8000,
   r7 = shooting info.
2. **Flag** (`0x536db160–0x536db170`): setting+0x9B = (SI+0x38A98 ≠ 0); the
   selection `0x536ef140` then sets the three curve indices (variant 4 =
   library slots 9, 10, 11).
3. **Compose** (`0x536e8694`): with the flag set, `0x536eedf0` reads the three
   curve pointers from the library table `.data 0x55037604[0..11]` and builds
   each channel through its curve.

Apart from the curves, Cross Process has no effect of its own: the
shading/clarity/grain gate `0x535131c0` treats tone 21 like Standard, and
Cross Process shots match the model like Standard does (see Credit).

The library table lives in `.data`, i.e. in RAM, and is read at compose time.
So the curves of any look can be reached by writing three pointers into slots
9–11 and choosing variant 4, without patching `GammaTuning`.

## Free space

Scan of all literal words and `movw`/`movt` pairs in the RTOS image
(~115,000 references) against zero runs:

| Area | Size | Notes |
|---|---|---|
| Padding `0x53000d50–0x53002000` | 4784 B | used for HSL today; camera-tested |
| Padding `0x53002100–0x53004000` | 7936 B | as above; `0x53002080–0x530020e6` holds the private Cross Process set |
| Library curves 0–11 (12 × 512 B, three runs at `0x53efb0e4…0x53efc928`) | 6144 B | each referenced only from `0x55037604`; free once every look goes through the new table |

The two references found inside the padding are instruction words that look
like addresses (`0x531f94a0`, `0x53a59398`), not pointers. Other zero runs
without references lie inside large data blocks (0x800-stride structures at
`0x53f0…–0x53f4…`, tables at `0x53d3…`); they are not used.

Needs: HSL set 0x950 B (2384 B, contiguous), RGB set 3 × 512 B (each channel
has its own pointer, so curves can be split). Rough capacity:

| RGB | HSL |
|---|---|
| all 8 | 2 |
| 4 | 4 |
| 0 | 5 (today) |

## Design

### Selector stub

`0x536dd450` becomes an unconditional branch to a stub (~25 words) in the
reserved part of the padding (the 1 MB section is mapped executable). The stub
runs in the frame of the context fill and returns with its epilogue
(`ldm sp, {r3, r4, r5, r6, r7, r8, sb, sl, fp, sp, pc}`):

```
entry = look 11..22 -> index look-11; Cross Process: Color Tone 0/1/2 -> own entries
table[entry] has curves:  write its 3 pointers to 0x55037604[9..11]
                          SI+0x38A98 = 4
otherwise:                SI+0x38A98 = 0      (stock behaviour for every other look)
```

Cross Process becomes three ordinary entries. Variants 1 (random) and 5–8
(saved sets) are never set again. The table (3 pointers per entry, 14 entries)
sits next to the stub; the editor writes it on export, the code is fixed.

### HDR without effects (optional, per export)

Only when HDR gets its own data:

| Effect | Site | Patch |
|---|---|---|
| local tone mapping | `0x535131fc` `beq 0x53513274` | target `0x53513218` (as Standard) |
| HDR strength to the pipeline | `0x536dbeb4` `ldrb sb,[r3,#0x788]` | `mov sb,#0` |
| monochrome variant (slot 24) | `0x536dc27c` `beq 0x536dc818` | `nop` (always slot 20) |

HDR keeps key, contrast, highlight and shadow fixed at neutral
(`0x536dbec0` ff.); saturation and hue still work. Retro (vignette ctx+0x1FC,
toning) and Bleach Bypass (toning, variants) can follow the same pattern
later.

## Phases

### Phase 0: verification tool and open questions

`tools/verify-tuning.py` (Unicorn, already in `.venv`): runs
`ImageTuningStill::Make` (`0x536dd8f0`) and the live view Make (`0x536ddecc`)
on an image and compares the R2Y block. Based on the harness of
gr3-film-mods (`emu.py`, `run_tuning.py`, MIT, with credit).

Checks: stock vs patched for looks 11–22, Bleach/HDR variants, Cross Process
Color Tone 1–3, still and live view, and sequences (a look with curves, then
a stock look). Looks without an assignment must be byte-identical; assigned
looks may differ only in the gamma tables, which must equal
`chan::compose(base, curves)`.

Questions to settle before any editor code (stop and report if one fails):

1. Order: does the context fill run before `0x536db130` and `GammaTuning`
   in the same Make call?
2. Live view caching: is the gamma recomputed when switching between two
   looks with their own curves (A → B → A)?
3. Does `0x536ef140` with variant 4 and a non-Cross look change anything
   besides the curve indices (no writes to the shooting info)?
4. HDR patches: does HDR then differ from Standard only where its own data
   acts?

**Result (2026-10-14): all four answered, nothing blocks Phase 1.**
`tools/verify-tuning.py` (with `tools/emu.py`) models the selector as a
Python hook at `0x536dd450` and runs 87 checks in ~20 s:

1. Yes. Per Make, still and live: context fill > variant > flag > selection
   `0x536ef140` > channel fill `0x536e8660` > curves `0x536eedf0`.
   `GammaTuning` itself is cached and runs only when its inputs change; the
   channel fill runs on every Make.
2. Yes. A > B > A, curves > Cross Process > curves, all looks back and forth
   and a slider change between looks each equal a fresh run, still and live.
   The flag routine raises `info+0x82af` when the filter goes from on to off
   (stock does the same after Cross Process); it follows the filter state, so
   it is excluded from the byte comparison and checked separately. Its
   `[r0]` byte (re-shuffle for the Random variant) does not matter for
   variant 4.
3. No. With Standard and variant 4 the selection writes exactly the targets
   it writes for stock Cross Process Color Tone 3 (`.data 0x55001a60/64` and
   the coordinator at `0x5536daba…db6c`); nothing in the shooting info.
4. Yes. Patched HDR equals Standard bit for bit (all three strengths, still
   and live) and keeps the tone sliders at neutral; no other look changes.
   The local tone mapping site `0x535131fc` lies outside Make and is not
   covered by the emulation.

Further: without assignments the modelled selector is byte-identical to stock
for every colour look and variant (Bleach Bypass 1-3, HDR 0-2, Cross Process
Color Tone 1-3); with every look assigned, only the gamma LUTs (info
`0x2688–0x5088`) change and GammaTuning's R/G/B/luma tables equal
`compose(base, curves)`. The emulated live view needs two more stubs than the
still path (`0x536e9bd8`, `0x536e91a8`, a device singleton at obj+0x180).

### Phase 1: core

- `core/src/space.rs`: allocator over the padding and the 12 library
  storages, with a fixed reserved block for stub and table. Deterministic
  order: HSL sets into the large contiguous gaps, curve channels (512 B) into
  the rest, private matrix sets last. Reports "does not fit" with numbers.
  `multiaxial::patch` takes its addresses from here instead of its own
  `FREE` loop.
- `core/src/curvesel.rs`: stub as fixed instruction words with disassembly
  comments, table writer, check of the stock bytes before patching. Unit test:
  run the stub in `arm.rs` (integer only) for every look and Color Tone.
- HDR effects off: three one-word patches with stock-byte checks.
- `looks::private_cross` → `private_set(table)`, so HDR (which shares its
  matrix with Standard and Cross Process) can get a matrix of its own.
- `session::export`: `look_curves: {look: [R, G, B]}` (Cross Process keeps
  Color Tone 1–3), `effects_off: ["HdrCuisine"]`, space report in the result.
- Preview API: `tone` takes the curves of any look; new `space` request for
  the editor's meter.
- Tests (`core/tests`, GR III and GR IIIx): no assignment → code and table
  stock; assigned looks get exactly their pointers; full pool refused; HSL and
  curves never overlap; old presets still load.

### Phase 2: editor

- The curves tab works for every colour look; Cross Process keeps the
  Color Tone 1/2/3 choice, other looks have one set. Today this is wired to
  Cross Process (`previewSet` in `app.js`, the tab switch that selects Cross
  Process).
- The preview applies the channel curves of the shown look.
- Space meter ("free: 7.2 KB, enough for 4 RGB or 2 HSL sets"); no new sets
  once the pool is full.
- HDR: "HDR effects off" switch with a note that the tone sliders are fixed
  in the camera.
- Presets: new fields `look_curves`, `effects_off`; old presets stay valid.
- Change list and i18n (de/en).

### Phase 3: docs

`tone-curves.md` (curves per look, stub), `look-mapping.md` (HDR effects,
shared matrices), `editor.md` (usage), `flash-safety.md` (first code patch:
what it changes; a stock update removes it completely).

### Phase 4: one test flash

Build one file and check exactly that image with the Unicorn tool; flashing is
the user's step. Test looks inherit Standard's colour data so Standard is the
exact control:

| Image control | Content |
|---|---|
| Standard | unchanged, control |
| Positive Film | marker curve A (red raised) |
| Vivid | marker curve B (blue in the shadows) |
| Negative Film | marker curve C + HSL marker |
| HDR (effects off) | marker curve D |
| Cross Process Color Tone 1–3 | three different markers |

Shots: same subject and exposure, grey ramp in the frame. Also: live view
while switching looks, in-camera RAW development, power off/on, one return to
stock firmware. Then compare the shots with the preview.

## Risks

- First code patch; it runs on every live view frame. Mitigation: Unicorn run
  of the whole Make for still and live view, unit test of the stub, equal-size
  patch at the site, and the same approach worked on a camera in
  gr3-film-mods.
- Return to stock: no unknown value stays in the user settings; the library
  table is RAM and is reloaded from the image at start-up.
- The stock Cross Process curve storage is reused for other data. Harmless,
  because the stub sets the pointers on every call.

## Credit

[idontknownihil/gr3-film-mods](https://github.com/idontknownihil/gr3-film-mods)
(MIT for code and notes) found the variant patch at the end of the context
fill, the RAM pointer rewrite of the curve library, the HDR/Retro effect
switches and the shading/clarity/grain gate, and tested them on a GR III
(`FINDINGS.md` §6, §14–14b, §23–26, §47). Its film fits are CC BY-SA /
CC BY-NC-SA and are not used here.
