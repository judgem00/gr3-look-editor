"""Camera test of the post matrix P (docs/plan-post-matrix.md, phase 4).

Every test look computes as Standard up to the YCbCr stage, so the camera JPEG
of a test look should be  RGB_test = P RGB_std  (8-bit gamma RGB, JFIF decode =
inverse of the camera's BT.601). Handheld shots: the test frame is aligned to
the Standard frame (translation + small rotation), then compared on flat
blocks. "Before the curve" alternative: RGB_test = curve(P curve^-1(RGB_std)).

Needs numpy, scikit-image, Pillow and colour-science (e.g. the venv of
tools/spektrafilm-fit.py). Shot numbers below are the first test (photo set
in work/post-matrix-test/, 2026-10-15).

usage: python -I tools/eval-post-matrix.py work/post-matrix-test work/spektrafilm-fit/camera.json
"""
import io, json, sys
import numpy as np
from PIL import Image
from skimage.registration import phase_cross_correlation
from skimage.transform import warp
import colour

DIR, CAM = sys.argv[1], sys.argv[2]
SHOTS = {"Standard": "MR029360", "Vivid": "MR029361", "NegativeFilm": "MR029362", "PositiveFilm": "MR029363"}
preset = json.load(open(f"{DIR}/post-matrix-test.looks.json"))["targets"]
CURVE = np.array(json.load(open(CAM))["curve"], float) / 16384     # 12-bit input / 16 -> output
SCALE = 4


def tiff_jpeg(path):
    """largest embedded JPEG (the camera JPEG) by scanning for SOI..EOI of the biggest one"""
    d = open(path, "rb").read()
    best, i = None, d.find(b"\xff\xd8\xff")
    while i >= 0:
        try:
            im = Image.open(io.BytesIO(d[i:]))
            if best is None or im.size[0] > best.size[0]:
                im.load(); best = im
        except Exception:
            pass
        i = d.find(b"\xff\xd8\xff", i + 3)
    return best


def load(name):
    im = tiff_jpeg(f"{DIR}/{SHOTS[name]}.DNG").convert("RGB")
    a = np.asarray(im, float)
    h, w = a.shape[0] // SCALE * SCALE, a.shape[1] // SCALE * SCALE
    return a[:h, :w].reshape(h // SCALE, SCALE, w // SCALE, SCALE, 3).mean((1, 3)), im.size


def align(ref, mov):
    """ORB features + RANSAC homography (handheld: shift, rotation, a little perspective)"""
    from skimage.feature import ORB, match_descriptors
    from skimage.measure import ransac
    from skimage.transform import ProjectiveTransform
    g = lambda a: (a @ [0.299, 0.587, 0.114]) / 255
    kp = []
    for img in (ref, mov):
        orb = ORB(n_keypoints=3000, fast_threshold=0.05)
        orb.detect_and_extract(g(img))
        kp.append((orb.keypoints, orb.descriptors))
    m = match_descriptors(kp[0][1], kp[1][1], cross_check=True, max_ratio=0.8)
    src, dst = kp[0][0][m[:, 0]][:, ::-1], kp[1][0][m[:, 1]][:, ::-1]      # (x, y)
    model, inl = ransac((src, dst), ProjectiveTransform, min_samples=4, residual_threshold=1.0, max_trials=5000, rng=0)
    out = np.stack([warp(mov[..., k], model, preserve_range=True, mode="edge") for k in range(3)], -1)
    err = np.sqrt((model.residuals(src[inl], dst[inl]) ** 2).mean())
    return out, f"{inl.sum()}/{len(m)} matches, residual {err:.2f} px", model.params[:2, 2]


def blocks(a, n=6):
    h, w = a.shape[0] // n * n, a.shape[1] // n * n
    v = a[:h, :w].reshape(h // n, n, w // n, n, 3)
    return v.mean((1, 3)).reshape(-1, 3), v.std((1, 3)).max(-1).reshape(-1)


def lab(rgb8):
    lin = colour.cctf_decoding(np.clip(rgb8 / 255, 0, 1), "sRGB")
    return colour.XYZ_to_Lab(colour.RGB_to_XYZ(lin, "sRGB"))


def curve_fwd(x):            # linear (12-bit scale 0..4095) -> 8-bit
    return np.interp(np.clip(x, 0, 4095) / 16, np.arange(256), CURVE) * 255


def curve_inv(y):            # 8-bit -> 12-bit linear
    return np.interp(y / 255, CURVE, np.arange(256) * 16)


std, size = load("Standard")
print(f"camera JPEG {size[0]}x{size[1]}, evaluated at 1/{SCALE}")
report = {}
for name in ("PositiveFilm", "Vivid", "NegativeFilm"):
    P = np.array(preset[name]["post"]).reshape(3, 3)
    test, _ = load(name)
    test_al, rot, shift = align(std, test)
    m = 8
    S, sd_s = blocks(std[m:-m, m:-m]); T, sd_t = blocks(test_al[m:-m, m:-m])
    flat = (sd_s < 2.5) & (sd_t < 4) & (S.min(1) > 4) & (S.max(1) < 250) & (T.min(1) > 4) & (T.max(1) < 250)
    S, T = S[flat], T[flat]
    fit = np.linalg.lstsq(S, T, rcond=None)[0].T
    after = S @ P.T
    before = curve_fwd(curve_inv(S) @ P.T)
    res = {}
    for label, pred in (("no P (= Standard)", S), ("P after curve", after), ("P before curve", before), ("fitted 3x3", S @ fit.T)):
        d = colour.delta_E(lab(pred), lab(T), method="CIE 2000")
        res[label] = (np.sqrt(((pred - T) ** 2).mean()), d.mean(), np.percentile(d, 95))
    chroma = np.hypot(*lab(S)[:, 1:].T)
    report[name] = res
    print(f"\n{name}: aligned {rot}, shift {shift.round(1)} px; {flat.sum()} flat blocks, "
          f"chroma of the scene: median {np.median(chroma):.1f}, p90 {np.percentile(chroma, 90):.1f}")
    print("  intended P:", np.round(P, 3).tolist())
    print("  fitted    :", np.round(fit, 3).tolist(), " max |diff|", np.abs(fit - P).max().round(3))
    for label, (rms, de, p95) in res.items():
        print(f"  {label:20s} RMS {rms:5.2f} levels   dE2000 mean {de:5.2f}  p95 {p95:5.2f}")
