# Flash safety

What the editor does to keep a modified update file from bricking the
camera, and what it cannot rule out.

> **Disclaimer.** These safeguards lower the risk. They do not remove it.
> Flashing modified firmware may void your warranty and can, in the worst
> case, leave the camera unusable. There is no confirmed recovery procedure
> yet. The modified update has been tested on one GR III and one GR IIIx
> only. The software is provided "as is", without warranty of any kind
> (see [LICENSE](../LICENSE)). The authors accept no liability for damage to
> cameras, data or anything else. **You flash at your own risk.**

## Overview

The safeguards come in four layers:

1. **Input.** Only two known, byte-exact Ricoh files are accepted.
2. **Patching.** Only known data is changed, never code, and every write
   lands in a known place.
3. **Packing.** The file is rebuilt with minimal change and verified the way
   the camera's decoder verifies it.
4. **Use.** The flash itself is a stock Ricoh update, and going back works
   the same way.

## 1. Input: only known firmware

Every address the editor reads or patches was found in one build. A different
firmware version moves code and data, and patching it blindly would write
into the wrong place. So `Session::open`
([core/src/session.rs](../core/src/session.rs)) does this:

- **SHA-256 of the update file.** The file must match one of exactly two
  hashes: GR III 2.10 (`fwdc239b.bin`) or GR IIIx 1.60 (`fwdc243b.bin`).
  Any other file is refused, including a file that was already modified.
- **Container checks.** Header, footer, project number, magic values and the
  file checksum are checked as the camera checks them
  ([firmware-packing.md](firmware-packing.md#the-cameras-checks)).
- **SHA-256 of the decoded image.** After decompressing, the image must match
  the known hash too. This catches a bug in the decoder as well as a damaged
  file.
- **Repack self-test.** The untouched image is packed again, and the result
  must be the original file byte for byte, with zero re-encoded frames. If
  the packer could not reproduce Ricoh's file exactly, it is not trusted to
  build a modified one.

The GR IIIx is the same build as the GR III, compiled two minutes later for
the other model. The two images differ in 99 bytes, all of which are model
identity (name, project number, model ID, version, checksums).
`core/tests/models.rs` checks that both files give the same answers, and that
an export patches both images at the same bytes.

## 2. Patching: data only, at known places

The editor never changes machine code. It writes only colour data: look
matrices, tone curves and channel curves.
The pointers to that data are the only other bytes it changes.

| What | Where | Safeguard |
|---|---|---|
| Look matrix (`looks::patch`) | the look's own 3×3 matrix | 9 values per matrix; each must fit into a signed 16-bit value (about −4.0 … +4.0), otherwise an error. Two looks that share one matrix may not get different values. |
| Private Cross Process set (`looks::private_cross`) | read-only padding at `0x53002080` | Written only if the area is all zero, otherwise an error. Cross Process gets its own copy, so the matrices of the other image controls stay untouched. |
| Channel curves (`chan::write`) | the 12 existing 256-entry tables | Index must be 0–11, exactly 256 values, each clamped to 0…65535. |
| Base tone curves (`gamma::with_curves`) | the existing base curves | Values clamped to 0…65535. This is an advanced option, and the UI warns that it changes every image control. |

What stays untouched:

- the header, including the model name, project number and version (the camera
  refuses a file with the wrong project number)
- the bootloader, the M0 supervisor, the Linux part and the factory
  calibration data
- the image size, because `repack` refuses any change of size

## 3. Packing and verification

`grfw::repack` ([core/src/grfw.rs](../core/src/grfw.rs)) changes as little
as possible:

- Frames are copied byte for byte from Ricoh's file unless their output
  changes. A frame also counts as changed if it copies from changed bytes. In
  that case the check is deliberately conservative and covers everything
  between the frame's lowest source and its end.
- Re-encoded frames are uncompressed and keep to the original 0x6000-byte
  grid, so the camera's decoder sees nothing it would not see in a Ricoh
  file.
- The image checksum (last word of the image) and the file checksum (footer)
  are recomputed properly, not disabled through the `0xffffffff` escape
  values the camera would accept.

Before the file is handed out, `Session::export` parses it again from
scratch and checks the following:

1. All checks of the camera's `FwDecoder`: project number in the header,
   project number twice, header magic, footer project/magic/firmware type,
   and the file checksum.
2. The file decodes to **exactly** the patched image, byte for byte. This
   also covers the image checksum and the decoded length.

If any check fails, no file is written. You get the error *the rebuilt update
file does not verify; not written*.

The command-line tool `fw-pack.py check` runs the same checks independently,
in Python, on any update file:

```sh
./fw-pack.py check FWDC239B.BIN
```

## 4. Tests

`cargo test` (see the README) checks the core against reference data:

- update files byte for byte against known-good exports
- GR III and GR IIIx against each other (`core/tests/models.rs`)
- round trip: decode → repack → identical file

The tests need Ricoh's files in `work/` and skip without them, as on CI.

## 5. Flashing

The editor's file is flashed like a stock update. There is no special mode
and no extra tool.

- **Battery.** Charge it fully first. The update writes to the internal flash
  in place, and an interruption (empty battery, removed card, switching off)
  is the classic brick case. The camera refuses to start the update on a low
  battery, but a full charge leaves a margin.
- **File name and place.** Copy the file unchanged to the root of the SD
  card. The camera only accepts a file with its own project number, so a GR
  III file cannot end up on a GR IIIx or the other way round.
- **Do not interrupt.** Do not open the battery cover or press buttons until
  the camera reports that it is done.
- **The way back.** To return to stock, flash Ricoh's original file the same
  way. The camera accepts the same version again; it only refuses older
  ones. Keep Ricoh's original file.
- **Before the first flash.** Check the result in the preview and with
  `fw-pack.py check`. Make the first test with a look that differs little
  from Standard.

## What remains open (residual risk)

The safeguards above prove that the file passes every check the camera makes
before it writes. They do not prove what happens after that.

- **Static analysis only.** The camera's checks were found by reading its
  code. Checks without an error message in the code could have been missed
  ([firmware-packing.md](firmware-packing.md#what-remains-open)).
- **Boot and runtime behaviour.** Whether the camera boots and computes
  correctly with modified data can only be shown on the hardware. Values
  within the allowed ranges can still produce settings the camera's image
  pipeline has never seen.
- **Little hardware testing.** The modified update has been flashed on one GR
  III and one GR IIIx. Other units, or other settings, may behave
  differently.
- **Recovery unconfirmed.** The firmware contains signs of a recovery path
  (an SD boot mode of the M0 supervisor, a service dump/restore, a factory
  full update). None of them has been confirmed on a real camera
  ([firmware-update-security.md](firmware-update-security.md#recovery-path--analysis)).
  If the main firmware no longer boots, there is currently **no known way**
  to recover the camera yourself.
- **Hardware faults and interruptions.** A power loss, a faulty SD card or a
  flash error during writing can damage the firmware. This applies to stock
  updates too, but with a modified file Ricoh's service may refuse a repair
  under warranty.

> **Once more:** use of this project and of the files it creates is at your
> own risk. The authors accept no liability for any damage. This is an
> independent project, not affiliated with or endorsed by Ricoh Imaging.
