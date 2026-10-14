# The GR III image pipeline — overall map

Firmware `fwdc239b`, static analysis (Capstone, custom scripts), as of
2026-10-12. Complements [look-mapping.md](look-mapping.md),
[tone-curves.md](tone-curves.md) and [multiaxial.md](multiaxial.md); those contain the
details on matrix, curves and MultiAxial.

The goal was to know every stage between RAW and YCbCr and to check whether,
apart from MultiAxial, there is a **hue-selective** stage (a prerequisite for
HSL sliders).

## Summary

- `ImageTuning` has 15 stages. Each fills part of a register image
  (`out+0x120`, 0x5d24 bytes for the R2Y core, plus separate blocks for CNR
  and resize). `0x537cd218` copies the image field by field into the
  hardware.
- Only three data sets **differ per image control**: colour matrix
  (`setting+0x08`), MultiAxial (`+0x0c`) and YCbCr matrix/hue (`+0x18`).
  Everything else depends on ISO, mode or sliders.
- **In hardware only MultiAxial is hue-selective.** All other stages are
  matrices, 1D curves, offsets or filters. Candidates I checked and
  discarded: YBCRV/`ColorReferenceTuning` (always off, purpose unclear),
  CNR colour suppression (can only desaturate), IIP LUT (six 1D curves).
- **`CC0CTL` mode 3 does not switch on MultiAxial.** Bit 1 enables the
  `CC0YB…` stage (offset and gain, the black point subtraction according to
  the GoPro patent). Cross-check against existing shots: mode 3 with grey
  matrices (MR024532) and with identity matrices (MR024533) gives the same
  image (chroma 20.2 / 20.9, same brightness). So the MultiAxial matrices
  have no effect.
- **CCA1 is a second 3×3 matrix after the gamma.** It is off in every mode
  (`CC1CTL = 0`, matrix zero). It is a free stage, but not hue-selective.

## Order in the hardware (R2Y core)

The block addresses are in the `R2yCoreUnit` driver (constructor
`0x537d2960`). The names come from the firmware's dump strings. The order
matches the RGB→YUV unit that GoPro patents describe for their
Socionext-based chips: WB → CC0 → [MCC] → black point → tone curve → gamma →
CC1 → [3D LUT] → [MCC] → YUV.

| HW block | Name | Image | Filled by | Content |
|---|---|---|---|---|
| `+0x0000` | CNTL | – | driver | start/stop (bits 8–9), `+0x10` bit 0 edge filter. No per-block enables |
| `+0x0020` | OFG | – | – | offset, WB gains |
| `+0x0080` | CCA0 | `0x000–0x056` | ColorMatrix | `CC0CTL` (2 bit), matrix 9×12 bit (`0x20`), `CC0YBOF` 3×11 bit (`0x40`), `CC0YBGA` 3×9 bit signed (`0x48`), `CC0YBBD` 2×12 bit (`0x50`), `CCYC` 3×8 bit (`0x54`) |
| `+0x1000` | (no dump name) | `0x080–0xa34` | MultiAxial | YCbCr matrix, axes, 60 matrices, 360 range values, control fields |
| `+0x2000` | BTC | `0x1080–0x1086` | ToneControl | adaptive tone curve control |
| `+0x2040` | TC | `0x10c0–0x10da` | ToneControl | tone curve |
| `+0x2060` | GAM | `0x10e0` | Gamma | control bits; curves in LUT RAM |
| `+0x2080` | CCA1 | `0x1100–0x114a` | ColorMatrix | `CC1CTL`, matrix 9×int16, further fields. **Always off** |
| `+0x2100` | (CCA1/CSC) | `0x1180–0x11a1` | YCbCrMatrix | RGB→YCbCr with hue rotation (hue slider) |
| `+0x2140` | YNR | `0x11c0–0x11dc` | LumaNr | luma noise filter |
| `+0x2180…0x23c0` | EGHW, EGMW, EGLW, EGDT, EGCMP, EGCRV | `0x1200–0x1412` | EdgeEnhancement, CoordinateScale (`+0x23a0`) | sharpening |
| `+0x2400–0x24bf` | YBCRV | `0x1420–0x14a1` | ColorReference | 3 sub-blocks, see below. **Always off** |
| `+0x2500` | CLPF | `0x14a8–0x14f6` | ColorLpf | chroma low-pass |
| `+0x2580` | CSP | `0x1528–0x1556` | YCbCrOperation | Cb/Cr offsets (toning), chroma limits |

Outside the R2Y image: `out+0x0` interpolation (demosaic), `out+0x5e44`
SuppreFilter (CNR, including `CSPR` = colour suppression), `out+0x7e44`
ResizeEdge, `out+0x8244` ReduceFilter. The RAW part (Bayer → Bayer: black
level, shading, defective pixels, ELA noise filter) is handled by separate
classes (`RawImageTuning`, `PPUTuning`, `PPUNRTuning`, `AFDFTuning`).

### LUT RAM

At the end of `0x537cd218`, large tables are copied via `memcpy` into the
core's LUT memory (base from the driver object `+0x18`):

| LUT RAM | Image | Size | Source |
|---|---|---|---|
| `0x0000`, `0x8000` | `0x1568`, `0x1d68` | 1024 × u16 each | probably ToneControl (gain table) |
| `0x8800` | `0x2568` | 1024 × u16 | Gamma (probably) |
| `0x9000`, `0x9800`, `0xa000` | `0x2d68`, `0x3568`, `0x3d68` | 1024 × u16 each | Gamma: channel curves R, G, B |
| `0xa800` | `0x4568` | 256 × u16 | open |
| `0xaa00…0xb000` | `0x4768…0x4d68` | 4 × 256 × u16 | Gamma |
| `0xc000…0xc600` and three more | `0x4f68…0x5b68` | 256 × u16 each | open |

## Stages and their data

Dispatchers: `0x536dd51c` (standard), `0x536dd8f4`, `0x536dded0` (reversed
order, special case), `0x536ddd14` (virtual, class `ImageTuningAeSensor`;
works on the persistent `setting` at `ImageTuning+0x14`). Each stage:
`Set(this, setting, image)` = Calc + writer.

| Member | Class | Set | Calc | Writer | Data (`setting+`) |
|---|---|---|---|---|---|
| +0x154 | Interpolation | `0x536decb4` | – | `0x536de454` | ISO table |
| +0x158 | ColorMatrix | `0x536df3d4` | `0x536deecc` | `0x536df13c` | `+0x08` = `0x55018d94[look]` |
| +0x15c | MultiAxial | `0x536e4470` | `0x536df484` | `0x536df55c` | `+0x0c` = `0x55018cf0[look]` |
| +0x160 | LumaNr | `0x536e4634` | `0x536e44d0` | `0x536e4518` | – |
| +0x164 | YCbCrMatrix | `0x536e4760` | `0x536e4720` | inline | `+0x18` = `0x55018e4c[look]` |
| +0x168 | ColorReference | `0x536e4c1c` | – | `0x536e4898` | `+0x28` = `0x53dc5764` (index 9: `0x53dc5750`) |
| +0x16c | CoordinateScale | `0x536e4de4` | `0x536e4c80` | `0x536e4cd0` | `+0x24` = `0x53dc59cc` |
| +0x170 | ColorLpf | `0x536e5070` | `0x536e4e44` | `0x536e4e94` | `+0x20` (mode table) |
| +0x174 | EdgeEnhancement | `0x536e7348` | `0x536e5864` | `0x536e72ec` | ISO tables |
| +0x178 | YCbCrOperation | `0x536e77dc` | `0x536e73d0` | `0x536e7610` | `+0x30` = `0x53dce744` / `0x53dcf5ec` |
| +0x17c | Gamma | `0x536e9000` | `0x536e8fbc` | `0x536e7a24` | curves, see tone-curves.md |
| +0x180 | ToneControl | `0x536e9d1c` | `0x536e9bd8` | `0x536e91a8` | `+0x34`, `+0x40` |
| +0x184 | ResizeEdge | `0x536eaf80` | – | – | ISO table |
| +0x188 | SuppreFilter | `0x536ec3e0` | `0x536eb2b0` | `0x536eb344` | – |
| +0x18c | ReduceFilter | `0x536eb0b0` | `0x536eb000` | `0x536eb074` | – |

Setting filler: `0x536dc0f4`. In the standard dispatcher it hard-sets the
MultiAxial runtime path (`setting+0x94`) to 0. The path is also used by
YCbCrOperation (`+0x94…+0x9a`) and the virtual dispatcher. When it is
active, MultiAxial replaces the axes with `setting+0x9c` (0x40 bytes) and two
banks of 5 matrices each with `setting+0xdc` and `+0x100`. This is probably
meant for factory tuning. What switches the path on is open.

## Individual findings

### CC0CTL and the CC0YB stage

`CC0CTL` has 2 bits. The firmware always writes 1 (matrix on,
`0x536def10`). Bit 1 switches in `CC0YBOF/GA/BD`. The values for it come from
ColorMatrix out of `0x550192f4` (`+0x0c`, `+0x10`, `+0x18`). Effect
(MR024545 ff. against MR024532/33): highlights about twice as bright, black
raised from 3 to 18. That fits offset plus gain, like the "blackpoint RGB"
block in the patent. It has nothing to do with MultiAxial.

### CCA1 (second matrix)

In the constructor (`0x536dece8`), `CC1CTL` (+0x30) is set to 0 and the
matrix (+0x32…+0x42) to 0. Calc never writes there. The stage sits after the
gamma and therefore works on gamma-encoded RGB. A test would be cheap:
`CC1CTL = 1` and an identity matrix (to test the scale), then a colour mix in
gamma space. Not enough for HSL.

### ColorReference → YBCRV

Three data sets: 8 bytes of control (`+0x2400`), plus two identically built
sub-blocks (`+0x2440`, `+0x2480`). Each sub-block consists of:

- enable bits (bit 0 and bit 4)
- a 2-bit mode
- points (2 × 11 bit)
- a 7-bit width
- three byte pairs (lower/upper limit?)
- 9-bit values (default 0 and 0x100 = 1.0)

In all data sets the enables are 0. For index 9 (a different mode) only one
9-bit value differs (0x80 instead of 0x100). Judging by its position in the
core (between the edge curve and the chroma low-pass), the stage is more
likely used for luminance/edge-dependent chroma control than for colour
correction. Without hardware documentation the purpose remains unconfirmed.

### YCbCrOperation → CSP

Calc computes Cb/Cr offsets from four 16-bit values (tables via
`setting+0x60`) with the factors 249/−11. These are the **tonings** of the
monochrome looks (sepia, blue …). The fields `+0x795`/`+0x797` are not part
of it: `+0x795` is the grain, `+0x797` probably the Color Tone choice of
Cross Process (see [tone-curves.md](tone-curves.md)).

### IIP LUT

The IIP processor (playback/retouching, multi-shot HDR) has a LUT unit with
`LUTCTL_A…F` and four `LUTPRECAL` each. These are six 1D curves, not a 3D
LUT. Irrelevant for capture.

## What this means for HSL

1. **MultiAxial is the hardware route, and it works.** The stage sits (as in
   the patent) directly after CC0 in linear RGB. Earlier tests showed no
   effect because the axes and nibbles were zero and the stock control
   fields weight it at almost nothing. The layout came from the PENTAX K-3
   Mark III's Extract Color filter; the editor uses it for its colour ranges
   ([multiaxial.md](multiaxial.md)).
2. **Approximation without MultiAxial:** only possible separated by
   brightness (channel curves, colour wheels), not by hue. That is already in
   the editor.
3. **Unused stages** (CCA1, YBCRV, CC0YB) extend the matrix/offset tools but
   do not replace HSL.

## Tools

The analysis scripts (Capstone, branch index, symbolic evaluation of
`0x537cd218`) lived in the session scratchpad and are not in the repo. The
method for reproducing them:

- Setters `0x537d2xxx…0x537dcxxx` read `ldr r2,[r0]` and then
  `[r2,#reg]`. The handle offset in the driver object gives the block.
- The writers of the tuning classes receive the image in `r1`. The field
  offsets are direct `strb/strh [r1|r1+0x1000, #off]`.
