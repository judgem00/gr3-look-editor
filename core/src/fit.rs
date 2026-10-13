//! The two numbers that tie the linear render to the camera's tone curve
//! (input scale and GammaTuning gain), fitted against the camera JPEG, and a
//! residual curve for what a global tone curve cannot explain -- the camera's
//! image-dependent shadow correction (port of rawimage.fit_camera_tone).
//!
//! The Python editor picked its 30000 sample pixels with numpy's random
//! generator; this uses its own fixed sequence, so the fit agrees closely but
//! not to the last digit.

use crate::gamma::ToneParams;

pub const RESIDUAL_SIZE: usize = 256;
const RESIDUAL_MIN_COUNT: usize = 12; // samples a bin needs to count
const RESIDUAL_SIGMA: f64 = 8.0; // smoothing, in bins
const RESIDUAL_MIN_STEP: f64 = 0.25 / 255.0; // least rise per bin
const SAMPLES: usize = 30000;

/// the matrix part of the pipeline, as ColorMatrixTuning computes it
pub fn render_linear(lin: &[f32], wb: [f64; 3], sensor_ccm: &[f64], look: &[f64], mono_mix: bool) -> Vec<f32> {
    let mut m = [0f32; 9];
    for r in 0..3 {
        for c in 0..3 {
            m[3 * r + c] = (0..3).map(|k| look[3 * r + k] as f32 * sensor_ccm[3 * k + c] as f32).sum();
        }
    }
    let wb = wb.map(|v| v as f32);
    let mut out = vec![0f32; lin.len()];
    for (o, p) in out.chunks_exact_mut(3).zip(lin.chunks_exact(3)) {
        let q = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
        for r in 0..3 {
            o[r] = m[3 * r] * q[0] + m[3 * r + 1] * q[1] + m[3 * r + 2] * q[2];
        }
        if mono_mix {
            let mean = (o[0] + o[1] + o[2]) / 3.0;
            o.fill(mean);
        }
    }
    out
}

/// camera tone LUT (256 entries, input step 16, 0..0x4000) on 12-bit input
pub fn apply_lut(lut: &[f64], x: f64) -> f64 {
    let f = x.clamp(0.0, 4095.0) / 16.0;
    let i = (f as usize).min(lut.len() - 2);
    let t = f - i as f64;
    (lut[i] * (1.0 - t) + lut[i + 1] * t) / 16384.0
}

/// numpy.geomspace(a, b, n)
fn geomspace(a: f64, b: f64, n: usize) -> Vec<f64> {
    let (la, lb) = (a.log10(), b.log10());
    let step = (lb - la) / (n - 1) as f64;
    let mut v: Vec<f64> = (0..n).map(|k| 10f64.powf(la + k as f64 * step)).collect();
    v[0] = a;
    v[n - 1] = b;
    v
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

/// (gain, scale, residual) for `rendered` against `camera` (both RGB 0..1);
/// `curve(gain)` is the camera's tone curve for the shot's settings
pub fn fit_camera_tone(
    rendered: &[f32],
    camera: &[f32],
    curve: &dyn Fn(i32) -> Result<Vec<u16>, String>,
) -> Result<(i32, f64, Vec<f32>), String> {
    let n = rendered.len() / 3;
    // a fixed pseudo-random subset (partial Fisher-Yates, xorshift64)
    let mut idx: Vec<u32> = (0..n as u32).collect();
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let take = SAMPLES.min(n);
    for k in 0..take {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let j = k + (s % (n - k) as u64) as usize;
        idx.swap(k, j);
    }
    let mut xs: Vec<[f64; 3]> = vec![];
    let mut ys: Vec<[f64; 3]> = vec![];
    for &i in &idx[..take] {
        let i = i as usize;
        let y = [camera[3 * i] as f64, camera[3 * i + 1] as f64, camera[3 * i + 2] as f64];
        if y.iter().cloned().fold(f64::MIN, f64::max) < 0.98 {
            xs.push([rendered[3 * i] as f64, rendered[3 * i + 1] as f64, rendered[3 * i + 2] as f64]);
            ys.push(y);
        }
    }
    if xs.is_empty() {
        return Err("camera JPEG is blown out".into());
    }
    let count = (xs.len() * 3) as f64;
    let scales = geomspace(800.0, 40000.0, 50);
    let mut best: Option<(f64, i32, f64)> = None;
    for g in geomspace(0x600 as f64, 0x8000 as f64, 20) {
        let gain = g as i32;
        let lut: Vec<f64> = curve(gain)?.iter().map(|&v| v as f64).collect();
        for &scale in &scales {
            let mut e = 0.0;
            for (x, y) in xs.iter().zip(&ys) {
                for c in 0..3 {
                    e += (apply_lut(&lut, x[c] * scale) - y[c]).abs();
                }
            }
            let e = e / count;
            if best.is_none_or(|b| e < b.0) {
                best = Some((e, gain, scale));
            }
        }
    }
    let (_, gain, scale) = best.unwrap();

    let lut: Vec<f64> = curve(gain)?.iter().map(|&v| v as f64).collect();
    let mut g = Vec::with_capacity(xs.len() * 3);
    let mut y = Vec::with_capacity(xs.len() * 3);
    for (xp, yp) in xs.iter().zip(&ys) {
        for c in 0..3 {
            g.push(apply_lut(&lut, xp[c] * scale));
            y.push(yp[c]);
        }
    }
    Ok((gain, scale, residual_curve(&g, &y)))
}

/// camera value over curve output: the median per bin, as an offset from the
/// identity that fades to 0 outside the data, smoothed and kept rising. Raw
/// medians step with the JPEG's 8 bits and jump between sparse bins, which
/// showed as banding; flat runs posterized highlights.
pub fn residual_curve(g: &[f64], y: &[f64]) -> Vec<f32> {
    let n = RESIDUAL_SIZE;
    let top = (n - 1) as f64;
    let mut bins: Vec<Vec<f64>> = vec![vec![]; n];
    for (&gv, &yv) in g.iter().zip(y) {
        bins[((gv * top) as usize).min(n - 1)].push(yv);
    }
    let (mut known, mut med, mut weight) = (vec![], vec![], vec![]);
    for (i, b) in bins.iter_mut().enumerate() {
        if b.len() >= RESIDUAL_MIN_COUNT {
            known.push(i as f64);
            weight.push((b.len() as f64).sqrt());
            med.push(median(b));
        }
    }
    if known.len() < 8 {
        return crate::tone::linspace(n).iter().map(|&v| v as f32).collect();
    }
    let mut kx = known.clone();
    let mut ky: Vec<f64> = known.iter().zip(&med).map(|(k, m)| m - k / top).collect();
    if kx[0] > 0.0 {
        kx.insert(0, 0.0);
        ky.insert(0, 0.0);
    }
    if kx[kx.len() - 1] < top {
        kx.push(top);
        ky.push(0.0);
    }
    let (first, last) = (known[0], known[known.len() - 1]);
    let mean_weight = weight.iter().sum::<f64>() / weight.len() as f64;
    let d: Vec<f64> = (0..n).map(|i| crate::tone::interp(i as f64, &kx, &ky)).collect();
    let w: Vec<f64> = (0..n).map(|i| {
        let x = i as f64;
        if x < first || x > last { mean_weight } else { crate::tone::interp(x, &known, &weight) }
    }).collect();
    let mut r: Vec<f64> = (0..n).map(|i| {
        let (mut num, mut den) = (0.0, 0.0);
        for j in 0..n {
            let t = (i as f64 - j as f64) / RESIDUAL_SIGMA;
            let k = (-0.5 * t * t).exp() * w[j];
            num += k * d[j];
            den += k;
        }
        i as f64 / top + num / den
    }).collect();
    for i in 1..n {
        r[i] = r[i].max(r[i - 1] + RESIDUAL_MIN_STEP);
    }
    r.iter().map(|&v| v.clamp(0.0, 1.0) as f32).collect()
}

pub fn tone_params(params: &[(&str, i32)], gain: i32) -> ToneParams {
    let get = |k: &str| params.iter().find(|p| p.0 == k).map(|p| p.1).unwrap_or(0);
    ToneParams { key: get("key"), contrast: get("contrast"), highlight: get("highlight"),
                 shadow: get("shadow"), gain, flag: 0 }
}
