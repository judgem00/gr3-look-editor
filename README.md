# GR III / GR IIIx Look Editor

Build your own image control ("look") for the Ricoh GR III and GR IIIx. **Your look
replaces the camera's Cross Process slot**, so it shows up in the camera as
image control *Cross Process*. Every other image control stays as Ricoh
made it.

The editor shows a live preview on one of your own DNGs. It renders
the photo the way the camera does: white balance, sensor matrix, look
matrix, saturation and hue, and the tone curve. The tone curve is computed
by running the camera's own tone-curve code in an ARM interpreter. When you
export, the editor patches a copy of the firmware image and repacks it into
`FWDC239B.BIN` (GR III) or `FWDC243B.BIN` (GR IIIx), with all checksums
recomputed.

> **Warning — read before flashing.** Flashing modified firmware may void
> your warranty and can, in the worst case, leave the camera unusable. There
> is no known recovery procedure yet. The modified update has been tested on
> one GR III and one GR IIIx only. You use this at your own risk. What the
> editor does to lower that risk, and what remains open, is in
> [docs/flash-safety.md](docs/flash-safety.md).

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
   Cross Process → *Color Tone 1, 2, 3*. Each variant has RGB curves (one combined
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

| Camera | Firmware | File | SHA-256 |
|---|---|---|---|
| GR III | 2.10 | `fwdc239b.bin` | `d6a3c9c2…5354bfd4` |
| GR IIIx | 1.60 | `fwdc243b.bin` | `708e79f3…b431c7` |

The editor refuses any other file. Every address it reads or patches was
found in the GR III build. The GR IIIx file is the same build for the other
model: its image differs only in the model's name, project number, version
and checksums, none of which the editor touches (see
[docs/firmware-packing.md](docs/firmware-packing.md#gr-iiix)). The preview has
not yet been checked against GR IIIx DNGs.

Download the update file from Ricoh's support page for your camera. This
repository contains no Ricoh firmware or data taken from it. Everything is
read from the file you provide, on your machine.

## Usage

### In the browser

Open [the editor](https://judgem00.github.io/gr3-look-editor/) and drop
Ricoh's update file (`fwdc239b.bin` or `fwdc243b.bin`) onto it, then a DNG. Everything runs in your browser as WebAssembly: the
firmware and your photos are not uploaded anywhere. **Create update file**
on the Export tab gives you `FWDC239B.BIN` or `FWDC243B.BIN` to download.

The page is plain static files (`editor/static`), published with GitHub
Pages by [`.github/workflows/pages.yml`](.github/workflows/pages.yml). To
run it locally, build the core and serve the folder:

```sh
wasm-pack build core --release --target web --out-dir ../editor/static/pkg --no-typescript --no-pack
python3 -m http.server 8790 --directory editor/static
```

### Flashing

Copy the update file unchanged to the root of the SD card and start the
firmware update from the camera menu. Then select the image control Cross
Process: that is your look. To go back, flash Ricoh's original file the same
way.

The interface is in English or German (switch top right).

### Command-line check

`fw-pack.py` (Python 3 with numpy) runs every check the camera's decoder
makes on an update file, and can rebuild one from a modified image:

```sh
./fw-pack.py check FWDC239B.BIN
./fw-pack.py pack fwdc239b.bin unpackedFW-modified -o FWDC239B.BIN
```

## Development

The editor is the static page in `editor/static` plus a Rust core
(`core/`) compiled to WebAssembly. `bridge.js` hands the page's `/api/…`
requests to the core in a web worker. The camera's tone-curve code runs in a
small ARM interpreter (`core/src/arm.rs`).

The core started as a port of a Python editor, which is gone from the tree
(see the git history). Its answers are kept as reference data in
`work/golden/` (not in git, since everything there derives from Ricoh's
firmware or from photos): the editor's answers to a set of requests and
exports, 7261 tone curves computed with the camera's code in Unicorn, and
photos as LibRaw decoded them. The tests compare against that data, the
update files byte for byte:

```sh
cp fwdc239b.bin fwdc243b.bin work/
cargo test --release --manifest-path core/Cargo.toml
```

`core/tests/models.rs` needs both files: it checks that the GR IIIx gives the
same answers as the GR III and that an export patches both images at the same
bytes.

Without that data (as on CI) those tests skip. After an intended change to
what the editor answers, refresh its answers from the core; the Unicorn
curves and the decoded photos stay as they are:

```sh
cargo run --release --example golden --manifest-path core/Cargo.toml
```

## Documentation

| | |
|---|---|
| [flash-safety.md](docs/flash-safety.md) | safeguards against a bricked camera, and the residual risk |
| [firmware-packing.md](docs/firmware-packing.md) | update file format and checksums |
| [firmware-update-security.md](docs/firmware-update-security.md) | what the camera verifies (a checksum, no signature) |
| [look-mapping.md](docs/look-mapping.md) | where the look matrices live and how they are combined |
| [tone-curves.md](docs/tone-curves.md) | base curves, sliders, Cross Process channel curves |
| [image-pipeline.md](docs/image-pipeline.md) | the whole RAW→YCbCr pipeline of the image processor |
| [editor.md](docs/editor.md) | the editor in detail |

## Credits

Builds on the firmware exploration in
[hhornbacher/gr3x-fw-hack](https://github.com/hhornbacher/gr3x-fw-hack).

## License

MIT, see [LICENSE](LICENSE).
