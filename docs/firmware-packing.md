# Packing the update file and checksums

How a modified `unpackedFW` becomes an update file again that the camera
accepts. Firmware `fwdc239b` (GR III, build 2025-09-25).

Tool: [`tools/fw-pack.py`](../fw-pack.py), format in
[`tools/fwpack/grfw.py`](../fwpack/grfw.py). The
[Look Editor](editor.md) produces the file directly on export.

```sh
tools/fw-pack.py check firmware/fwdc239b.bin
tools/fw-pack.py pack firmware/fwdc239b.bin assets/fwdc239b/unpackedFW-looks \
    -o assets/fwdc239b/FWDC239B.BIN
```

## Summary

- The container is protected by **two additive checksums**, both "sum of all
  little-endian 32-bit words = 0". No signature, no hash.
- Both can be recomputed. **Repacking the unmodified image yields the
  original file byte for byte.** The format is thus fully understood.
- A modified look matrix costs two re-encoded frames (+24 KB). The file
  passes all checks of the camera's decoder, and the independent `gr_unpack`
  unpacks exactly the modified image.

## File layout

```
0x00000000  header, 0x80 bytes
              +0x00 "RICOH" / "GR III" / company name
              +0x30 project number 0x24c (= 588, "KB588")
              +0x38 version 02 0a 15 1b
              +0x54 build ID 0x438ff331 (also in the RTOS, RAM 0x53002054)
              +0x58 "2025/09/25 11:28:34 JST"
              +0x78 0x024c024c        +0x7c 0xa55a5aa5
0x00000080  compressed stream: 2430 frames, end marker 0x0000
0x01cfbdca  2 bytes padding to a 4-byte boundary
0x01cfbdcc  footer, 0x18 bytes
              +0x00 0x024c024c        +0x04 0xa55a5aa5
              +0x08 FW type 2         +0x0c stream length incl. end marker
              +0x10 unpacked length   +0x14 checksum
```

Frames (format from `gr_unpack`): each frame unpacks to exactly **0x6000
bytes** (one exception with 0x8000, plus the last one). 729 are
uncompressed, 1701 LZ-compressed; back-references (up to 8 KB) reach across
frame boundaries.

## The camera's checks

`FwDecoder` in the RTOS, in order:

| Check | Code | Message on failure |
|---|---|---|
| header +0x30 = project number | `0x5362bd58` | `project number is different!` |
| header +0x78 = project number ×2 | `0x5362bdb0` | `header project number is different!` |
| header +0x7c = `0xa55a5aa5` | `0x5362bdc0` | `header chek data is invalid!` |
| footer +0x00/+0x04/+0x08 | `0x5362be0c` | `footer … is different/invalid!` |
| **word sum of the file = 0** | `0x5362bcc8` | `check sum is invalid!` |
| unpacked length = footer +0x10 | `0x5362c020` | `decode FW size fail…` |
| **word sum of the image = 0** | `0x5362bea8` | `decoded check sum error!` |
| version not older than installed | `0x53628574` | `Update Version is older…` |

Balancing values: footer +0x14 for the file, the **last word of the image**
for the image. The camera skips the file check if the field is `0xffffffff`,
and the image check if the last word is 0, `0xfffffffe` or `0xffffffff`.
`fw-pack` does not rely on that but computes them properly.

The version check only rejects **older** versions (`blo`); the same version
is accepted. `fw-pack` keeps the header unchanged.

The last image word lies behind the last resource data (before it, a JPEG
ends with `ff d9` exactly at `0x38f3af0`). It belongs to no partition.

## How packing works

Minimally invasive: the original stream is carried over frame by frame. Only
frames whose output changes, and frames that copy from changed bytes via a
back-reference, are re-encoded, as uncompressed frames on the 0x6000 grid.
Then the image checksum, footer and file checksum are recomputed. Before
anything is written, the result is decoded again and compared.

## Correction of earlier assumptions

[firmware-update-security.md](firmware-update-security.md) describes a
byte-wise sum that is "folded to one byte" and checked against a header byte
(`CHECKSUM NG: Target`, `fw-checksum.py`). That routine lives in the SRIC
module (`0x53d1dfd8`) and belongs to the **update of the sub-CPU** over the
SRIC protocol, not to the check of the update file. The container checksums
are the two word sums above.

## What remains open

- **Checks after writing.** Statically there is no per-partition checksum and
  no check of the RTOS partition at boot: the M0 boot code only checks the
  factory calibration data (`adjust check sum error!`). This is not proven;
  the camera may check things that have no message text in the code.
- **Recovery.** Still the prerequisite before any flash, see
  [firmware-update-security.md](firmware-update-security.md). The packed file
  passes the camera's checks — whether the modified image then boots and
  computes correctly only the hardware can show.
- **Changed size.** `fw-pack` only supports changes that do not change the
  image size; that is enough for matrices and tables.
