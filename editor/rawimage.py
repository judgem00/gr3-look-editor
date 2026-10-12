"""Decode a GR III DNG into what the preview needs.

- linear camera RGB, black-subtracted, white level = 1.0, no white balance,
  cropped to the DNG's default crop and scaled down for the GPU
- the full-size JPEG the camera embedded, rendered with the shot's settings
- the shot's image-control settings (makernote tag 0x0247)
- the two numbers that tie our linear render to the camera's tone curve
  (input scale and GammaTuning gain), fitted against the camera JPEG
- a residual curve for what a global tone curve cannot explain: the camera's
  image-dependent shadow correction runs on top of GammaTuning

The tone curve itself is not estimated: it is computed by the camera's code
(fwgamma.py) from the shot's settings.
"""
import io, json, struct, subprocess

import numpy as np
import rawpy
from PIL import Image

from firmware import IMAGE_TONE_NAMES

PREVIEW_LONG_EDGE = 1600
RESIDUAL_SIZE = 256
# order of the int16 values in makernote tag 0x0247, inferred from the
# settings layout (+0x77d..) and the parameter order in the firmware's names
PARAM_ORDER = ["saturation", "hue", "key", "contrast", "highlight", "shadow", "sharpness"]
NOT_SET = -0x8000


def shot_info(path):
    """ImageTone, model and the image-control settings, if exiftool is installed"""
    try:
        out = subprocess.run(["exiftool", "-j", "-Pentax:ImageTone", "-Model", path],
                             capture_output=True, text=True, timeout=20)
        info = json.loads(out.stdout)[0]
        raw = subprocess.run(["exiftool", "-b", "-u", "-Pentax:Pentax_0x0247", path],
                             capture_output=True, timeout=20).stdout
    except (OSError, ValueError, IndexError, subprocess.TimeoutExpired):
        return None, None, {}
    params = {}
    if len(raw) >= 2 * len(PARAM_ORDER):
        for name, v in zip(PARAM_ORDER, struct.unpack_from(f"<{len(PARAM_ORDER)}h", raw)):
            if v != NOT_SET and -4 <= v <= 4:
                params[name] = v
    # last value: the Cross Process "Farbton" setting 1..3 (= curve set Preset 1..3),
    # confirmed with marker curves on the camera
    if len(raw) >= 44:
        v = struct.unpack_from("<h", raw, 42)[0]
        if 1 <= v <= 3:
            params["cross_variant"] = v
    return info.get("ImageTone"), info.get("Model"), params


def _downscale(img, factor):
    if factor <= 1:
        return img
    h, w = img.shape[0] // factor * factor, img.shape[1] // factor * factor
    img = img[:h, :w]
    return img.reshape(h // factor, factor, w // factor, factor, -1).mean(axis=(1, 3))


def _orient(img, flip):
    # LibRaw flip codes: 3 = 180, 5 = 90 ccw, 6 = 90 cw
    return {3: lambda a: np.rot90(a, 2), 5: lambda a: np.rot90(a, 1),
            6: lambda a: np.rot90(a, -1)}.get(flip, lambda a: a)(img)


def load(path):
    with rawpy.imread(path) as raw:
        s = raw.sizes
        wb = np.array(raw.camera_whitebalance[:3], dtype=np.float64)
        wb = (wb / wb[1]).tolist()
        # demosaic at half size, camera colour space, linear, no WB
        lin = raw.postprocess(output_color=rawpy.ColorSpace.raw, gamma=(1, 1),
                              no_auto_bright=True, user_wb=[1, 1, 1, 1],
                              output_bps=16, half_size=True, user_flip=0)
        try:
            thumb = raw.extract_thumb()
            jpeg = thumb.data if thumb.format == rawpy.ThumbFormat.JPEG else None
        except rawpy.LibRawError:
            jpeg = None

    # default crop, in half-size coordinates
    top = (s.crop_top_margin - s.top_margin) // 2
    left = (s.crop_left_margin - s.left_margin) // 2
    lin = lin[top:top + s.crop_height // 2, left:left + s.crop_width // 2]
    lin = lin.astype(np.float32) / 65535.0

    factor = max(1, round(max(lin.shape[:2]) / PREVIEW_LONG_EDGE))
    lin = _downscale(lin, factor).astype(np.float32)

    cam = None
    if jpeg:
        cam = Image.open(io.BytesIO(jpeg)).convert("RGB")
        cam = np.asarray(cam.resize((lin.shape[1], lin.shape[0]), Image.BILINEAR),
                         dtype=np.float32) / 255.0

    return {"linear": lin, "camera": cam, "wb": wb, "flip": s.flip}


def render_linear(lin, wb, sensor_ccm, look, mono_mix):
    """the matrix part of the pipeline, as ColorMatrixTuning computes it"""
    c = lin * np.asarray(wb, dtype=np.float32)
    m = np.asarray(look, dtype=np.float32).reshape(3, 3) @ \
        np.asarray(sensor_ccm, dtype=np.float32).reshape(3, 3)
    c = c @ m.T
    if mono_mix:
        c = np.repeat(c.mean(axis=2, keepdims=True), 3, axis=2)
    return c


def apply_lut(lut, x):
    """camera tone LUT (256 entries, input step 16, 0..0x4000) on 12-bit input"""
    f = np.clip(x, 0, 4095) / 16.0
    i = np.minimum(f.astype(np.int32), len(lut) - 2)
    t = f - i
    return (lut[i] * (1 - t) + lut[i + 1] * t) / 16384.0


def fit_camera_tone(rendered, camera, engine, params, samples=30000):
    """input scale and gain that make the camera's curve best match its JPEG,
    plus the residual curve (on the curve's output) for the rest"""
    x = rendered.reshape(-1, 3)
    y = camera.reshape(-1, 3)
    rng = np.random.default_rng(0)
    idx = rng.choice(len(x), min(samples, len(x)), replace=False)
    x, y = x[idx], y[idx]
    keep = y.max(axis=1) < 0.98
    x, y = x[keep], y[keep]
    tone = {k: params.get(k, 0) for k in ("key", "contrast", "highlight", "shadow")}

    best = None
    for gain in np.geomspace(0x600, 0x8000, 20).astype(int):
        lut = np.asarray(engine.curve(gain=int(gain), **tone), dtype=np.float64)
        for scale in np.geomspace(800, 40000, 50):
            e = np.abs(apply_lut(lut, x * scale) - y).mean()
            if best is None or e < best[0]:
                best = (e, int(gain), float(scale))
    _, gain, scale = best

    g = apply_lut(np.asarray(engine.curve(gain=gain, **tone), dtype=np.float64), x * scale)
    g, yv = g.reshape(-1), y.reshape(-1)
    bins = np.minimum((g * (RESIDUAL_SIZE - 1)).astype(np.int32), RESIDUAL_SIZE - 1)
    res = np.full(RESIDUAL_SIZE, np.nan)
    for i in range(RESIDUAL_SIZE):
        sel = yv[bins == i]
        if len(sel) >= 12:
            res[i] = np.median(sel)
    known = np.flatnonzero(~np.isnan(res))
    if len(known) < 8:
        residual = np.linspace(0, 1, RESIDUAL_SIZE)
    else:
        residual = np.interp(np.arange(RESIDUAL_SIZE), known, res[known])
        residual = np.maximum.accumulate(residual)
    return gain, scale, residual.astype(np.float32)


def prepare(path, firmware, engine):
    """everything the editor needs for one DNG"""
    shot = load(path)
    tone_name, model, params = shot_info(path)
    looks = {l["name"]: l for l in firmware.looks()}
    shot_look = IMAGE_TONE_NAMES.get((tone_name or "").lower())

    gain, scale, residual = 0x1800, 4000.0, None
    if shot["camera"] is not None:
        ref = looks.get(shot_look or "Standard")
        # the camera's saturation setting multiplies its own matrix in front
        # (not under Cross Process: there the measured shots fit better without)
        s = 0 if shot_look == "CrossProcess" else params.get("saturation", 0)
        sat = ref["sat_matrices"][max(0, min(8, s + 4))]
        look = (np.asarray(sat).reshape(3, 3) @ np.asarray(ref["matrix"]).reshape(3, 3)).ravel()
        rendered = render_linear(shot["linear"], shot["wb"], firmware.sensor_ccm(),
                                 look, ref["mono_mix"])
        gain, scale, residual = fit_camera_tone(rendered, shot["camera"], engine, params)

    lin = np.ascontiguousarray(_orient(shot["linear"], shot["flip"]))
    cam = shot["camera"]
    cam_jpeg = None
    if cam is not None:
        cam = np.ascontiguousarray(_orient(cam, shot["flip"]))
        buf = io.BytesIO()
        Image.fromarray((cam * 255 + 0.5).astype(np.uint8)).save(buf, "JPEG", quality=92)
        cam_jpeg = buf.getvalue()

    meta = {
        "width": lin.shape[1], "height": lin.shape[0], "wb": shot["wb"],
        "model": model, "image_tone": tone_name, "shot_look": shot_look,
        "params": params, "gain": gain, "scale": scale,
        "residual": residual.tolist() if residual is not None else None,
    }
    return meta, lin.astype("<f4").tobytes(), cam_jpeg
