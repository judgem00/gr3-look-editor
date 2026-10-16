# GR III tone curves

How the camera builds its tone curve, where the data lives and how the
[Look Editor](editor.md) edits it. Firmware `fwdc239b`, analysed with
REA/Ghidra and Capstone.

## Summary

- The tone curve is **computed per shot**, not looked up per image control.
  `GammaTuning` (RTOS `0x536e7b64`) blends it from **15 base curves** based on
  four sliders: key, contrast, highlight contrast, shadow contrast.
- The image controls differ in their tone curve only through these slider
  values; GammaTuning does not take a look index.
- The editor runs **the camera's original code** in an ARM interpreter
  (`core/src/arm.rs`; checked against 7261 curves from the CPU emulator
  Unicorn). Edited base curves go through the same arithmetic as in the
  camera; with the same sliders the preview is bit-exact with the camera
  curve.
- Per channel (R, G, B) only Cross Process applies curves in stock
  firmware. A small selector patch lets every colour image control have its
  own ([below](#channel-curves-for-every-colour-look-selector)).

## Data

| What | Where | Format |
|---|---|---|
| 3 curve families | `.data` `0x550204d4[0..2]` | pointer to 5 curve pointers each |
| 15 base curves | e.g. `0x53ddd394`, `0x53dcf7f0` … | 256 × u16, 0…~16300 |
| key table | `0x538f34e0` | 9 × int: 512 … 256 … 128 |
| transition smoothing | `0x53ddd594` | u16 weights, /2048 |

Family 0 = high, 1 = normal, 2 = low contrast. The 5 curves of a family
belong to white point bands (knee at 0x80, 0x100, 0x200, 0x400, 0x800 on the
12-bit input scale).

## The algorithm (`0x536e7b64`)

Inputs from the setting (filled by `0x536e84d0`): key−4, contrast−4,
4−shadows, highlights−4, a flag and a runtime gain (`setting+100`).

1. **Knee:** `gain × key table[key]` gives the knee point on the 12-bit input
   scale and thus the two white point bands.
2. **Contrast per range:** separately for highlights, midtones and shadows,
   `2·contrast + range slider` (−12…+12) is mapped to a blend of two
   neighbouring families.
3. **Blend the bands** so that the value at the knee is **fixed at
   `0x1d80`** — the camera anchors its curve there.
4. **256 sample points** (input in steps of 16): above the knee the
   highlight blend, below it the shadow blend, cross-faded to the midtones in
   the transitions using the smoothing table; output `× 0x4000 / 0x3f02`.

Consequence for edits: a global lift of all base curves is partly cancelled
out again by step 3. That is why the editor shows the **effective** curve
from the emulator, not the naive expectation.

## Image control sliders in the pipeline

`FUN_536db7d8` maps the camera settings per look to the tuning parameters
(monochrome looks: saturation/hue fixed at centre):

| Camera field | Slider | Tuning → setting |
|---|---|---|
| `+0x77d` | saturation | `+0x1e0` → `+0x57` |
| `+0x77e` | hue | `+0x1e2` → `+0x5c` |
| `+0x77f` | key | `+0x1e4` → `+0x68` |
| `+0x780` | contrast | `+0x1e6` → `+0x6c` |
| `+0x781` | highlight contrast | `+0x1e8` → `+0x74` |
| `+0x782` | shadow contrast | `+0x1ea` → `+0x70` |
| `+0x785…788` | filter/toning per look | `+0x1f0` → `+0x60` |
| `+0x793` | shading (vignette) | `+0x1f4` → ? |
| `+0x794` | clarity | `+0x1f6` → ? |
| `+0x795` | grain ("Particle") | `+0x1f8` → ? |
| `+0x797` | Cross Process: Color Tone 1–3 (probably) | `+0x1fe` → ? |

The rows from `+0x793` on are **derived** from two pieces of evidence: the
firmware's parameter names (`StillImageControlNo0Param…`) are in the order
Sharpness, ColorTone…, FilterEffect…, Shading, ClarityControl, Particle. And
`FUN_536db7d8` reads `+0x795` exactly for the looks that have a "Particle"
parameter: MonotoneGR, SoftMonotone, HardMonotone, HighContrastBW and
NegativeFilm. The setting filler `0x536dc0f4` does not read
`+0x1f4…+0x1fe`. They have no effect in the R2Y pipeline: shading, clarity
and grain are applied afterwards by the DigitalFilterProcess, which reads
them from the shooting settings directly ([grain.md](grain.md)).

In the DNG the values are in makernote tag `0x0247` (int16, −4…+4,
`0x8000` = not set). The order saturation, hue, key, contrast, highlights,
shadows, sharpness is **derived** (memory layout and parameter order of the
firmware), not established from the writing code.

## Preview fidelity

Two quantities depend on the shot and are fitted against the camera JPEG: the
input scaling (sensor → 12-bit input) and the gain. Whatever a global curve
cannot represent — above all the image-dependent **shadow correction**
(`ToneControlTuning`, makernote "ShadowCorrection") — is absorbed by a
residual correction from the JPEG that can be switched off.

| Shot | camera curve only | with residual correction | free curve (reference) |
|---|---|---|---|
| Standard, all sliders 0 | 6.9 | 5.0 | 4.9 |
| Negative Film, custom sliders | 9.9 | 7.8 | 7.9 |

(mean deviation from the camera JPEG in 8-bit steps)

The curve itself is exact — the emulator runs the camera code, and the curve
in an exported image is bit-identical to the preview. The remaining deviation
comes from local processing and the non-emulated stages after it.

## Channel curves (R, G, B)

GammaTuning sets up **four output tables** (object `+0x2a08` R, `+0x2c08` G,
`+0x2e08` B, `+0x3008` luma). Normally `0x536e8660` copies the computed curve
into all four. If the filter flag is set (`setting+0x9b`, from the camera
setting `+0x38a98`), each channel is produced through a channel curve:

```
channel[i] = C(base[i])      C interpolated over base >> 6 (steps of 64)
luma[i]    = (77 R + 150 G + 29 B) >> 8
```

The filter is **Cross Process**. Its coordinator (constructor `0x536eea90`,
selection `0x536ef140`) picks three of the **12 channel curves** per variant
(`.data 0x55037604[0..11]` → `0x53efb0e4…0x53efc728`, 256 × u16 each):

| Variant | R | G | B |
|---|---|---|---|
| Random | 0–2, order randomised per shot (`0x53efb910/918/920`) | | |
| Preset 1 | 3 | 4 | 5 |
| Preset 2 | 6 | 7 | 8 |
| Preset 3 | 9 | 10 | 11 |

For the presets the coordinator keeps the current look settings and only sets
the channel curves.

**Menu mapping, established on the camera (2026-10-12):** Cross Process has a
"Color Tone" setting 1–3; it is the last value in makernote tag `0x0247`. A
test firmware with marker curves (preset 1 red, 2 green, 3 blue, Random as a
negative) gave exactly red, green, blue for Color Tone 1, 2, 3: **Color Tone
n = preset n.** The Random set did not appear; it cannot be reached via Color
Tone 1–3. This means three RGB curve sets can be flashed and switched in the
camera.

Open: preset 1 lifts blue in the shadows (curve 5 starts at 1920); in
original shots with Color Tone 1, however, the blacks were not blue. The
preview of the shadows of Color Tone 1 is therefore not certain. In stock
firmware no other image control uses the channel curves; the editor's
selector changes that (next section).

The editor reproduces `0x536e8660` in `chan::compose` (`core/src/chan.rs`);
it is checked to be bit-identical against the emulated original code for all
four sets.

## Channel curves for every colour look (selector)

The coordinator does not care which look is active: it only needs the
variant and reads the curve pointers from the library table, which lives in
RAM (`.data`). So one small code patch gives every colour look curves of its
own ([plan-look-curves.md](plan-look-curves.md); `core/src/curvesel.rs`).

**Where.** At the end of the context fill (`0x536dd270`), `0x536dd444`
compares the look (r1, menu index) with 21 and writes the variant
SI+0x38A98: 2/3/4 for Cross Process Color Tone 1/2/3 (SI+0x797), 0
otherwise. The editor replaces the word at `0x536dd450` with `b 0x53003d00`.
The stub there (26 words) returns through the context fill's own epilogue
(`ldm sp, {r3-sl, fp, sp, pc}`), which also restores the registers it uses.

**What it does.**

```
entry = look - 11                 looks 11..22
Cross Process: entry = 12 + Color Tone (0..2); Color Tone > 2 -> variant 0, as stock
table[entry] = [R, G, B] pointers  (0x53003d80, 15 entries x 12 bytes)
R != 0:  0x55037604[9..11] = R, G, B;  variant = 4
else:    variant = 0
```

Variant 4 makes the flag routine (`0x536db130`) set `setting+0x9b`, and the
selection takes slots 9–11. The three Cross Process entries point at the
stock storage of slots 3–5, 6–8 and 9–11, so Cross Process behaves exactly
as before. Variants 1 (Random) and 5–8 are never set again; the Random
storage (3 × 512 bytes) becomes free space. Monochrome looks (13–16) keep
empty entries.

**Verified in Unicorn** (`tools/verify-tuning.py`, ImageTuning Make for
still and live view; harness adapted from gr3-film-mods):

- The context fill runs before the flag, the selection and the channel fill
  in the same Make; the channel fill runs on every Make, even when
  GammaTuning reuses its cached curve.
- Without an entry the result is byte-identical to stock for every colour
  look and variant; with entries only the gamma LUTs (info `0x2688–0x5088`)
  change, and R, G, B equal `compose(tone curve, curve)`.
- Switching looks back and forth (A → B → A, curves → Cross Process →
  curves) gives the same result as a fresh start. The flag routine raises
  `info+0x82af` when the filter goes from on to off, as stock does after
  Cross Process.
- The selection writes the same targets for Standard with variant 4 as for
  Cross Process Color Tone 3 and nothing in the shooting info.
- `--patched IMAGE` runs an exported image with the real stub against that
  model, all looks in one history.

**Space.** Curves for the other looks share one pool with the HSL sets
(`core/src/space.rs`): the Random storage first, then whatever the HSL sets
leave of the padding. With no HSL all seven colour looks fit; with five HSL
sets one does.

## Cross Process as a carrier for RGB curves

Before the selector, only Cross Process applied channel curves, so it can serve as a "look with
RGB curves":

1. **Take over look data.** The setting filler selects the look data via
   three tables in `.data` (`0x55018d94`, `0x55018cf0`, `0x55018e4c`),
   indexed by the look. If entry 21 (Cross Process) points to the same data
   as entry 25 (Negative Film), Cross Process computes with the matrix and
   look data of Negative Film. Standard and HDR, which previously used the
   same data set as Cross Process, are unaffected. For Negative Film only one
   pointer changes.
2. **Tone curve.** It depends on the sliders, not on the look: in the camera,
   set the same values for Cross Process as for Negative Film and choose a
   preset variant (it takes over the current settings; "Random" sets look and
   sliders itself).
3. **RGB linear.** `C[k] = 64·k` passes the tone curve through. For inputs
   ≥ 0xff·64 the camera only reads the last entry; with `C[255] = 16383`
   white stays white and the deviation stays below one 8-bit step (only the
   top 0.8 %).

In the editor, Cross Process with taken-over Negative Film data and linear
channel curves is pixel-identical to Negative Film; from there the RGB curves
can be edited. Not established: whether other modules use the look index
(21 instead of 25) for their own case distinctions — `FUN_536db7d8` treats 21
differently from 22: Negative Film passes on the grain `+0x795`, Cross
Process `+0x797` instead. Filter and toning (`+0x1f0`, `+0x1f2`) are 0 for
both, as for Standard; so Negative Film has no Cb/Cr toning. The grain acts
outside the R2Y pipeline and is missing from the Cross Process look.

## Colour wheels

The editor's colour wheels are a control layer on top of the channel curves
(`chan::grade` in `core/src/chan.rs`). A wheel with hue h, strength a and
luminance l shifts channel c (φ = 0°, 120°, 240° for R, G, B) by

```
w(x) · (0.2 · a · cos(h − φ_c) + 0.2 · l)
```

with the weights shadows (1 − x)², midtones 4x(1 − x), highlights x², global
1 over the computed tone value x. Balance shifts x → x^(2^−b). The cosine
terms sum to zero, so a tint leaves the channel mean unchanged. The result is
clamped to 0…0x3fff. Monotonicity is not enforced, because several original
channel curves already have small dips.

Selective colour changes (HSL per hue) are not possible this way. They go
through the hue-dependent stage MultiAxial ([multiaxial.md](multiaxial.md)).

## What is open

- The base curves are also read by other paths (pointer table in `.data`
  from `0x55020470`, e.g. `0x536e82d0` for special cases). An edit takes
  effect there too.
- The shadow correction (`0x536e955c`, histogram) is not emulated.
- Retro and Bleach Bypass tonings use their own tables
  (`setting+8` → `+0x34…+0x40`); not editable.
- What the setting `+0x38a98` is in the camera menu (values 1–11, mapping
  via `0x53dbd19c`).
