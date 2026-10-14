//! The two numbers that tie the linear render to the camera's tone curve
//! (input scale and GammaTuning gain), fitted against the camera JPEG, and a
//! residual curve for what a global tone curve cannot explain -- the camera's
//! image-dependent shadow correction (ported from the earlier Python editor).
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

/// Where the sensor clips, white balance turns "unknown, very bright" into a
/// strong colour (the red gain is often ~3): clipped lights came out yellow
/// or pink. The camera renders them white, so all channels are cut at the
/// level of the channel with the smallest gain, as it clips first.
pub fn highlight_clip(wb: &[f32; 3]) -> f32 {
    wb.iter().cloned().fold(f32::INFINITY, f32::min)
}

/// the matrix part of the pipeline, as ColorMatrixTuning computes it
pub fn render_linear(lin: &[f32], wb: [f64; 3], sensor_ccm: &[f64], look: &[f64], mono_mix: bool) -> Vec<f32> {
    let mut m = [0f32; 9];
    for r in 0..3 {
        for c in 0..3 {
            m[3 * r + c] = (0..3).map(|k| look[3 * r + k] as f32 * sensor_ccm[3 * k + c] as f32).sum();
        }
    }
    let wb = wb.map(|v| v as f32);
    let clip = highlight_clip(&wb);
    let mut out = vec![0f32; lin.len()];
    for (o, p) in out.chunks_exact_mut(3).zip(lin.chunks_exact(3)) {
        let q = [(p[0] * wb[0]).min(clip), (p[1] * wb[1]).min(clip), (p[2] * wb[2]).min(clip)];
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
/// the tone curve's 12-bit input for a rendered pixel: scaled, then the
/// camera's shadow correction if any (tonecontrol.rs)
#[inline]
pub fn tone_input(x: [f64; 3], scale: f64, tc: Option<&[f32]>) -> [f64; 3] {
    let v = [x[0] * scale, x[1] * scale, x[2] * scale];
    match tc {
        Some(t) => crate::tonecontrol::apply(t, v),
        None => v,
    }
}

pub fn fit_camera_tone(
    rendered: &[f32],
    camera: &[f32],
    curve: &dyn Fn(i32) -> Result<Vec<u16>, String>,
    tc: Option<&[f32]>,
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
    // the residual sees every pixel: leaving out the ones the camera clipped
    // kept only the darker ones near white, and white came out grey
    let (mut xa, mut ya): (Vec<[f64; 3]>, Vec<[f64; 3]>) = (vec![], vec![]);
    for &i in &idx[..take] {
        let i = i as usize;
        let y = [camera[3 * i] as f64, camera[3 * i + 1] as f64, camera[3 * i + 2] as f64];
        let x = [rendered[3 * i] as f64, rendered[3 * i + 1] as f64, rendered[3 * i + 2] as f64];
        if y.iter().cloned().fold(f64::MIN, f64::max) < 0.98 {
            xs.push(x);
            ys.push(y);
        }
        xa.push(x);
        ya.push(y);
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
                let v = tone_input(*x, scale, tc);
                for c in 0..3 {
                    e += (apply_lut(&lut, v[c]) - y[c]).abs();
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
    for (xp, yp) in xa.iter().zip(&ya) {
        let v = tone_input(*xp, scale, tc);
        for c in 0..3 {
            g.push(apply_lut(&lut, v[c]));
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
    // above the data the correction fades out, so white stays white: the
    // smoothing alone left 1.0 at ~0.97, and clipped lights came out grey
    for i in (last as usize + 1)..n {
        let t = (i as f64 - last) / (top - last);
        r[i] = r[i] * (1.0 - t) + i as f64 / top * t;
    }
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

// ---- the camera's lens shading (vignetting) correction ------------------------------

/// Radial gain the camera applies to the linear image to correct the lens's
/// vignetting: 1 + a r² + b r⁴ + c r⁶, with r the distance from the centre
/// over half the diagonal. The DNG carries no data for it (no opcodes, no
/// makernote tag), so it is fitted per shot against the camera JPEG: that
/// covers every aperture. At f/2.8 the corners get about ×1.7.
pub type Vignette = [f64; 3];

/// mean of the fits on three Standard shots at f/2.8 (MR024661, MR024669,
/// MR024685: ×1.14 at half the radius, ×1.43 at 0.8); used when the shot's
/// own image control changes the edges itself (Retro, HDR Tone) or the fit
/// finds too little. Other apertures correct less; only f/2.8 is measured.
pub const DEFAULT_VIGNETTE: Vignette = [0.489, 0.333, -0.052];

pub fn vignette_gain(v: &Vignette, r2: f64) -> f64 {
    1.0 + r2 * (v[0] + r2 * (v[1] + r2 * v[2]))
}

/// squared normalised radius of pixel (x, y) in a w x h image
fn r2_at(x: usize, y: usize, w: usize, h: usize) -> f64 {
    let (dx, dy) = (x as f64 + 0.5 - w as f64 / 2.0, y as f64 + 0.5 - h as f64 / 2.0);
    (dx * dx + dy * dy) / ((w * w + h * h) as f64 / 4.0)
}

/// `rgb` (w x h x 3, linear) with the correction applied
pub fn apply_vignette(rgb: &[f32], w: usize, h: usize, v: &Vignette) -> Vec<f32> {
    let mut out = rgb.to_vec();
    for y in 0..h {
        for x in 0..w {
            let g = vignette_gain(v, r2_at(x, y, w, h)) as f32;
            for c in 0..3 {
                out[3 * (y * w + x) + c] *= g;
            }
        }
    }
    out
}

/// the correction that brings `rendered` (linear, w x h) closest to the
/// camera JPEG, given the shot's tone curve and scale: the JPEG goes back
/// through the inverted curve, and the ratio camera / render per radius is
/// fitted with the polynomial. None if the shot does not show enough of it.
pub fn fit_vignette(rendered: &[f32], camera: &[f32], w: usize, h: usize, lut: &[f64], scale: f64,
                    tc: Option<&[f32]>) -> Option<Vignette> {
    // the curve on its 12-bit input, made non-decreasing, to invert it
    let mut f = vec![0.0f64; 4096];
    for (x, v) in f.iter_mut().enumerate() {
        *v = apply_lut(lut, x as f64);
    }
    for x in 1..4096 {
        f[x] = f[x].max(f[x - 1]);
    }
    let invert = |y: f64| -> f64 {
        let i = f.partition_point(|&v| v < y).clamp(1, 4095);
        let (a, b) = (f[i - 1], f[i]);
        (i - 1) as f64 + if b > a { (y - a) / (b - a) } else { 0.0 }
    };
    const BINS: usize = 20;
    let mut bins: Vec<Vec<f64>> = vec![vec![]; BINS];
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let r2 = r2_at(x, y, w, h);
            let b = ((r2.sqrt() * BINS as f64) as usize).min(BINS - 1);
            let i = 3 * (y * w + x);
            let v = tone_input([rendered[i] as f64, rendered[i + 1] as f64, rendered[i + 2] as f64], scale, tc);
            for c in 0..3 {
                let cam = camera[i + c] as f64;
                let ren = v[c];
                // mid tones only: no clipping, no shadow correction
                if (0.15..0.85).contains(&cam) && (40.0..4000.0).contains(&ren) {
                    bins[b].push(invert(cam) / ren);
                }
            }
        }
    }
    // weighted least squares of k (1 + a r² + b r⁴ + c r⁶) over the bin medians
    let mut ata = [[0.0f64; 4]; 4];
    let mut atb = [0.0f64; 4];
    let mut used = 0;
    for (k, b) in bins.iter_mut().enumerate() {
        if b.len() < 200 {
            continue;
        }
        used += 1;
        let m = median(b);
        let r2 = ((k as f64 + 0.5) / BINS as f64).powi(2);
        let row = [1.0, r2, r2 * r2, r2 * r2 * r2];
        let wgt = (b.len() as f64).sqrt();
        for i in 0..4 {
            atb[i] += wgt * row[i] * m;
            for j in 0..4 {
                ata[i][j] += wgt * row[i] * row[j];
            }
        }
    }
    if used < 8 {
        return None;
    }
    let s = solve4(ata, atb)?;
    let v = [s[1] / s[0], s[2] / s[0], s[3] / s[0]];
    // plausible: a gain that rises from the centre and stays below 4
    let corner = vignette_gain(&v, 1.0);
    (s[0] > 0.0 && (0.9..4.0).contains(&corner) && (0..=10).all(|k| vignette_gain(&v, k as f64 / 10.0) > 0.8)).then_some(v)
}

fn solve4(mut a: [[f64; 4]; 4], mut b: [f64; 4]) -> Option<[f64; 4]> {
    for col in 0..4 {
        let p = (col..4).max_by(|&i, &j| a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap())?;
        if a[p][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, p);
        b.swap(col, p);
        for r in 0..4 {
            if r != col {
                let f = a[r][col] / a[col][col];
                for c in 0..4 {
                    a[r][c] -= f * a[col][c];
                }
                b[r] -= f * b[col];
            }
        }
    }
    Some([b[0] / a[0][0], b[1] / a[1][1], b[2] / a[2][2], b[3] / a[3][3]])
}

// ---- the camera's chroma roll-off in the highlights --------------------------------

/// The camera takes saturation out of bright colours: measured on MR022009,
/// MR024661 and MR024685, chroma falls from about Y 180 (of 255) on, to
/// ×0.55 near white. Not modelled from firmware data (the CSP block's chroma
/// limits are a candidate); fitted per shot against the camera JPEG like the
/// residual, this table is the typical case. One factor per 16 levels of Y.
pub const CHROMA_BINS: usize = 16;
pub const DEFAULT_HIGHLIGHT_CHROMA: [f32; CHROMA_BINS] =
    [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.92, 0.83, 0.72, 0.62, 0.55];

fn ycc601(c: [f64; 3]) -> (f64, f64) {
    let y = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
    let cb = -0.1687 * c[0] - 0.3313 * c[1] + 0.5 * c[2];
    let cr = 0.5 * c[0] - 0.4187 * c[1] - 0.0813 * c[2];
    (y, cb.hypot(cr))
}

/// camera chroma over preview chroma per luma bin of the preview (after tone
/// curve and residual), relative to the mid tones, from Y 160 on; below 1.0.
/// Bins without enough data keep the typical value.
pub fn fit_highlight_chroma(rendered: &[f32], camera: &[f32], lut: &[f64], scale: f64, residual: &[f32],
                            tc: Option<&[f32]>) -> [f32; CHROMA_BINS] {
    let res = |v: f64| -> f64 {
        let f = v.clamp(0.0, 1.0) * (residual.len() - 1) as f64;
        let i = (f as usize).min(residual.len() - 2);
        residual[i] as f64 * (1.0 - (f - i as f64)) + residual[i + 1] as f64 * (f - i as f64)
    };
    let mut bins: Vec<Vec<f64>> = vec![vec![]; CHROMA_BINS];
    for (r, c) in rendered.chunks_exact(3).zip(camera.chunks_exact(3)).step_by(3) {
        let v = tone_input([r[0] as f64, r[1] as f64, r[2] as f64], scale, tc);
        let p = [0, 1, 2].map(|k| res(apply_lut(lut, v[k])));
        let (yp, cp) = ycc601(p);
        let (_, cq) = ycc601([c[0] as f64, c[1] as f64, c[2] as f64]);
        if cp < 8.0 / 255.0 {
            continue;
        }
        bins[((yp * CHROMA_BINS as f64) as usize).min(CHROMA_BINS - 1)].push(cq / cp);
    }
    let med: Vec<Option<f64>> = bins.iter_mut().map(|b| (b.len() >= 60).then(|| median(b))).collect();
    // the mid tones' ratio is the look's saturation, not the roll-off
    let mut mid: Vec<f64> = med[4..10].iter().flatten().cloned().collect();
    if mid.len() < 3 {
        return DEFAULT_HIGHLIGHT_CHROMA;
    }
    let base = median(&mut mid);
    let mut out = DEFAULT_HIGHLIGHT_CHROMA;
    for k in 10..CHROMA_BINS {
        if let Some(m) = med[k] {
            out[k] = (m / base).clamp(0.1, 1.0) as f32;
        }
    }
    for k in 0..10 {
        out[k] = 1.0;
    }
    // never more saturated towards white
    for k in 11..CHROMA_BINS {
        out[k] = out[k].min(out[k - 1]);
    }
    out
}
