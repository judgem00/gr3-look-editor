"""Per-channel tone curves (R, G, B) of Cross Process.

GammaTuning keeps four output tables -- R, G, B and a luma table. Normally the
one computed curve is copied into all of them. When the digital-filter flag
is set (setting+0x9b, from the camera setting at +0x38a98), RTOS 0x536e8660
instead builds each channel from the computed curve through a channel curve:

    channel[i] = C(base[i])      C interpolated on base >> 6, step 64
    luma[i]    = (77 R + 150 G + 29 B) >> 8

The channel curves are 12 tables of 256 x u16 in the image, reached through
.data 0x55037604[0..11]. The filter is Cross Process; its coordinator
(constructor 0x536eea90, selection 0x536ef140) picks three curves per variant:

    random     curves 0, 1, 2, assigned to R, G, B in a random order per shot
    preset 1   3, 4, 5
    preset 2   6, 7, 8
    preset 3   9, 10, 11

The camera menu reaches the presets through Cross Process "Farbton" 1, 2, 3
(stored as the last value of makernote tag 0x0247). Confirmed on the camera
with marker curves: preset 1 red, 2 green, 3 blue came out exactly so for
Farbton 1, 2, 3; "random" (made a negative) never appeared.
"""
import struct

import numpy as np

from fwgamma import file_offset

TABLE = 0x55037604
COUNT = 12
CURVE_LEN = 256
SETS = {
    "Zufall": (0, 1, 2),
    "Preset 1": (3, 4, 5),
    "Preset 2": (6, 7, 8),
    "Preset 3": (9, 10, 11),
}


# C[k] = 64 k passes the tone curve through. The camera reads the last entry
# for every input >= 0xff * 64, so the top 0.4 % cannot be exact; 16383
# keeps white at white and stays within one 8-bit level.
LINEAR = [64 * k for k in range(CURVE_LEN - 1)] + [16383]


def addresses(image):
    return [struct.unpack_from("<I", image, file_offset(TABLE + 4 * i))[0] for i in range(COUNT)]


def read(image):
    """[12 x [256 values]]"""
    return [list(struct.unpack_from(f"<{CURVE_LEN}H", image, file_offset(va)))
            for va in addresses(image)]


def write(image, curves):
    """image bytes with {index: [256 values]} written in"""
    img = bytearray(image)
    addrs = addresses(image)
    for idx, vals in curves.items():
        idx = int(idx)
        if not 0 <= idx < COUNT or len(vals) != CURVE_LEN:
            raise ValueError(f"channel curve {idx}: need 256 values")
        vals = [min(0xFFFF, max(0, int(round(v)))) for v in vals]
        struct.pack_into(f"<{CURVE_LEN}H", img, file_offset(addrs[idx]), *vals)
    return bytes(img)


def compose(base, curve):
    """channel table from the computed tone curve, as 0x536e8660 does it"""
    base = np.asarray(base, dtype=np.int64)
    c = np.asarray(curve, dtype=np.int64)
    k = base >> 6
    f = base & 0x3F
    inside = k < 0xFF
    kk = np.minimum(k, 0xFE)
    d = (c[kk + 1] - c[kk]) * f
    d = np.where(d < 0, d + 0x3F, d) >> 6          # C division, toward zero
    out = np.where(inside, (c[kk] + d) & 0xFFFF, c[0xFF])
    return out.astype(int).tolist()


# ---- colour grading wheels ---------------------------------------------------
#
# A wheel tints a tonal range: hue (degrees, red 0, yellow 60, green 120 ...),
# amount 0..1 and a luminance offset -1..1. On a channel curve C (tone value x
# in, channel value out) that becomes an offset per channel,
#
#     C'_c(x) = C_c(x) + sum_r w_r(x) * (S * amount_r * cos(hue_r - phi_c) + L * lum_r)
#
# with phi = 0, 120, 240 for R, G, B (the cosines sum to zero, so a tint
# leaves the channel mean alone) and weights over the tone value x:
#
#     shadows (1 - x)^2, midtones 4 x (1 - x), highlights x^2, global 1
#
# Balance shifts the weights: x -> x ** 2 ** (-balance). This acts on the
# tone-mapped value per channel, like RGB curves, not in a perceptual space.

WHEELS = ("shadows", "midtones", "highlights", "global")
TINT = 0.2          # channel offset at amount 1, in units of full scale
LUMA = 0.2          # offset at luminance +-1
FULL = 0x3FFF


def _weights(x, balance):
    xb = x ** (2.0 ** (-balance))
    return {"shadows": (1 - xb) ** 2, "midtones": 4 * xb * (1 - xb), "highlights": xb ** 2,
            "global": np.ones_like(x)}


def grade_is_neutral(grade):
    return not grade or all(
        abs(float((grade.get(w) or {}).get("amount", 0))) < 1e-4 and
        abs(float((grade.get(w) or {}).get("lum", 0))) < 1e-4 for w in WHEELS)


def grade(curves, grade):
    """[R, G, B] channel curves (256 values each) with the wheels applied"""
    if grade_is_neutral(grade):
        return [list(map(int, c)) for c in curves]
    x = np.minimum(1.0, np.arange(CURVE_LEN) * 64 / FULL)
    w = _weights(x, max(-1.0, min(1.0, float(grade.get("balance", 0)))))
    out = []
    for ch, c in enumerate(curves):
        y = np.asarray(c, dtype=float) / FULL
        for name in WHEELS:
            wh = grade.get(name) or {}
            amount = max(0.0, min(1.0, float(wh.get("amount", 0))))
            lum = max(-1.0, min(1.0, float(wh.get("lum", 0))))
            hue = np.radians(float(wh.get("hue", 0)) - 120 * ch)
            y = y + w[name] * (TINT * amount * np.cos(hue) + LUMA * lum)
        # no monotonicity forced: several firmware curves have small dips
        y = np.clip(y, 0, 1)
        out.append(np.round(y * FULL).astype(int).tolist())
    return out
