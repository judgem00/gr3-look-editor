"""Screen chart for the highlight correction test (docs/plan-highlight-correction.md).

Shown full screen and shot as scene A. Everything is placed relative to a
reference grey at 1/32 of the screen's white (linear), so with the camera
metering on it the white sits 5 EV over grey: above the clip point without
highlight correction (+3.45 EV) and with it (about +4.45 EV).

- background: the reference grey; the framed patch: a checkerboard for the
  AF whose linear mean is the reference grey (spot meter here, AE linked to
  the AF point)
- grey wedge -4 .. +5 EV over the reference in 1/3 EV steps
- a smooth log ramp 0 .. +5 EV (to see where and how the edge sets in)
- red, green, blue, skin, yellow, cyan at +1 .. +5 EV (brightest channel)

The EV labels are what the chart sends; the screen's own response does not
need to be exact, the evaluation reads scene EV from the RAW.

usage: python -I tools/highlight-chart.py OUT.png [WIDTH HEIGHT]   (default 3840 2160)
"""
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFont

REF = 1 / 32                     # reference grey, linear, of the screen white
WEDGE = [k / 3 for k in range(-12, 16)]          # -4 .. +5 EV, 28 steps
COLOUR_EV = [1, 2, 3, 3.5, 4, 4.5, 5]
COLOURS = {                      # linear, brightest channel 1
    "R": (1, 0.05, 0.05), "G": (0.05, 1, 0.05), "B": (0.05, 0.05, 1),
    "skin": (1, 0.62, 0.45), "Y": (1, 1, 0.08), "C": (0.08, 1, 1),
}


def encode(lin):
    """linear 0..1 -> sRGB 8 bit"""
    lin = np.clip(np.asarray(lin, dtype=np.float64), 0, 1)
    v = np.where(lin <= 0.0031308, 12.92 * lin, 1.055 * lin ** (1 / 2.4) - 0.055)
    return np.round(v * 255).astype(np.uint8)


def ev_lin(ev):
    return min(REF * 2 ** ev, 1.0)


def main():
    out = sys.argv[1]
    w, h = (int(sys.argv[2]), int(sys.argv[3])) if len(sys.argv) >= 4 else (3840, 2160)
    img = Image.new("RGB", (w, h), tuple(int(c) for c in encode([REF] * 3)))
    d = ImageDraw.Draw(img)
    try:
        font = ImageFont.load_default(size=max(12, h // 90))
    except TypeError:
        font = ImageFont.load_default()
    label = tuple(int(c) for c in encode([REF / 3] * 3))
    m, gap = w // 40, max(4, w // 400)

    # grey wedge: two rows of 14
    rows, per = 2, len(WEDGE) // 2
    pw = (w - 2 * m - (per - 1) * gap) // per
    ph = h // 7
    for i, ev in enumerate(WEDGE):
        r, c = divmod(i, per)
        x, y = m + c * (pw + gap), m + r * (ph + gap + h // 40)
        d.rectangle([x, y, x + pw, y + ph], fill=tuple(int(v) for v in encode([ev_lin(ev)] * 3)))
        d.text((x + gap, y + ph + gap // 2), f"{ev:+.2f}", fill=label, font=font)
    y0 = m + rows * (ph + gap + h // 40)

    # smooth log ramp 0 .. +5 EV
    rh = h // 10
    evs = np.linspace(0, 5, w - 2 * m)
    ramp = encode(np.minimum(REF * 2 ** evs, 1.0))
    band = np.repeat(np.repeat(ramp[None, :, None], 3, axis=2), rh, axis=0)
    img.paste(Image.fromarray(band, "RGB"), (m, y0))
    for ev in range(6):
        x = m + int(ev / 5 * (w - 2 * m - 1))
        d.line([x, y0 + rh, x, y0 + rh + gap * 2], fill=label, width=2)
        d.text((x + gap, y0 + rh + gap), f"{ev:+d} EV", fill=label, font=font)
    y0 += rh + h // 11

    # reference patch (centre left) and colour rows (right)
    ref_w = (w - 2 * m) // 4
    cy = y0 + (h - m - y0) // 2
    rs = min(ref_w, h - m - y0) * 2 // 3
    rx = m + (ref_w - rs) // 2
    d.rectangle([rx - gap, cy - rs // 2 - gap, rx + rs + gap, cy + rs // 2 + gap], fill=label)
    # a checkerboard for the AF, its linear mean the reference grey
    cell = max(8, h // 60)
    yy, xx = np.mgrid[0:rs, 0:rs]
    checker = np.where((yy // cell + xx // cell) % 2 == 0, REF * 1.6, REF * 0.4)
    img.paste(Image.fromarray(np.repeat(encode(checker)[:, :, None], 3, axis=2), "RGB"), (rx, cy - rs // 2))
    d.text((rx, cy + rs // 2 + 2 * gap), "reference grey: spot meter here", fill=label, font=font)

    cx0 = m + ref_w + m
    names = list(COLOURS)
    cw = (w - m - cx0 - (len(COLOUR_EV) - 1) * gap) // len(COLOUR_EV)
    ch = (h - m - y0 - (len(names) - 1) * gap) // len(names)
    for r, name in enumerate(names):
        col = np.array(COLOURS[name])
        for c, ev in enumerate(COLOUR_EV):
            x, y = cx0 + c * (cw + gap), y0 + r * (ch + gap)
            lin = np.minimum(col * REF * 2 ** ev / col.max(), 1.0)
            d.rectangle([x, y, x + cw, y + ch], fill=tuple(int(v) for v in encode(lin)))
            if r == 0:
                d.text((x, y0 - h // 40), f"{ev:+g} EV", fill=label, font=font)
        d.text((cx0 - m + gap, y0 + r * (ch + gap) + ch // 3), name, fill=label, font=font)

    img.save(out)
    print(f"{out}: {w}x{h}, reference grey sRGB {encode(REF)} (linear 1/32 of white)")


if __name__ == "__main__":
    main()
