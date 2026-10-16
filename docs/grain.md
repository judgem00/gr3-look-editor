# Grain

The camera's grain ("Particle" in the firmware) is a menu setting of the
monochrome image controls and of Negative Film only. The editor can give
grain 1–3 to any colour target, including those whose menu offers none.
Firmware `fwdc239b` (GR III 2.10); the GR IIIx 1.60 image uses the same
addresses.

Status: understood statically, checked in Unicorn (gate and parameter fill,
stock and patched) and confirmed on a GR III (2026-10-15, test firmware
below): Vivid, Positive Film and Negative Film showed grain; **grain 1 is the
strongest, 3 the weakest**. The grain pass itself has not been emulated.

## How the camera does it

The grain is not part of the development (`ImageTuning` Make, R2Y, gamma):
neither the still nor the live view Make reads it or calls the code below
(checked in Unicorn with SI+0x795 = 0…3). Grain is applied afterwards by the
**DigitalFilterProcess** (C++ class `20DigitalFilterProcess`, vtable
`0x53c62890`), together with shading (vignette) and clarity. The three come
from the shooting settings:

| Field | Setting | Values |
|---|---|---|
| SI+0x793 | shading | 0…8, 4 = 0 |
| SI+0x794 | clarity | 0…8, 4 = 0 |
| SI+0x795 | grain | 0 = off, 1 strong, 2 medium, 3 weak |
| SI+0x796 | HDR Tone level | 0 = off, 1…3 |

SI+0x77C is the look (menu index 11…22). The firmware's parameter names
`StillImageControlNo0…2Param{MonotoneGR,SoftMonotone,HardMonotone,HighContrastBW}Particle`
and `AfterReleaseStillImageControlNo0…2ParamNegativeFilmParticle` are why
only these looks show the setting. SI+0x795 is 0 by default (the shooting
settings init writes 0 at `0x53513f08`).

### 1. Gate: does the filter run? (`0x535131c0`)

`r0` = filter object, `r1` = shooting settings; result in `obj+0x24`:

| Look | Filter runs when |
|---|---|
| Standard, Vivid, Positive Film, Bleach Bypass, Retro, Cross Process | shading ≠ 0 or clarity ≠ 0 |
| Monotone, Hard Monotone, Hi-contrast B&W, Negative Film | shading ≠ 0 or clarity ≠ 0 or **grain ≠ 0** |
| Soft Monotone | always |
| HDR Tone | HDR level ≠ 0 |

So for a colour look without the setting, the grain never runs the filter on
its own.

### 2. Parameter fill (`0x5350304c`)

Called from two DigitalFilterProcess methods (`0x535032b4`, `0x535036cc`;
vtable slots 11 and 8). It fills four int16 parameters:

| Parameter | From | Mapping |
|---|---|---|
| +0 | shading | table `0x53c62914`: −4…+4 |
| +2 | clarity | jump table: −4…+4 |
| +4 | **grain** | `0x535031b0`: 0…3 → 0…3, anything else → 0 |
| +6 | HDR level | 0…3 |

The fill has **no look check**: it passes the grain for every look. In-camera
RAW development calls it without the gate
([look-mapping.md](look-mapping.md#hdr-without-its-effects)).

### 3. Filter engine (`0x53680dbc`)

Shading goes to `0x5367b540`, clarity to `0x5367c158`. When parameter +4 ≠ 0
the engine runs its grain pass `0x5367ad3c(obj, image, 0, grain, …)` instead
of the plain path `0x53680088`. That pass works line by line on the image
with 512-byte tables (from `0x538f19d4`). Grain 1 sets an internal step of
0x100, grain 2 0x80, grain 3 0x40; on the camera grain 1 is the strongest and
3 the weakest, so the step scales the amount (or size) of the grain.

### Verified in Unicorn (`tools/verify-grain.py`)

Stock, all twelve looks:

- the gate runs the filter for grain 1…3 exactly for Monotone, Hard
  Monotone, Hi-contrast B&W and Negative Film, always for Soft Monotone, and
  never for grain alone on the other looks;
- the fill turns SI+0x795 = 0…4 into parameter +4 = 0, 1, 2, 3, 0 for every
  look.

## The patch (`core/src/grain.rs`)

One block of 0x68 bytes from the free-space pool, placed by the space planner
like a private colour set (`targets::Item::Grain`). It holds two stubs and a
table of 12 grain values (looks 11…22, 0 = as stock):

| Site | Stock | Patched |
|---|---|---|
| `0x535131c0` (gate entry) | `ldrb r3, [r1, #0x77c]` | `b` gate stub |
| `0x535031b0` (fill) | `ldrb r3, [r5, #0x795]` | `bl` fill stub |

- **Gate stub:** if the look has a grain in the table, set `obj+0x24 = 1` and
  return; otherwise reload the look and continue at `0x535131c4` as stock.
- **Fill stub:** returns the table's grain for the look, or the camera's
  SI+0x795 if the table has 0 there.

The shooting settings are not changed, so the DNG makernote still records the
menu's grain. Where the menu has a grain setting (Negative Film), the table
wins. Both stubs only use registers the stock code at those places treats as
scratch (`r2`, `r3`, `ip`; the fill's `lr` was saved by its prologue).

Checked:

- `core/src/grain.rs` tests: both stubs instruction by instruction in the ARM
  interpreter, for looks with and without a grain, out-of-range looks, and
  camera settings 0…3;
- `core/tests/grain.rs`: the export writes the block and the two sites, and
  nothing else differs from the same targets without grain; no grain, no
  patch; the preset gives the same file again;
- `tools/verify-grain.py --patched`: the camera's own gate and fill on the
  exported image give, for every look and nine inputs (grain, shading,
  clarity, HDR level), either exactly the stock result or filter on with the
  table's grain and stock shading, clarity and HDR level.

## Test firmware

```sh
cargo run --release --example grain_flash --manifest-path core/Cargo.toml
.venv/bin/python tools/verify-grain.py --patched work/grain-test/unpackedFW
```

`work/grain-test/FWDC239B.BIN`: Vivid, Positive Film and Negative Film take
Standard's colour data, so Standard is the exact control; Vivid gets grain
1, Positive Film grain 2, Negative Film grain 3.

Result on a GR III (2026-10-15): all three show grain; Vivid the strongest,
then Positive Film, then Negative Film. The editor therefore offers the values
as strong (1), medium (2) and weak (3).

## Open

- Not reported from the camera test yet: Standard without grain as the
  control, and in-camera RAW development of a grain shot.
- Whether other code paths (e.g. playback effects) read the filter flag of a
  look and are affected by the gate stub. None was found; the gate has a
  single caller (`0x5351361c`).
