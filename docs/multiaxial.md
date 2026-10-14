# MultiAxial: per-hue adjustments (HSL)

MultiAxial is the GR III's hue-selective colour stage. Ricoh does not use
it for the image controls: every look points to one stock set that does
nothing. The editor gives a look a set of its own and drives the stage from
the "Colour ranges" panel (`core/src/multiaxial.rs`, `editor/static/hsl.js`).

The data layout comes from the PENTAX K-3 Mark III. It has the same image
processor and uses the stage for its Extract/Replace Color filters. The
behaviour below was measured on a GR III with test firmware: shots
MR024653–76 of a monitor chart with a stepless colour wheel, against
Standard as the control.

## Where it sits

```
camera RGB -> WB -> CC0 (sensor CCM x look matrix x saturation) -> MultiAxial -> tone curve -> ...
```

The stage works on linear RGB (white = 1.0). A hue rotation there looks
about 2–3× larger in the JPEG, and uneven. So the editor does not convert
angles. The preview applies the stage at its place in the pipeline and then
the camera's tone curve.

## The parameter set

`.data 0x55018cf0[table index]` points to six pointers:

| | Content | Editor |
|---|---|---|
| `[0]` | 16 × u16: 4 descending bounds per quadrant | own |
| `[1]` | 20 × u8: bank for each of the 5 segments of a quadrant | own |
| `[2]` | header, `+0x10` → 60 matrices (12 banks × 5 rows), 3×3 i32, 1.0 = 1024 | own |
| `[3]` | header, `+0x10` → 360 region values | shared (no effect) |
| `[4]` | 21 × u16 control fields | own |
| `[5]` | 9 × i16 RGB → YCbCr, 1.0 = 256 | shared |

The own sets sit in unused read-only padding: `0x53002100…0x53004000` and
`0x53000d50…0x53002000`. Each set takes 0x950 bytes, so up to five image
controls can have HSL.

## Measured behaviour

- **Hue coordinate.** The Cb/Cr plane is split into four quadrants. Within
  a quadrant the coordinate is `2048 · b / (a + b)`, where `a` and `b` are
  the chroma components along the quadrant's first and second axis. Q0 runs
  +Cr (red) → +Cb (blue), Q3 → −Cr (green), Q2 → −Cb (yellow), Q1 → +Cr.
  The editor joins them into one coordinate 0…8192 in that order. The
  measured bounds agree to about ±30 units (≈ 1°). The angle and tangent
  models are off by 100–250 units.
- **Segments.** With bounds `a ≥ b ≥ c ≥ d`, nibble n selects the bank for
  `x ≥ a` (0), `[b, a)`, `[c, b)`, `[d, c)` and `x < d` (4). Unused bounds
  are 0.
- **Matrices.** Only row 0 of a bank acts, so the editor writes all five rows
  alike. Bank 0 is the identity. Rows that sum to 1024 keep grey grey.
- **Edges are hard**, within about 6°. No control field softens them. A
  soft edge is a half-strength step on either side ("feather").
- **Weight.** The effect is blended by a piecewise linear curve over
  `x = 4096 · Y` of the stage's input. Y is row 0 of `[5]` on linear RGB.
  Control field 0x800 switched at Y ≈ 0.49. The curve's breakpoints are
  fields 18–20 (b0 = 0), the weight at each segment start is fields 10–13
  (0x400 = 1.0), and the slopes are fields 14–17 (9 bit signed, per 32
  units). The editor sets fields 1–8 = 0xfff, 9 = 0, 0 = 0. The GR stock
  curve (0 … 0x3e) leaves the stage almost without effect.

## From curves to steps

The editor's Hue vs Hue / Sat / Lum curves are closed monotone cubic curves
through the user's points. The core evaluates them on 512 bins of 16 units
each. It splits every quadrant into at most 5 steps by least squares
(dynamic programming), and every further edge must pay a small cost, so a
flat stretch gets no edge. Steps whose value is almost zero use bank 0.
When more than 11 different steps remain, Ward's method merges the pair
that adds the least error, or merges a step into "unchanged". The answer to
`/api/hsl` contains the smooth curves and the steps, and the editor draws
both.

## What a step or band becomes

A step (or a band, from older presets) has hue (°), saturation and
brightness (−100…100). Its matrix is built in the Cb/Cr plane of `[5]`:

```
M = YCC⁻¹ · [ 1   k·dCb   k·dCr ]  · YCC      s = 1 + sat/100, k = lum/100
            [ 0   s·R(−hue)     ]             d = unit direction of the band centre
```

Rotation and scaling act only on Cb/Cr. The brightness change moves Y in
proportion to the chroma along the band's direction. Grey therefore stays
untouched, even though near-grey pixels fall into random segments. A
positive hue turns towards the next colour in the order red → yellow →
green → blue, as in Lightroom.

For the brightness direction, a merged step uses the mean hue of its parts.

Bands (older presets): the encoder paints them onto the coordinate. Where
bands overlap or their soft edges meet, their changes are averaged, so the
result does not depend on the order of the bands. Equal matrices share a
bank. An edge within 24 units of a quadrant
border moves onto the border. The encoder refuses more than 12 banks or more
than 4 edges in a quadrant. When a quadrant runs short, the soft edges there
are dropped first, and the answer says so.

## Checks

- `core/tests/multiaxial.rs`: export with HSL for several looks, set layout,
  other looks untouched, at most five looks.
- The preview model against the camera: for the flashed sets of MR024666
  (HSL demo) and MR024673 (film test), the predicted JPEG hue change per
  hue range matches the camera to a median of 1.1° and 1.2°, for changes of
  up to −68° and +30°. Chroma steps agree too. Where they do not, the
  editor's base model for Retro/Bleach Bypass is already off without the
  stage.
- Before a flash, run the camera's own Set (`0x536e4470` = Calc
  `0x536df484` + writer `0x536df55c`) in Unicorn on the exported image. Its
  register image (axes `0xa0`, nibbles `0xc0`, matrices `0x100…`, control
  `0xa00…0xa34`) must equal the one from a set built the proven way.
