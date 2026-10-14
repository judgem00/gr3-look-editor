# Image control → data: the mapping

Phase 3 of the colour profile plan: which data belongs to which image
control? For the **colour matrix** the chain is now fully established in the
code.

Firmware `fwdc239b` (GR III), file `assets/fwdc239b/unpackedFW`.

## Summary

```
image control (UI)       ConvertToCommonPropertyImageControl  0x53289058
  -> internal index      byte setting+0x77c (11 … 22)
  -> 0x55018d94[index]   26 pointers, one per index (.data)
       -> ColorMatrix set    +0x00 sensor CCM (same for all looks)
                             +0x04 look matrix   <- the character of the look
  -> ColorMatrixTuning::Calc 0x536deecc -> ISP registers
```

Tool: `tools/look-matrices.py` shows the matrix of each look and writes a
modified one into a copy of the image.

## 1. Second address mapping: `.data` lives at `0x55000000`

The startup code (`0x5300038c`) copies, before anything else, `0x3a080`
bytes from `0x5407b540` to **`0x55000000`** and then zeroes the BSS
(`0x5503a100 … 0x55804374`).

```
RTOS code/rodata:  RAM = file offset + 0x52FEF790
.data:             RAM 0x55000000 + x  =  file offset (0x5407b540 - 0x52FEF790) + x
```

Consequences for earlier findings:

- The "tuning registry" at file offset 17,454,552 **is** this `.data`
  section (RAM `0x55019828`). That is why it had no static references — the
  code addresses it under `0x5501…`.
- Pointers to `0x5501…`/`0x5502…` are not runtime buffers but initialised
  data from the image — and therefore patchable.

## 2. The `ImageTuning` class

Constructor at `0x536da2xx` (`src/ImageTuning.cpp`), members according to
asserts:

| Offset | Member | Init |
|---|---|---|
| +0x154 | InterpolationTuning | `0x536de438` |
| +0x158 | **ColorMatrixTuning** | `0x536ded68` |
| +0x15c | **MultiAxialTuning** | `0x536df468` |
| +0x164 | YCbCrMatrixTuning | `0x536e4694` |
| +0x168 | ColorReferenceTuning | `0x536e487c` |
| +0x17c | **GammaTuning** (0x3240 B) | `0x536e7a10` |
| +0x180 | **ToneControlTuning** | `0x536e90ac` |
| +0x204 / +0x208 | BasicGammaTable Natural / Reversal | – |

Each tuning class receives a `setting` struct; its pointer fields are filled
by the function at `0x536dc0f4` — **indexed by the image control**.

## 3. ColorMatrixTuning::Calc (`0x536deecc`)

1. Only when `setting+0x79` is set (monochrome looks): mix weights
   (`setting+0x7c/7e/80`) × **look matrix** (`set+0x04`)
2. optionally × fixed matrix `0x53dc1430` (`setting+0x52`)
3. × **sensor CCM** (`set+0x00`, `0x53dc2b58`, same for all looks)
4. Saturation: level `setting+0x58` → factor from `0x53dc140c`
   (17 values, /128: 0.5× … 2.0×) × strength `setting+0x5a`
5. Row sums corrected (grey stays grey), clamp ±0x7FF → registers

**Correction:** `setting+0x7c/7e/80` are not white balance gains. Step 1
builds a matrix whose rows are all `[w_R, w_G, w_B]` and multiplies it with
the look matrix (`0x536deddc`, 3×3 product, 1.0 = 8192); the result has
three identical rows, i.e. a monochrome mix. For indices 13, 14 and 16 the
code sets the weights to `0xAAA` (≈ ⅓ per channel), an equally weighted mix;
other weights are presumably the B&W filter effects (not checked).

White balance does not enter `ColorMatrixTuning` at all: sensor CCM and look
matrix are fixed pointers, independent of the white balance. The camera
applies white balance as per-channel gains on camera RGB **before** this
matrix, presumably in the R2Y block `OFG` ("offset, WB gains"), which
`ImageTuning` does not fill; the code writing it is not traced. The editor's preview does the same: camera RGB × WB gains → sensor CCM
→ look matrix → A[saturation].

### Addendum: the complete computation (Ghidra, 2026-10-12)

```
M = A[saturation] × look matrix × sensor CCM          (0x536deddc, 0x536dee50: out = P3 × P4)
    (× sRGB→Adobe RGB 0x53dc1430 in front, if colour space is Adobe RGB, setting+0x52)
Gain: off-diagonal scaled, diagonal set so that each row sums exactly
```

- **A[saturation]**: nine matrices per look (`set+8`), selected by the
  saturation slider −4…+4 (`setting+0x57` → `0x53dc1400`). A[4] is the
  identity; all colour looks share the same table `0x55018fe8`, monochrome
  looks only have the identity, HDR variant 24 a luma matrix.
- **Hue**: does not act here but in YCbCrMatrixTuning (`0x536e4720`):
  `YCbCr = H[hue] × RGB→YCbCr`, H a rotation in Cb/Cr by ≈ 2.5° per step,
  after the tone curve. Table via `0x55018e4c[look]`.
- The row sums of look × sensor are already exactly 1 for all looks; the
  normalisation changes nothing there.

The editor computes both (sliders "Saturation" and "Hue", pre-filled from the
DNG). Measured on `MR021783` (Negative Film, saturation +2): saturation gap
camera/editor 1.39 → 1.25, mean error 5.79 → 5.52. For Cross Process the
DNG saturation value does not fit this way (the fit gets worse); the editor
sets it to 0 there.

Standard against Vivid in red: on a monitor colour wheel (MR024507/08), red
in Standard was much purer than predicted, Vivid matched. On a real red
object in daylight (MR024518/19) the difference disappears: camera Standard
(129, 22, 14) / Vivid (136, 22, 12), same hue, Vivid marginally more
saturated and brighter. The monitor effect comes from red outside the
camera's colour space, not from a look stage.

What remains is a **shared** deviation of the editor for both looks: red a
good 3–4° too orange and slightly undersaturated. Suspected cause: the camera
selects or interpolates the sensor CCM by white balance, the editor uses a
fixed one. Not investigated.

## 4. Index → image control

`ConvertToCommonPropertyImageControl` (`0x53289058`) is a jump table from UI
value 1…12 → internal index:

| Image control | Index | Look matrix | File offset |
|---|---:|---|---:|
| Standard | 11 | `0x53dc2754` | 14,495,684 |
| Vivid | 12 | `0x53dc26dc` | 14,495,564 |
| MonotoneGR | 13 | `0x53dc2a94` (identity) | 14,496,516 |
| SoftMonotone | 14 | `0x53dc27f0` (identity) | 14,495,840 |
| HardMonotone | 15 | `0x53dc2698` (rank 1 = B&W with filter) | 14,495,496 |
| HighContrastBW | 16 | `0x53dc2458` (identity) | 14,494,920 |
| PositiveFilm | 17 | `0x53dc272c` | 14,495,644 |
| BleechBypass_GR | 18 | `0x53dc2474` | 14,494,948 |
| Retro | 19 | `0x53dc2818` | 14,495,880 |
| HdrCuisine | 20 | `0x53dc2754` (= Standard; own copy with effects off) | 14,495,684 |
| CrossProcess | 21 | `0x53dc2754` (= Standard) | 14,495,684 |
| NegativeFilm | 22 → 25 | `0x53dc2ad0` | 14,496,576 |

**Established:** index 14 = SoftMonotone and 20 = HdrCuisine (asserts at
`0x531acac8`/`0x531acafc` compare exactly these values). Indices 13/14/16
are monochrome (code, see above). **Derived:** the remaining names from the
order of the name pool — consistent with all three pieces of evidence and
with the character of the matrices (Vivid saturates, Bleach Bypass/Retro
desaturate, HardMonotone is a rank-1 matrix).

Indices 0–10 and 23–25 belong to other modes (not mapped).

### Cross-check against real shots

Every GR III DNG contains a JPEG rendered by the camera and, in the
makernote, the image control. Rendering the RAW with the sensor CCM and each
of the 12 look matrices (tone curve fitted from the JPEG each time) and
comparing only the colour (deviation from grey, in 8-bit steps), the winner
is:

| Shot style | Best result | Second best |
|---|---|---|
| Positive Film (2 photos) | **PositiveFilm** 5.7 | Standard matrix 6.0 |
| Negative Film | **NegativeFilm** 4.3 | PositiveFilm 6.4 |
| Negative Film | Standard matrix 3.75 | NegativeFilm 3.77 |
| Standard | **Standard** matrix 5.1 | Vivid 5.9 |
| Bleach Bypass (2 photos) | monochrome 6.4 | BleechBypass_GR 7.1 |

The mapping for Positive and Negative Film is thus also supported
empirically. With Bleach Bypass the camera desaturates more than the matrix
alone explains — further tables (saturation/MultiAxial) are at work there. A
Standard photo with a different file naming scheme (`MRI…`) fits PositiveFilm
better; open. Tool: [`tools/look-editor/`](editor.md).

### HDR without its effects

HDR Tone shares Standard's colour data; what makes it HDR are three effects
outside the colour data. With "HDR effects off" the editor patches one word
each (`curvesel::HDR_OFF`, stock word checked first). SI+0x796 is HDR Tone's
level (0 = off, 1–3); only the HDR branches read it:

| Effect | Site | Stock | Patched |
|---|---|---|---|
| local tone mapping | `0x535131fc` | `beq 0x53513274` | `beq 0x53513218` (the Standard path) |
| HDR strength to the pipeline | `0x536dbeb4` | `ldrb sb, [r3, #0x788]` | `mov sb, #0` |
| monochrome variant (table 24) | `0x536dc27c` | `beq 0x536dc818` | `nop` (always table 20) |
| HDR level to the look context (`ctx+0x1fa`) | `0x536dbebc` | `ldrb r2, [r3, #0x796]` | `mov r2, #0` |
| HDR level to the local processing | `0x53503188` | `ldrb r3, [r5, #0x796]` | `mov r3, #0` |

In Unicorn the patched HDR equals Standard bit for bit in ImageTuning Make
(still and live view, all three strengths), and the tone sliders key,
contrast, highlight and shadow stay at neutral, as the HDR branch of the
setting filler (`0x536dbec0` ff.) fixes them. Saturation and hue still act.
The local processing fill `0x5350304c` passes SI+0x796 into its parameter
`+6` for any look; patched it is always 0 (checked in Unicorn). The first
three patches alone were not enough on the camera: shots were right, but
in-camera RAW development of an HDR shot brought the tone mapping back
(2026-10-14). That path fills the local processing parameters without the
gate at `0x535131c0`; HDR files store no clarity/shading values. With all
five patches HDR stays without tone mapping in RAW development too (GR III,
second test flash, 2026-10-14). The first four patches come from
[gr3-film-mods](https://github.com/idontknownihil/gr3-film-mods) (camera-tested
for shots there).

## 5. Correction of earlier assumptions

- `CalcTable` at `0x536e93ac` and the "selection function" `0x536e955c`
  from colour-pipeline-data.md belong to **ToneControlTuning**. `0x536e955c`
  builds a histogram and searches for percentiles — adaptive shadow/DR
  correction, not the image control.
- The matrix `0x53dc272c` ("colour-shifting: green moves into red and blue")
  is the one of **Positive Film**.

## 6. What is open

- **Tone curve:** solved, see [tone-curves.md](tone-curves.md). The curve is
  computed from 15 base curves and the sliders, not selected per look. Bleach
  Bypass and HDR have variants with their own parameter sets (table index
  22/23 and 24). MultiAxial and the toning tables remain open.
- **Shared matrices:** solved for the editor. Standard, HdrCuisine and
  CrossProcess share one ColorMatrix set. `looks::private_set` copies the set
  (21 words) and its matrix into free space and redirects the look's entry in
  `0x55018d94`: Cross Process always (`0x53002080`), HDR when its effects are
  off (`0x53003e40`, next to the curve selector).
- **Repacking the `.bin`**: solved, see
  [firmware-packing.md](firmware-packing.md). Before any flash the recovery
  path still has to be clarified
  ([firmware-update-security.md](firmware-update-security.md)).
- Static analysis: the matrices are demonstrably *loaded*; how visible a
  change is in the image only a test can show.
