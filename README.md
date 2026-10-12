# GR III Look Editor

Create your own image controls ("looks") for the Ricoh GR III and write
them into a firmware update file the camera accepts.

The editor shows a live preview on one of your own GR III DNGs. It renders
the photo the way the camera does: white balance, sensor matrix, look
matrix, saturation and hue, and the tone curve. The tone curve is computed
by running the camera's own tone-curve code in a CPU emulator. When you
export, the editor patches a copy of the firmware image and repacks it into
`FWDC239B.BIN`, with all checksums recomputed.

> **Warning — read before flashing.** Flashing modified firmware may void
> your warranty and can, in the worst case, leave the camera unusable. There
> is no known recovery procedure yet. The modified update has been tested on
> one camera only. You use this at your own risk.

This is an independent project. It is not affiliated with or endorsed by
Ricoh Imaging. "Ricoh" and "GR" are trademarks of their respective owners.

## What you can change

- **Colour matrix per look.** Edit it directly, or use saturation, hue and
  Lightroom-style calibration (hue and saturation of the red, green and blue
  primaries).
- **Tone curve.** The curve is applied to the camera's base curves, so it
  affects every look. The camera's key, contrast, highlight and shadow
  sliders keep working on top of it.
- **Cross Process R/G/B channel curves** for the camera's Cross Process
  "Tone" settings 1–3, plus colour wheels for shadows, midtones and
  highlights that are converted into those curves.
- **Cross Process with another look's data.** For example, Negative Film
  colours combined with your own RGB curves.

Not possible: per-hue adjustments (HSL). The camera's hardware has such a
stage, but nobody has managed to switch it on yet; see
[docs/image-pipeline.md](docs/image-pipeline.md).

## Supported firmware

Only **GR III, firmware 2.10** (`fwdc239b.bin`, SHA-256
`d6a3c9c2…5354bfd4`). The editor refuses any other file. Every address it
reads or patches was found in this one build.

Download the update file from Ricoh's support page for the GR III. This
repository contains no Ricoh firmware or data taken from it. Everything is
read from the file you provide, on your machine.

## Usage

Requirements: Python 3.10+, optionally `exiftool`. If installed, the editor
uses it to read the look and slider settings of the photo you open.

```sh
./run.sh --update ~/Downloads/fwdc239b.bin
./run.sh --update ~/Downloads/fwdc239b.bin --dng ~/Pictures/R0001234.DNG
```

On the first run, `run.sh` creates a virtualenv (rawpy, numpy, Flask,
Pillow, Unicorn). The editor then decodes the update file into `work/` and
opens `http://127.0.0.1:8765/`. It listens on localhost only.

**Export** writes into `work/fwdc239b/`:

- `unpackedFW-looks`: the patched image
- `unpackedFW-looks.looks.json`: a preset you can load again later
- `FWDC239B.BIN`: the update file, which you can also download from the
  page

To flash, copy `FWDC239B.BIN` unchanged to the root of the SD card and
start the firmware update from the camera menu. To go back, flash Ricoh's
original file the same way.

The user interface is currently in German.

### Command-line packing

`fw-pack.py` checks and rebuilds update files without the editor:

```sh
./fw-pack.py check fwdc239b.bin
./fw-pack.py pack fwdc239b.bin work/fwdc239b/unpackedFW-looks -o work/fwdc239b/FWDC239B.BIN
```

## Documentation

The research notes are in German:

| | |
|---|---|
| [firmware-packing.md](docs/firmware-packing.md) | update file format and checksums |
| [firmware-update-security.md](docs/firmware-update-security.md) | what the camera verifies (a checksum, no signature) |
| [look-mapping.md](docs/look-mapping.md) | where the look matrices live and how they are combined |
| [tone-curves.md](docs/tone-curves.md) | base curves, sliders, Cross Process channel curves |
| [image-pipeline.md](docs/image-pipeline.md) | the whole RAW→YCbCr pipeline of the image processor |
| [editor-de.md](docs/editor-de.md) | the editor in detail |

## Credits

Builds on the firmware exploration in
[hhornbacher/gr3x-fw-hack](https://github.com/hhornbacher/gr3x-fw-hack).

## License

MIT, see [LICENSE](LICENSE).
