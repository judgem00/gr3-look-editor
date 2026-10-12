#!/usr/bin/env python3
"""Look editor: edit the colour matrices and the tone curves of the GR III
image controls against a live preview of a GR III DNG.

    ./run.sh --update fwdc239b.bin [--dng photo.DNG] [--port 8765]

The browser does the rendering (WebGL); this server decodes DNGs, reads the
firmware's look data, computes tone curves by running the camera's own
GammaTuning code (fwgamma.py) and writes patched copies of the firmware image.
The input image is never modified.
"""
import argparse, hashlib, json, os, sys, tempfile, threading, webbrowser

import numpy as np

from flask import Flask, Response, abort, jsonify, request, send_file, send_from_directory

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, ".."))
WORK = os.path.join(REPO, "work")   # decoded images and exports, kept out of git
sys.path.insert(0, HERE)

sys.path.insert(0, os.path.join(REPO, "fwpack"))

import chancurves  # noqa: E402
import firmware as fwmod  # noqa: E402
import fwgamma  # noqa: E402
import grfw  # noqa: E402
import rawimage  # noqa: E402
import toneedit  # noqa: E402

app = Flask(__name__, static_folder=os.path.join(HERE, "static"), static_url_path="")
state = {"fw": None, "base_fw": None, "inherit": {}, "engine": None, "container": None,
         "shot": None, "exported": {},
         "lock": threading.Lock(), "engine_lock": threading.Lock()}
TONE_KEYS = ("key", "contrast", "highlight", "shadow")


@app.get("/")
def index():
    return send_from_directory(app.static_folder, "index.html")


@app.get("/api/firmware")
def get_firmware():
    fw = state["fw"]
    return jsonify({
        "path": os.path.relpath(fw.path, REPO),
        "looks": fw.looks(),
        "sensor_ccm": fw.sensor_ccm(),
        "luma": fw.luma_weights(),
        "ycc": fw.ycc_base(),
        "container": os.path.basename(state["container"]) if state["container"] else None,
        "channel_curves": chancurves.read(fw.data),
        "channel_sets": chancurves.SETS,
        "channel_linear": chancurves.LINEAR,
        "inherit": state["inherit"],
    })


@app.post("/api/inherit")
def set_inherit():
    """let a look use another look's data: {target, source}; source null undoes it"""
    body = request.get_json(force=True)
    target, source = body.get("target"), body.get("source")
    names = {l["name"] for l in state["base_fw"].looks()}
    if target not in names or (source is not None and source not in names) or source == target:
        abort(400, "unknown look")
    inherit = dict(state["inherit"])
    if source:
        inherit[target] = source
    else:
        inherit.pop(target, None)
    state["inherit"] = inherit
    state["fw"] = state["base_fw"].with_inheritance(inherit)
    return get_firmware()


@app.post("/api/dng")
def post_dng():
    f = request.files.get("file")
    if f is None:
        abort(400, "no file")
    with tempfile.NamedTemporaryFile(suffix=".dng", delete=False) as tmp:
        f.save(tmp)
        path = tmp.name
    try:
        return jsonify(open_dng(path, f.filename))
    finally:
        os.unlink(path)


def open_dng(path, filename):
    try:
        with state["engine_lock"]:
            state["engine"].set_curves({})
            meta, linear, camera = rawimage.prepare(path, state["fw"], state["engine"])
    except Exception as e:  # LibRaw raises a family of its own errors
        abort(422, f"could not decode {filename}: {e}")
    meta["filename"] = filename
    with state["lock"]:
        state["shot"] = {"meta": meta, "linear": linear, "camera": camera}
    return meta


@app.get("/api/dng")
def get_dng():
    """the DNG already loaded, e.g. with --dng; 204 if there is none"""
    if not state["shot"]:
        return "", 204
    return jsonify(state["shot"]["meta"])


@app.get("/api/dng/linear")
def get_linear():
    shot = state["shot"] or abort(404)
    return Response(shot["linear"], mimetype="application/octet-stream")


@app.get("/api/dng/camera.jpg")
def get_camera():
    shot = state["shot"] or abort(404)
    if shot["camera"] is None:
        abort(404)
    return Response(shot["camera"], mimetype="image/jpeg")


@app.post("/api/tone")
def tone():
    """the camera's tone curve for the given settings, with and without an edit"""
    body = request.get_json(force=True)
    params = {k: int(max(-4, min(4, (body.get("params") or {}).get(k, 0)))) for k in TONE_KEYS}
    gain = int(body.get("gain") or 0x1800)
    edit = body.get("edit") or {}
    points = edit.get("points") or [[0, 0], [1, 1]]
    families = tuple(edit.get("families") or (0, 1, 2))
    engine = state["engine"]
    with state["engine_lock"]:
        engine.set_curves({})
        ref = engine.curve(gain=gain, **params)
        if toneedit.is_identity(points):
            lut = ref
        else:
            engine.set_curves(toneedit.apply(engine.base, points, families))
            lut = engine.curve(gain=gain, **params)
            engine.set_curves({})
    T = toneedit.transfer(points, n=129)
    out = {"lut": lut, "lut_ref": ref, "transfer": T.tolist()}

    # channel curves: effective values of all 12, and R/G/B for the previewed set
    chan = body.get("chan") or {}
    original = chancurves.read(state["fw"].data)
    pre = effective_channels(original, chan.get("edits") or {})
    effective = graded_channels(pre, chan.get("grade") or {})
    out["channel_curves"] = effective
    out["channel_curves_pre"] = pre
    out["channel_transfer"] = {k: toneedit.transfer(v.get("points") or [[0, 0], [1, 1]], n=129).tolist()
                               for k, v in (chan.get("edits") or {}).items()}
    rgb_set = chan.get("set")
    if rgb_set:
        out["rgb"] = [chancurves.compose(lut, effective[i]) for i in rgb_set]
        out["rgb_ref"] = [chancurves.compose(ref, original[i]) for i in rgb_set]
    return jsonify(out)


def effective_channels(original, edits):
    """the 12 channel curves after edits {index: {base: [256] | None, points}}"""
    eff = [list(c) for c in original]
    for key, e in edits.items():
        i = int(key)
        base = e.get("base") or original[i]
        points = e.get("points") or [[0, 0], [1, 1]]
        if toneedit.is_identity(points):
            eff[i] = [int(v) for v in base]
        else:
            T = toneedit.transfer(points)
            v = [min(1.0, max(0.0, x / toneedit.FULL)) for x in base]
            eff[i] = np.round(np.interp(v, np.linspace(0, 1, len(T)), T) * toneedit.FULL).astype(int).tolist()
    return eff


def graded_channels(curves, grades):
    """the 12 curves with colour grading wheels applied per set {set name: wheels}"""
    out = [list(c) for c in curves]
    for name, g in grades.items():
        idx = chancurves.SETS.get(name)
        if idx is None or chancurves.grade_is_neutral(g):
            continue
        for i, c in zip(idx, chancurves.grade([curves[i] for i in idx], g)):
            out[i] = c
    return out


@app.post("/api/export")
def export():
    body = request.get_json(force=True)
    matrices = body.get("matrices") or {}
    tone = body.get("tone") or {}
    points = tone.get("points") or [[0, 0], [1, 1]]
    tone_changed = not toneedit.is_identity(points)
    channels = {int(k): v for k, v in (body.get("channels") or {}).items()}
    # even without edits the export is a new look: Cross Process with linear
    # curves and its own matrix
    # default next to the input, which keeps the 60 MB image out of git
    default = os.path.join(os.path.dirname(state["fw"].path), "unpackedFW-looks")
    out = os.path.abspath(os.path.join(REPO, body.get("output") or default))
    if os.path.commonpath([out, WORK]) != WORK:
        abort(400, "exports go below work/")
    if os.path.abspath(out) == os.path.abspath(state["fw"].path):
        abort(400, "refusing to overwrite the input image")
    try:
        data = state["fw"].patched(matrices)
    except ValueError as e:
        abort(400, str(e))
    if tone_changed:
        families = tuple(tone.get("families") or (0, 1, 2))
        data = fwgamma.with_curves(data, toneedit.apply(fwgamma.base_curves(data), points, families))
    if channels:
        try:
            data = chancurves.write(data, channels)
        except ValueError as e:
            abort(400, str(e))
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "wb") as f:
        f.write(data)
    with open(out + ".looks.json", "w") as f:
        json.dump({"source": os.path.relpath(state["fw"].path, REPO), "matrices": matrices,
                   "tone": tone if tone_changed else None,
                   "channel_curves": channels or None,
                   # to edit the wheels again: the curves before grading and the wheels
                   "channel_curves_pre": body.get("channels_pre") or None,
                   "channel_grade": body.get("grade") or None,
                   # base + sliders (incl. calibration) to edit again; the camera uses "matrices"
                   "matrix_edits": body.get("matrix_edits") or None,
                   "inherit": state["inherit"] or None}, f, indent=2)
    result = {"written": os.path.relpath(out, REPO),
              "preset": os.path.relpath(out + ".looks.json", REPO)}
    exported = {"image": out, "preset": out + ".looks.json"}

    if body.get("update_file") and state["container"]:
        # the camera looks for C:\FWDC<n>B.BIN, so keep the original's name
        name = os.path.basename(state["container"]).upper()
        target = os.path.join(os.path.dirname(out), name)
        original = grfw.Firmware.load(state["container"])
        image = grfw.fix_image_checksum(data)
        packed, redo = grfw.repack(original, image)
        rebuilt = grfw.Firmware(packed)
        if rebuilt.check_container(original.project, original.fw_type) or rebuilt.decode() != image:
            abort(500, "the rebuilt update file does not verify; not written")
        with open(target, "wb") as f:
            f.write(packed)
        result["update_file"] = os.path.relpath(target, REPO)
        result["frames_reencoded"] = len(redo)
        exported["update"] = target
    # only what this export wrote can be downloaded, by kind, never by path
    state["exported"] = exported
    result["downloads"] = {kind: f"/api/download/{kind}" for kind in exported}
    return jsonify(result)


@app.get("/api/download/<kind>")
def download(kind):
    path = state["exported"].get(kind)
    if not path or not os.path.exists(path):
        abort(404, "nothing exported yet")
    return send_file(path, as_attachment=True, download_name=os.path.basename(path),
                     mimetype="application/octet-stream")


# Every address the editor reads and patches was found in this one build. Other
# versions move code and data, so they are refused rather than patched blindly.
SUPPORTED = {
    # sha256 of the update file as Ricoh distributes it -> (name, sha256 of the decoded image)
    "d6a3c9c288d784080e7b283222ecdd8752adfeed6faa36dab44f73115354bfd4":
        ("fwdc239b", "53da690797f35ad6dc2c429d8ca1d0de8e333c211ed959816813982cd093b703"),
}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def unpack(update):
    """decode the update file into work/<name>/unpackedFW once; return its path"""
    with open(update, "rb") as f:
        raw = f.read()
    known = SUPPORTED.get(sha256(raw))
    if not known:
        sys.exit(f"{update}: unknown firmware file. Only these builds are supported: "
                 f"{', '.join(n for n, _ in SUPPORTED.values())} (see README)")
    name, image_hash = known
    path = os.path.join(WORK, name, "unpackedFW")
    if not os.path.exists(path):
        image = grfw.Firmware(raw).decode()
        if sha256(image) != image_hash:
            sys.exit(f"{update}: decoded image does not match the known build")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(image)
        print(f"decoded {update} -> {os.path.relpath(path, REPO)}")
    return path


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--update", required=True,
                    help="the original update file from Ricoh, e.g. fwdc239b.bin")
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--dng", help="DNG to open at start")
    ap.add_argument("--no-browser", action="store_true")
    args = ap.parse_args()

    firmware = unpack(os.path.abspath(args.update))
    # Cross Process is the user's look: its channel curves start linear (no
    # cross-processing), and it gets a colour matrix of its own
    with open(firmware, "rb") as f:
        stock = f.read()
    linear = chancurves.write(stock, {i: chancurves.LINEAR for i in range(chancurves.COUNT)})
    state["base_fw"] = fwmod.Firmware(firmware, linear)
    state["fw"] = state["base_fw"].with_inheritance({})
    state["engine"] = fwgamma.GammaEngine(state["fw"].data)
    state["container"] = os.path.abspath(args.update)
    if args.dng:
        with app.app_context():
            open_dng(os.path.abspath(args.dng), os.path.basename(args.dng))
    url = f"http://127.0.0.1:{args.port}/"
    if not args.no_browser:
        threading.Timer(1.0, lambda: webbrowser.open(url)).start()
    print(f"look editor on {url}  (firmware: {os.path.relpath(firmware, REPO)})")
    app.run(host="127.0.0.1", port=args.port, debug=False)


if __name__ == "__main__":
    main()
