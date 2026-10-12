# GR III Look Editor

Build your own image control ("look") for the Ricoh GR III. **Your look
replaces the camera's Cross Process slot**, so it shows up in the camera as
image control *Cross Process*. Every other image control stays as Ricoh
made it.

The editor shows a live preview on one of your own GR III DNGs. It renders
the photo the way the camera does: white balance, sensor matrix, look
matrix, saturation and hue, and the tone curve. The tone curve is computed
by running the camera's own tone-curve code in an ARM interpreter. When you
export, the editor patches a copy of the firmware image and repacks it into
`FWDC239B.BIN`, with all checksums recomputed.

> **Warning — read before flashing.** Flashing modified firmware may void
> your warranty and can, in the worst case, leave the camera unusable. There
> is no known recovery procedure yet. The modified update has been tested on
> one camera only. You use this at your own risk.

This is an independent project. It is not affiliated with or endorsed by
Ricoh Imaging. "Ricoh" and "GR" are trademarks of their respective owners.

## How it works

1. **Base.** Your look starts from the colours of Standard, Vivid,
   Positive Film, Negative Film, Retro or Bleach Bypass. The base is copied
   into a colour matrix of the look's own, so the original image controls
   are never touched.
2. **Colour.** Saturation, hue, and Lightroom-style calibration (hue and
   saturation of the red, green and blue primaries).
3. **Curves.** Three variants, which you select in the camera with
   Cross Process → *Tone 1, 2, 3*. Each variant has RGB curves (one combined
   curve or per channel) and colour wheels for shadows, midtones and
   highlights. The curves always start linear, so the stock
   cross-processing is gone.
4. **Export.** Writes the update file and a preset you can load again
   later.

The camera's contrast, saturation, highlight and shadow settings keep working
on your look as on any other image control.

Advanced options:

- editing the other image controls
- a tone curve that applies to all image controls

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

### In the browser

Open the editor's web page and drop Ricoh's update file `fwdc239b.bin`
onto it, then a DNG. Everything runs in your browser as WebAssembly: the
firmware and your photos are not uploaded anywhere. **Create update file**
on the Export tab gives you `FWDC239B.BIN` to download.

The page is plain static files (`editor/static`), published with GitHub
Pages by [`.github/workflows/pages.yml`](.github/workflows/pages.yml). To
run it locally, build the core and serve the folder:

```sh
wasm-pack build core --release --target web --out-dir ../editor/static/pkg --no-typescript --no-pack
python3 -m http.server 8790 --directory editor/static
```

### With Python on your machine

The same interface, with a local Python server doing the work.
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
start the firmware update from the camera menu. Then select the image
control Cross Process: that is your look. To go back, flash Ricoh's
original file the same way.

The interface is in English or German (switch top right).

### Command-line packing

`fw-pack.py` checks and rebuilds update files without the editor:

```sh
./fw-pack.py check fwdc239b.bin
./fw-pack.py pack fwdc239b.bin work/fwdc239b/unpackedFW-looks -o work/fwdc239b/FWDC239B.BIN
```

## Development

The browser version runs a Rust port of the server (`core/`) compiled to
WebAssembly; `editor/static/bridge.js` sends the page's `/api/…` requests
either to `editor/server.py` or, when no server answers, to that core in a
web worker. The camera's tone-curve code runs in a small ARM interpreter
(`core/src/arm.rs`) instead of Unicorn.

The port is checked against the Python editor. `golden/make.py` records
the Python editor's answers for a set of requests, photos and 7261
tone-curve cases into `work/golden/`; the Rust tests must reproduce them,
the update files byte for byte:

```sh
.venv/bin/python golden/make.py --update fwdc239b.bin --dng a.DNG b.DNG
cp fwdc239b.bin work/
cargo test --release --manifest-path core/Cargo.toml
```

Without that data (as on CI) those tests skip.

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
