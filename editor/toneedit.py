"""Edits to the camera's base tone curves.

The firmware keeps 15 base curves (3 contrast families x 5 white-level bands,
see fwgamma.py) and blends them per shot from key, contrast, highlight and
shadow. An edit here is a transfer curve T on the output axis, given as control
points in 0..1, applied to the selected base curves:

    base'(i) = T(base(i))

so the camera's own blending still works on top of the edit: contrast,
highlight and shadow keep doing what they do, starting from a different
character. T is a monotone cubic (Fritsch-Carlson), so a curve that rises
stays rising.
"""
import numpy as np

FULL = 0x4000


def transfer(points, n=1025):
    """T sampled at n points on 0..1; points are [[x, y], ...] in 0..1"""
    pts = sorted((min(1, max(0, x)), min(1, max(0, y))) for x, y in points)
    if not pts or pts[0][0] > 0:
        pts.insert(0, (0.0, 0.0))
    if pts[-1][0] < 1:
        pts.append((1.0, 1.0))
    xs = np.array([p[0] for p in pts])
    ys = np.maximum.accumulate(np.array([p[1] for p in pts]))
    # drop duplicate x
    keep = np.concatenate([[True], np.diff(xs) > 1e-6])
    xs, ys = xs[keep], ys[keep]
    t = np.linspace(0, 1, n)
    if len(xs) < 3:
        return np.interp(t, xs, ys)

    h = np.diff(xs)
    d = np.diff(ys) / h
    m = np.empty_like(xs)
    m[0], m[-1] = d[0], d[-1]
    for k in range(1, len(xs) - 1):
        if d[k - 1] * d[k] <= 0:
            m[k] = 0.0
        else:
            w1, w2 = 2 * h[k] + h[k - 1], h[k] + 2 * h[k - 1]
            m[k] = (w1 + w2) / (w1 / d[k - 1] + w2 / d[k])
    k = np.clip(np.searchsorted(xs, t, side="right") - 1, 0, len(xs) - 2)
    s = (t - xs[k]) / h[k]
    h00, h10 = 2 * s**3 - 3 * s**2 + 1, s**3 - 2 * s**2 + s
    h01, h11 = -2 * s**3 + 3 * s**2, s**3 - s**2
    return h00 * ys[k] + h10 * h[k] * m[k] + h01 * ys[k + 1] + h11 * h[k] * m[k + 1]


def is_identity(points):
    return all(abs(x - y) < 1e-4 for x, y in points)


def apply(base, points, families=(0, 1, 2)):
    """{(family, band): new 256 values} for an edit of the selected families"""
    T = transfer(points)
    out = {}
    for (fam, band), (_, vals) in base.items():
        if fam not in families:
            continue
        v = np.asarray(vals, dtype=np.float64) / FULL
        new = np.interp(np.clip(v, 0, 1), np.linspace(0, 1, len(T)), T) * FULL
        out[(fam, band)] = np.round(new).astype(int).tolist()
    return out
