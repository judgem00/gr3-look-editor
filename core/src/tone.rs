//! Edits to the camera's base tone curves (port of editor/toneedit.py).
//!
//! An edit is a transfer curve T on the output axis, given as control points
//! in 0..1, applied to the selected base curves: base'(i) = T(base(i)). T is
//! a monotone cubic (Fritsch-Carlson), so a curve that rises stays rising.
//!
//! numpy's linspace and interp are reproduced operation for operation so the
//! integer curves come out as in the Python editor.

use crate::addr::round_even;

pub const FULL: f64 = 16384.0;

/// numpy.linspace(0, 1, n)
pub fn linspace(n: usize) -> Vec<f64> {
    let step = 1.0 / (n - 1) as f64;
    let mut v: Vec<f64> = (0..n).map(|i| i as f64 * step).collect();
    v[n - 1] = 1.0;
    v
}

/// numpy.interp for one value; xp ascending
pub fn interp(x: f64, xp: &[f64], fp: &[f64]) -> f64 {
    let n = xp.len();
    if n == 1 {
        return fp[0];
    }
    if x < xp[0] {
        return fp[0];
    }
    if x > xp[n - 1] {
        return fp[n - 1];
    }
    // j with xp[j] <= x < xp[j+1]
    let j = xp.partition_point(|&v| v <= x) - 1;
    if j == n - 1 || xp[j] == x {
        return fp[j];
    }
    let slope = (fp[j + 1] - fp[j]) / (xp[j + 1] - xp[j]);
    let r = slope * (x - xp[j]) + fp[j];
    if r.is_nan() {
        return slope * (x - xp[j + 1]) + fp[j + 1];
    }
    r
}

pub fn is_identity(points: &[[f64; 2]]) -> bool {
    points.iter().all(|p| (p[0] - p[1]).abs() < 1e-4)
}

/// T sampled at n points on 0..1
pub fn transfer(points: &[[f64; 2]], n: usize) -> Vec<f64> {
    let mut pts: Vec<(f64, f64)> = points.iter().map(|p| (p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0))).collect();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if pts.is_empty() || pts[0].0 > 0.0 {
        pts.insert(0, (0.0, 0.0));
    }
    if pts.last().unwrap().0 < 1.0 {
        pts.push((1.0, 1.0));
    }
    let mut ys: Vec<f64> = pts.iter().map(|p| p.1).collect();
    for k in 1..ys.len() {
        ys[k] = ys[k].max(ys[k - 1]);
    }
    // drop duplicate x
    let (mut xs2, mut ys2) = (vec![pts[0].0], vec![ys[0]]);
    for k in 1..pts.len() {
        if pts[k].0 - pts[k - 1].0 > 1e-6 {
            xs2.push(pts[k].0);
            ys2.push(ys[k]);
        }
    }
    let (xs, ys) = (xs2, ys2);
    let t = linspace(n);
    if xs.len() < 3 {
        return t.iter().map(|&v| interp(v, &xs, &ys)).collect();
    }
    let m_len = xs.len();
    let h: Vec<f64> = xs.windows(2).map(|w| w[1] - w[0]).collect();
    let d: Vec<f64> = (0..m_len - 1).map(|k| (ys[k + 1] - ys[k]) / h[k]).collect();
    let mut m = vec![0.0; m_len];
    m[0] = d[0];
    m[m_len - 1] = d[m_len - 2];
    for k in 1..m_len - 1 {
        if d[k - 1] * d[k] <= 0.0 {
            m[k] = 0.0;
        } else {
            let (w1, w2) = (2.0 * h[k] + h[k - 1], h[k] + 2.0 * h[k - 1]);
            m[k] = (w1 + w2) / (w1 / d[k - 1] + w2 / d[k]);
        }
    }
    t.iter()
        .map(|&tv| {
            let k = (xs.partition_point(|&x| x <= tv) as isize - 1).clamp(0, m_len as isize - 2) as usize;
            let s = (tv - xs[k]) / h[k];
            let (s2, s3) = (s * s, s.powf(3.0));
            let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
            let h10 = s3 - 2.0 * s2 + s;
            let h01 = -2.0 * s3 + 3.0 * s2;
            let h11 = s3 - s2;
            h00 * ys[k] + h10 * h[k] * m[k] + h01 * ys[k + 1] + h11 * h[k] * m[k + 1]
        })
        .collect()
}

/// a curve (values in units of `full`) through T, rounded half to even
pub fn through(vals: &[u16], t: &[f64], full: f64) -> Vec<i64> {
    let grid = linspace(t.len());
    vals.iter()
        .map(|&v| round_even(interp((v as f64 / full).clamp(0.0, 1.0), &grid, t) * full) as i64)
        .collect()
}

/// new values for the base curves of the selected families:
/// (index into gamma::base_addresses, 256 values)
pub fn apply(base: &[Vec<u16>], points: &[[f64; 2]], families: &[usize]) -> Vec<(usize, Vec<i64>)> {
    let t = transfer(points, 1025);
    base.iter()
        .enumerate()
        .filter(|(i, _)| families.contains(&(i / 5)))
        .map(|(i, vals)| (i, through(vals, &t, FULL)))
        .collect()
}
