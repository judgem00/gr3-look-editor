//! MultiAxial, the hue-selective colour stage: per-hue adjustments (HSL) for
//! an image control (docs/multiaxial.md).
//!
//! The stage sits behind the colour matrix (CC0), on linear camera RGB before
//! the tone curve. Per look it reads a parameter set from 0x55018cf0[index]:
//!
//!     [0] 16 x u16   axes: 4 descending bounds per quadrant
//!     [1] 20 x u8    nibbles: bank for each of the 5 segments of a quadrant
//!     [2] header     +0x10 -> 60 matrices (12 banks x 5 rows), 3x3 i32, 1.0 = 1024
//!     [3] header     +0x10 -> 360 region values (no effect, left as they are)
//!     [4] 21 x u16   control fields, 10..20 = weight curve over luminance
//!     [5] 9 x i16    RGB -> YCbCr, 1.0 = 256
//!
//! Every look shares one stock set that does nothing. A look with HSL gets a
//! set of its own in unused read-only padding.
//!
//! Measured on the GR III (test shots MR024653-76):
//! - Cb/Cr is split into quadrants, each with a coordinate 0..2048 that is
//!   2048 * b / (a + b), a and b the chroma components along the quadrant's
//!   first and second axis. Going round: Q0 +Cr (red) -> +Cb (blue), Q3 -> -Cr
//!   (green), Q2 -> -Cb (yellow), Q1 -> +Cr. This module joins them into one
//!   hue coordinate 0..8192 in that order.
//! - Nibble n of a quadrant with bounds a >= b >= c >= d selects the bank for
//!   x >= a (n = 0), [b, a), [c, b), [d, c), x < d (n = 4).
//! - Only row 0 of a bank acts; all five rows get the same matrix.
//! - Edges are hard. Soft edges need a step in between ("feather").
//! - The weight is a piecewise linear curve over x = 4096 * Y of the stage's
//!   input (linear RGB, white = 1.0): breakpoints = fields 18..20, the weight
//!   at the start of each segment = fields 10..13 (1.0 = 0x400), slopes =
//!   fields 14..17 (9 bit signed, per 32 units of x).

use crate::addr::{file_offset, i16_at, put_u16, put_u32, word};
use serde_json::{json, Value};

pub const SET_TABLE: u32 = 0x5501_8cf0;
pub const BANKS: usize = 12;
const ROWS: usize = 5;
pub const QUADRANT: i32 = 2048;
pub const CIRCLE: i32 = 4 * QUADRANT;
const ONE: f64 = 1024.0;
pub const CONTROL_FIELDS: usize = 21;
/// edges this close to a quadrant border (about 1 degree) move onto it
const SNAP: f64 = 24.0;

/// quadrant index at each quarter of the hue coordinate
const QUADRANT_AT: [usize; 4] = [0, 3, 2, 1];

/// one hue range with its adjustment
#[derive(Clone, Debug, Default)]
pub struct Band {
    /// centre and width on the hue coordinate (0..8192)
    pub center: f64,
    pub width: f64,
    /// width of the half-strength step on either side, 0 = hard edge
    pub feather: f64,
    /// hue rotation in degrees of Cb/Cr; positive = towards the next colour
    /// in the order red -> yellow -> green -> blue (as in Lightroom)
    pub hue: f64,
    /// saturation and luminance, -100..100
    pub sat: f64,
    pub lum: f64,
}

/// Hue vs Hue / Sat / Lum curves (as in DaVinci Resolve): points (hue
/// coordinate 0..8192, value) on a closed circle. Values: hue in degrees,
/// saturation and luminance -100..100 (0 = unchanged).
#[derive(Clone, Debug, Default)]
pub struct Curves {
    pub hue: Vec<[f64; 2]>,
    pub sat: Vec<[f64; 2]>,
    pub lum: Vec<[f64; 2]>,
}

/// HSL of one look: curves, or (older presets) bands
#[derive(Clone, Debug)]
pub struct Hsl {
    pub curves: Option<Curves>,
    /// a base under the curves (a fitted film or LUT, lutfit.rs): combined
    /// with them before encoding -- hue and luminance add, saturation
    /// multiplies -- so the user's curves stay their own
    pub base: Option<Curves>,
    pub bands: Vec<Band>,
    /// weight curve: full weight above `shadows`, fading to 0 at x = 0
    /// (0 = no protection), and from `highlights` on fading to
    /// `highlight_weight` at x = 4096
    pub shadows: f64,
    pub highlights: f64,
    pub highlight_weight: f64,
}

impl Default for Hsl {
    fn default() -> Self {
        Hsl { curves: None, base: None, bands: vec![], shadows: 0.0, highlights: 4096.0, highlight_weight: 1.0 }
    }
}

fn num(v: &Value, k: &str, default: f64) -> f64 {
    v.get(k).and_then(|x| x.as_f64()).filter(|x| x.is_finite()).unwrap_or(default)
}

impl Hsl {
    pub fn from_json(v: &Value) -> Hsl {
        let bands = v.get("bands").and_then(|b| b.as_array()).map(|a| {
            a.iter().map(|b| Band {
                center: num(b, "center", 0.0).rem_euclid(CIRCLE as f64),
                width: num(b, "width", 600.0).clamp(0.0, CIRCLE as f64),
                feather: num(b, "feather", 0.0).clamp(0.0, QUADRANT as f64),
                hue: num(b, "hue", 0.0).clamp(-180.0, 180.0),
                sat: num(b, "sat", 0.0).clamp(-100.0, 100.0),
                lum: num(b, "lum", 0.0).clamp(-100.0, 100.0),
            }).collect()
        }).unwrap_or_default();
        let curve = |set: &str, k: &str, lim: f64| -> Vec<[f64; 2]> {
            v.get(set).and_then(|c| c.get(k)).and_then(|a| a.as_array()).map(|a| {
                a.iter().filter_map(|p| {
                    let (x, y) = (p.get(0)?.as_f64()?, p.get(1)?.as_f64()?);
                    (x.is_finite() && y.is_finite()).then(|| [x.rem_euclid(CIRCLE as f64), y.clamp(-lim, lim)])
                }).collect()
            }).unwrap_or_default()
        };
        let set = |name: &str| v.get(name).filter(|c| c.is_object()).map(|_| Curves {
            hue: curve(name, "hue", HUE_MAX), sat: curve(name, "sat", 100.0), lum: curve(name, "lum", 100.0),
        });
        let (curves, base) = (set("curves"), set("base"));
        let p = v.get("protect").cloned().unwrap_or(Value::Null);
        Hsl {
            curves,
            base,
            bands,
            shadows: num(&p, "shadows", 0.0).clamp(0.0, 4096.0),
            highlights: num(&p, "highlights", 4096.0).clamp(0.0, 4096.0),
            highlight_weight: num(&p, "highlight_weight", 1.0).clamp(0.0, 1.0),
        }
    }

    pub fn is_neutral(&self) -> bool {
        let flat = |c: &[[f64; 2]]| c.iter().all(|p| p[1] == 0.0);
        let neutral = |c: &Option<Curves>| c.as_ref().is_none_or(|c| flat(&c.hue) && flat(&c.sat) && flat(&c.lum));
        self.bands.iter().all(|b| b.hue == 0.0 && b.sat == 0.0 && b.lum == 0.0) && neutral(&self.curves) && neutral(&self.base)
    }
}

/// what the hardware gets, and the preview with it
#[derive(Clone, Debug)]
pub struct Encoded {
    /// 4 descending bounds per quadrant, quadrant order Q0..Q3
    pub axes: [[u16; 4]; 4],
    pub nibbles: [u8; 20],
    /// 12 banks, 3x3 at scale 1024
    pub banks: Vec<[i32; 9]>,
    pub control: [u16; CONTROL_FIELDS],
    pub banks_used: usize,
    /// what the encoder had to simplify
    pub notes: Vec<String>,
    /// for curves: the smooth curves and the steps the camera gets, per bin
    pub curves: Option<Value>,
}

type M3 = [f64; 9];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut o = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            o[3 * r + c] = (0..3).map(|k| a[3 * r + k] * b[3 * k + c]).sum();
        }
    }
    o
}

fn inverse(m: &M3) -> M3 {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det,
     (f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det,
     (d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det]
}

/// direction (Cb, Cr) of a point on the hue coordinate
pub fn direction(g: f64) -> (f64, f64) {
    let g = g.rem_euclid(CIRCLE as f64);
    let quarter = (g / QUADRANT as f64).floor().min(3.0) as usize;
    let r = (g - quarter as f64 * QUADRANT as f64) / QUADRANT as f64;
    // a along the first axis, b along the second, a + b = 1
    let (a, b) = (1.0 - r, r);
    let (cb, cr) = match QUADRANT_AT[quarter] {
        0 => (b, a),     // +Cr -> +Cb
        3 => (a, -b),    // +Cb -> -Cr
        2 => (-b, -a),   // -Cr -> -Cb
        _ => (-a, b),    // -Cb -> +Cr
    };
    let n = cb.hypot(cr);
    (cb / n, cr / n)
}

/// hue coordinate (0..8192) of a Cb/Cr pair, as the hardware sorts it
pub fn coordinate(cb: f64, cr: f64) -> f64 {
    let (quarter, a, b) = if cr >= 0.0 {
        if cb >= 0.0 { (0, cr, cb) } else { (3, -cb, cr) }
    } else if cb >= 0.0 {
        (1, cb, -cr)
    } else {
        (2, -cr, -cb)
    };
    let x = if a + b > 0.0 { QUADRANT as f64 * b / (a + b) } else { 0.0 };
    quarter as f64 * QUADRANT as f64 + x
}

/// the adjustment of a band as a matrix in linear RGB (1.0 = 1.0): rotation
/// and scaling in the Cb/Cr plane of `ycc`, and luminance moved along the
/// band's own direction, so grey stays grey
pub fn band_matrix(ycc: &M3, b: &Band, strength: f64) -> M3 {
    let (dcb, dcr) = direction(b.center);
    // positive hue goes towards lower coordinates (red -> yellow -> green)
    let th = -(b.hue * strength).to_radians();
    let s = 1.0 + b.sat * strength / 100.0;
    let k = b.lum * strength / 100.0;
    let (c, sn) = (th.cos(), th.sin());
    // Cb/Cr rotation in the direction of increasing coordinate (Cr -> Cb)
    let inner: M3 = [
        1.0, k * dcb, k * dcr,
        0.0, s * c, s * sn,
        0.0, -s * sn, s * c,
    ];
    mul(&inverse(ycc), &mul(&inner, ycc))
}

fn quantise(m: &M3) -> Result<[i32; 9], String> {
    let mut o = [0i32; 9];
    for (k, v) in m.iter().enumerate() {
        let q = (v * ONE).round();
        if !(-2048.0..=2047.0).contains(&q) {
            return Err("adjustment too strong: a matrix value leaves -2..+2".into());
        }
        o[k] = q as i32;
    }
    // rows that sum to 1.0 keep summing to exactly 1024, so grey stays grey
    for r in 0..3 {
        let want = ((m[3 * r] + m[3 * r + 1] + m[3 * r + 2]) * ONE).round() as i32;
        o[4 * r] += want - (o[3 * r] + o[3 * r + 1] + o[3 * r + 2]);
    }
    Ok(o)
}

const IDENTITY: [i32; 9] = [1024, 0, 0, 0, 1024, 0, 0, 0, 1024];

/// the 21 control fields for full strength with the weight curve of `h`
pub fn control(h: &Hsl) -> [u16; CONTROL_FIELDS] {
    let mut f = [0u16; CONTROL_FIELDS];
    for v in &mut f[1..=8] {
        *v = 0xFFF;
    }
    // segments 0..3 start at b0 = 0, b1, b2, b3
    let slope = |from: f64, to: f64, len: f64| -> i32 {
        if len <= 0.0 { 0 } else { ((to - from) * ONE * 32.0 / len).round().clamp(-256.0, 255.0) as i32 }
    };
    let s = (h.shadows.round() as i32).clamp(0, 4095);
    let hi = (h.highlights.round() as i32).clamp(s, 4096);
    let (start, slopes, bps): ([f64; 4], [i32; 4], [i32; 3]) = (
        [0.0, 1.0, 1.0, 1.0],
        [slope(0.0, 1.0, s as f64), 0, 0, slope(1.0, h.highlight_weight, (4096 - hi) as f64)],
        [s, hi.min(4095), hi.min(4095)],
    );
    for k in 0..4 {
        f[10 + k] = (start[k] * ONE).round() as u16;
        f[14 + k] = (slopes[k] & 0x1FF) as u16;
    }
    for k in 0..3 {
        f[18 + k] = bps[k] as u16;
    }
    f
}

/// weight (0..1) the control fields give at x = 4096 * Y
pub fn weight(control: &[u16; CONTROL_FIELDS], x: f64) -> f64 {
    let bp = [0.0, control[18] as f64, control[19] as f64, control[20] as f64];
    let seg = (0..4).rev().find(|&k| x >= bp[k]).unwrap_or(0);
    let raw = control[14 + seg] as i32;
    let slope = if raw & 0x100 != 0 { raw - 0x200 } else { raw };
    let w = control[10 + seg] as f64 + slope as f64 * (x - bp[seg]) / 32.0;
    (w / ONE).clamp(0.0, 1.0)
}

const QUADRANT_NAMES: [&str; 4] = ["red - blue", "yellow - red", "green - yellow", "blue - green"];

/// which bands cover each hue coordinate: (band, true = on its soft edge).
/// Soft edges are left out for bands where `feather[i]` is false.
fn coverage(h: &Hsl, feather: &[bool]) -> Vec<Vec<(usize, bool)>> {
    let mut lane: Vec<Vec<(usize, bool)>> = vec![vec![]; CIRCLE as usize];
    // an edge a few units off a quadrant border moves onto it: one bound less
    let snap = |g: f64| {
        let border = (g / QUADRANT as f64).round() * QUADRANT as f64;
        if (g - border).abs() <= SNAP { border } else { g }
    };
    let mut fill = |from: f64, to: f64, band: usize, edge: bool| {
        let (a, b) = (snap(from).round() as i64, snap(to).round() as i64);
        for g in a..b {
            let cell = &mut lane[g.rem_euclid(CIRCLE as i64) as usize];
            // the core wins over the band's own soft edge where they wrap onto each other
            match cell.iter_mut().find(|c| c.0 == band) {
                Some(c) => c.1 &= edge,
                None => cell.push((band, edge)),
            }
        }
    };
    for (i, b) in h.bands.iter().enumerate() {
        if b.width <= 0.0 {
            continue;
        }
        let (lo, hi) = (b.center - b.width / 2.0, b.center + b.width / 2.0);
        fill(lo, hi, i, false);
        if b.feather > 0.0 && feather[i] {
            fill(lo - b.feather, lo, i, true);
            fill(hi, hi + b.feather, i, true);
        }
    }
    for cell in &mut lane {
        cell.sort();
    }
    lane
}

/// the matrix where several bands meet: each band adds its change (half of it
/// on a soft edge); where the weights sum to more than 1 they are averaged.
/// So the result does not depend on the order of the bands, and two bands
/// that overlap or touch with soft edges blend instead of cutting each other.
fn blend(cell: &[(usize, bool)], full: &[M3]) -> M3 {
    let w: f64 = cell.iter().map(|c| if c.1 { 0.5 } else { 1.0 }).sum();
    let norm = if w > 1.0 { w } else { 1.0 };
    let id: M3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let mut m = id;
    for &(b, edge) in cell {
        let k = if edge { 0.5 } else { 1.0 } / norm;
        for j in 0..9 {
            m[j] += k * (full[b][j] - id[j]);
        }
    }
    m
}

/// hue coordinate -> bank
fn paint(cover: &[Vec<(usize, bool)>], full: &[M3], banks: &mut Vec<[i32; 9]>) -> Result<Vec<u8>, String> {
    let mut known: std::collections::HashMap<&[(usize, bool)], u8> = std::collections::HashMap::new();
    let mut lane = vec![0u8; cover.len()];
    for (g, cell) in cover.iter().enumerate() {
        if cell.is_empty() {
            continue;
        }
        if let Some(&b) = known.get(cell.as_slice()) {
            lane[g] = b;
            continue;
        }
        let m = quantise(&blend(cell, full))?;
        let bank = match banks.iter().position(|b| *b == m) {
            Some(i) => i as u8,
            None if banks.len() >= BANKS => {
                return Err(format!("too many different adjustments: the stage has {BANKS} banks, one stays unchanged"));
            }
            None => {
                banks.push(m);
                (banks.len() - 1) as u8
            }
        };
        known.insert(cell.as_slice(), bank);
        lane[g] = bank;
    }
    Ok(lane)
}

/// runs (start, bank) of one quarter of the lane, highest start first
fn runs(lane: &[u8], quarter: usize) -> Vec<(usize, u8)> {
    let seg = &lane[quarter * QUADRANT as usize..(quarter + 1) * QUADRANT as usize];
    let mut runs: Vec<(usize, u8)> = vec![];
    for (x, &bank) in seg.iter().enumerate() {
        if runs.last().map_or(true, |r| r.1 != bank) {
            runs.push((x, bank));
        }
    }
    runs.reverse();
    runs
}

/// Bands -> what the hardware gets. Where a quadrant would need more than 4
/// edges (or the stage more than 12 banks), the soft edges there are dropped
/// first, and `notes` says so; only if that is not enough it fails.
pub fn encode(h: &Hsl, ycc: &M3) -> Result<Encoded, String> {
    if h.curves.is_some() || h.base.is_some() {
        return encode_curves(h, h.curves.as_ref().unwrap_or(&Curves::default()), ycc);
    }
    let full: Vec<M3> = h.bands.iter().map(|b| band_matrix(ycc, b, 1.0)).collect();
    let mut feather = vec![true; h.bands.len()];
    let mut notes = vec![];
    loop {
        let cover = coverage(h, &feather);
        let mut banks = vec![IDENTITY];
        let painted = paint(&cover, &full, &mut banks);
        // bands whose soft edges lie in the given quarters
        let soft_in = |quarters: &[usize]| -> Vec<usize> {
            let mut v: Vec<usize> = quarters.iter()
                .flat_map(|&k| cover[k * QUADRANT as usize..(k + 1) * QUADRANT as usize].iter())
                .flat_map(|cell| cell.iter().filter(|c| c.1).map(|c| c.0))
                .collect();
            v.sort();
            v.dedup();
            v
        };
        let lane = match painted {
            Ok(l) => l,
            Err(e) => {
                let drop = soft_in(&[0, 1, 2, 3]);
                if drop.is_empty() {
                    return Err(e);
                }
                for &i in &drop {
                    feather[i] = false;
                }
                notes.push("soft edges dropped: too many different adjustments".to_string());
                continue;
            }
        };
        let over: Vec<usize> = (0..4).filter(|&k| runs(&lane, k).len() > 5).collect();
        if over.is_empty() {
            return Ok(finish(&lane, banks, h, notes, None));
        }
        let names: Vec<&str> = over.iter().map(|&k| QUADRANT_NAMES[QUADRANT_AT[k]]).collect();
        let drop = soft_in(&over);
        if drop.is_empty() {
            let k = over[0];
            return Err(format!("too many edges between {}: at most 4 per quadrant, here {}",
                               QUADRANT_NAMES[QUADRANT_AT[k]], runs(&lane, k).len() - 1));
        }
        for &i in &drop {
            feather[i] = false;
        }
        notes.push(format!("soft edges dropped between {}: at most 4 edges per quadrant", names.join(", ")));
    }
}

/// lane (bank per coordinate, at most 5 runs per quarter) -> Encoded
fn finish(lane: &[u8], mut banks: Vec<[i32; 9]>, h: &Hsl, notes: Vec<String>, curves: Option<Value>) -> Encoded {
    let mut axes = [[0u16; 4]; 4];
    let mut nibbles = [0u8; 20];
    for quarter in 0..4 {
        let q = QUADRANT_AT[quarter];
        for (n, &(start, bank)) in runs(lane, quarter).iter().enumerate() {
            if n < 4 && start > 0 {
                axes[q][n] = start as u16;
            }
            nibbles[5 * q + n] = bank;
        }
    }
    let used = banks.len();
    banks.resize(BANKS, IDENTITY);
    Encoded { axes, nibbles, banks, control: control(h), banks_used: used, notes, curves }
}

// ---- curves ----------------------------------------------------------------------

pub const HUE_MAX: f64 = 45.0;
/// curves are evaluated and split on bins of this many coordinate units
const BIN: usize = 16;
const BINS: usize = CIRCLE as usize / BIN;
const QBINS: usize = QUADRANT as usize / BIN;
/// value scale per channel for the fit: a hue degree counts like ~3 of saturation
const NORM: [f64; 3] = [1.0 / 30.0, 1.0 / 100.0, 1.0 / 100.0];
/// cost of one more edge in a quadrant (squared normalised error x bins)
const EDGE_COST: f64 = 0.02;

/// a closed curve through `points` on the circle, at the centres of `n` bins:
/// piecewise cubic, monotone between points (no overshoot)
pub fn sample_curve(points: &[[f64; 2]], n: usize) -> Vec<f64> {
    let c = CIRCLE as f64;
    let mut p: Vec<[f64; 2]> = points.to_vec();
    p.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
    p.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6);
    match p.len() {
        0 => return vec![0.0; n],
        1 => return vec![p[0][1]; n],
        _ => {}
    }
    let k = p.len() as isize;
    let at = |i: isize| -> (f64, f64) {
        let j = i.rem_euclid(k) as usize;
        (p[j][0] + i.div_euclid(k) as f64 * c, p[j][1])
    };
    let slope = |i: isize| -> f64 {
        let (x0, y0) = at(i - 1);
        let (x1, y1) = at(i);
        let (x2, y2) = at(i + 1);
        let (h0, h1) = (x1 - x0, x2 - x1);
        let (d0, d1) = ((y1 - y0) / h0, (y2 - y1) / h1);
        if d0 * d1 <= 0.0 { 0.0 } else { 3.0 * (h0 + h1) / ((2.0 * h1 + h0) / d0 + (h1 + 2.0 * h0) / d1) }
    };
    (0..n).map(|s| {
        let mut g = (s as f64 + 0.5) * c / n as f64;
        if g < p[0][0] {
            g += c;
        }
        let mut i = 0isize;
        while at(i + 1).0 <= g {
            i += 1;
        }
        let ((x0, y0), (x1, y1)) = (at(i), at(i + 1));
        let (h, t) = (x1 - x0, (g - x0) / (x1 - x0));
        let (m0, m1) = (slope(i) * h, slope(i + 1) * h);
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * m0 + (-2.0 * t3 + 3.0 * t2) * y1 + (t3 - t2) * m1
    }).collect()
}

/// best split of one quarter's bins into at most 5 steps (least squares,
/// each further edge must pay EDGE_COST): (start bin, end bin) per step
fn split(v: &[[f64; 3]]) -> Vec<(usize, usize)> {
    let n = v.len();
    let mut s1 = vec![[0.0; 3]; n + 1];
    let mut s2 = vec![0.0; n + 1];
    for i in 0..n {
        for c in 0..3 {
            s1[i + 1][c] = s1[i][c] + v[i][c];
        }
        s2[i + 1] = s2[i] + v[i].iter().map(|x| x * x).sum::<f64>();
    }
    let cost = |a: usize, b: usize| -> f64 {
        let len = (b - a) as f64;
        s2[b] - s2[a] - (0..3).map(|c| (s1[b][c] - s1[a][c]).powi(2)).sum::<f64>() / len
    };
    // best[k][j]: first j bins in k+1 steps
    let mut best = vec![vec![f64::INFINITY; n + 1]; 5];
    let mut from = vec![vec![0usize; n + 1]; 5];
    for j in 1..=n {
        best[0][j] = cost(0, j);
    }
    for k in 1..5 {
        for j in (k + 1)..=n {
            for i in k..j {
                let c = best[k - 1][i] + cost(i, j);
                if c < best[k][j] {
                    best[k][j] = c;
                    from[k][j] = i;
                }
            }
        }
    }
    let k = (0..5).min_by(|&a, &b| {
        (best[a][n] + EDGE_COST * a as f64).partial_cmp(&(best[b][n] + EDGE_COST * b as f64)).unwrap()
    }).unwrap();
    let mut out = vec![];
    let (mut k, mut j) = (k, n);
    loop {
        let i = if k == 0 { 0 } else { from[k][j] };
        out.push((i, j));
        if k == 0 {
            break;
        }
        k -= 1;
        j = i;
    }
    out.reverse();
    out
}

/// Curves -> what the hardware gets. Each quarter of the circle is split
/// into at most 5 steps that follow the curves best; similar steps are then
/// merged until at most 11 adjustments remain (bank 0 stays unchanged).
/// the user's curve values on top of a base: hue and luminance add,
/// saturation multiplies (both are scales of the chroma)
fn combine(b: [f64; 3], u: [f64; 3]) -> [f64; 3] {
    [b[0] + u[0], (((1.0 + b[1] / 100.0) * (1.0 + u[1] / 100.0) - 1.0) * 100.0).max(-100.0), b[2] + u[2]]
}

/// what is left of the user's part in a combined value
fn uncombine(c: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    let s = 1.0 + b[1] / 100.0;
    [c[0] - b[0], if s > 1e-6 { ((1.0 + c[1] / 100.0) / s - 1.0) * 100.0 } else { 0.0 }, c[2] - b[2]]
}

fn sample_set(c: &Curves) -> Vec<[f64; 3]> {
    let s = [sample_curve(&c.hue, BINS), sample_curve(&c.sat, BINS), sample_curve(&c.lum, BINS)];
    (0..BINS).map(|i| [s[0][i], s[1][i], s[2][i]]).collect()
}

fn encode_curves(h: &Hsl, c: &Curves, ycc: &M3) -> Result<Encoded, String> {
    let user = sample_set(c);
    let base = h.base.as_ref().map(sample_set);
    // the base's own steps: the user's part is drawn against them, so flat
    // curves over a base show flat steps
    let base_steps: Option<Vec<[f64; 3]>> = match &h.base {
        Some(bc) => {
            let alone = Hsl { curves: Some(bc.clone()), base: None, ..h.clone() };
            let e = encode_curves(&alone, bc, ycc)?;
            let st = &e.curves.as_ref().unwrap()["steps"];
            let ch = |k: &str| -> Vec<f64> { st[k].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect() };
            let (hu, sa, lu) = (ch("hue"), ch("sat"), ch("lum"));
            Some((0..BINS).map(|i| [hu[i], sa[i], lu[i]]).collect())
        }
        None => None,
    };
    let total: Vec<[f64; 3]> = match &base {
        Some(b) => (0..BINS).map(|i| combine(b[i], user[i])).collect(),
        None => user.clone(),
    };
    let smooth = [0, 1, 2].map(|k| user.iter().map(|v| v[k]).collect::<Vec<f64>>());
    let v: Vec<[f64; 3]> = total.iter().map(|t| [t[0] * NORM[0], t[1] * NORM[1], t[2] * NORM[2]]).collect();
    // steps: (first bin, end bin, value in normalised units)
    let mut steps: Vec<(usize, usize, [f64; 3])> = vec![];
    for quarter in 0..4 {
        let part = &v[quarter * QBINS..(quarter + 1) * QBINS];
        for (a, b) in split(part) {
            let len = (b - a) as f64;
            let mut m = [0.0; 3];
            for x in &part[a..b] {
                for k in 0..3 {
                    m[k] += x[k] / len;
                }
            }
            steps.push((quarter * QBINS + a, quarter * QBINS + b, m));
        }
    }
    // clusters of steps with one adjustment; None = unchanged (bank 0)
    let tiny = |m: &[f64; 3]| m[0].abs() < 0.3 * NORM[0] && m[1].abs() < 0.5 * NORM[1] && m[2].abs() < 0.5 * NORM[2];
    let mut cluster: Vec<Option<usize>> = vec![None; steps.len()];
    let mut centre: Vec<([f64; 3], f64)> = vec![];
    for (i, s) in steps.iter().enumerate() {
        if !tiny(&s.2) {
            cluster[i] = Some(centre.len());
            centre.push((s.2, (s.1 - s.0) as f64));
        }
    }
    let mut notes = vec![];
    let alive = |cluster: &[Option<usize>], k: usize| cluster.iter().any(|c| *c == Some(k));
    let mut merged = false;
    while (0..centre.len()).filter(|&k| alive(&cluster, k)).count() > BANKS - 1 {
        // Ward: merge the pair (or a cluster into "unchanged") that adds the least error
        let live: Vec<usize> = (0..centre.len()).filter(|&k| alive(&cluster, k)).collect();
        let d2 = |a: &[f64; 3], b: &[f64; 3]| (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>();
        let mut best = (f64::INFINITY, 0, None);
        for (x, &a) in live.iter().enumerate() {
            let (ca, wa) = centre[a];
            let to_zero = wa * d2(&ca, &[0.0; 3]);
            if to_zero < best.0 {
                best = (to_zero, a, None);
            }
            for &b in &live[x + 1..] {
                let (cb, wb) = centre[b];
                let cost = wa * wb / (wa + wb) * d2(&ca, &cb);
                if cost < best.0 {
                    best = (cost, a, Some(b));
                }
            }
        }
        let (_, a, b) = best;
        for cl in cluster.iter_mut() {
            if *cl == Some(a) {
                *cl = b;
            }
        }
        if let Some(b) = b {
            let ((ca, wa), (cb, wb)) = (centre[a], centre[b]);
            centre[b] = ([0, 1, 2].map(|k| (ca[k] * wa + cb[k] * wb) / (wa + wb)), wa + wb);
        }
        merged = true;
    }
    if merged {
        notes.push("similar steps merged: the stage has 11 adjustments".to_string());
    }
    // one matrix per cluster; brightness moves along the cluster's mean hue direction
    let mut banks = vec![IDENTITY];
    let mut bank_of_cluster = std::collections::HashMap::new();
    for i in 0..steps.len() {
        let Some(k) = cluster[i] else { continue };
        if bank_of_cluster.contains_key(&k) {
            continue;
        }
        let (mut cb, mut cr) = (0.0, 0.0);
        for (j, t) in steps.iter().enumerate() {
            if cluster[j] == Some(k) {
                let (b, r) = direction(((t.0 + t.1) as f64 / 2.0) * BIN as f64);
                let w = (t.1 - t.0) as f64;
                cb += w * b;
                cr += w * r;
            }
        }
        let val = centre[k].0;
        let band = Band {
            center: coordinate(cb, cr), hue: val[0] / NORM[0], sat: val[1] / NORM[1], lum: val[2] / NORM[2],
            ..Default::default()
        };
        let m = quantise(&band_matrix(ycc, &band, 1.0))?;
        let bank = match banks.iter().position(|b| *b == m) {
            Some(p) => p,
            None => {
                banks.push(m);
                banks.len() - 1
            }
        };
        bank_of_cluster.insert(k, bank);
    }
    let mut lane = vec![0u8; CIRCLE as usize];
    let mut stepped = vec![[0.0f64; 3]; BINS];
    for (i, s) in steps.iter().enumerate() {
        let (bank, val) = match cluster[i] {
            Some(k) => (bank_of_cluster[&k] as u8, centre[k].0),
            None => (0, [0.0; 3]),
        };
        for g in s.0 * BIN..s.1 * BIN {
            lane[g] = bank;
        }
        for b in s.0..s.1 {
            let got = [val[0] / NORM[0], val[1] / NORM[1], val[2] / NORM[2]];
            // the editor draws the user's part of what the camera gets
            stepped[b] = match &base_steps {
                Some(bs) => uncombine(got, bs[b]),
                None => got,
            };
        }
    }
    let curves = json!({
        "bin": BIN,
        "smooth": { "hue": smooth[0], "sat": smooth[1], "lum": smooth[2] },
        "steps": {
            "hue": stepped.iter().map(|s| s[0]).collect::<Vec<_>>(),
            "sat": stepped.iter().map(|s| s[1]).collect::<Vec<_>>(),
            "lum": stepped.iter().map(|s| s[2]).collect::<Vec<_>>(),
        },
    });
    Ok(finish(&lane, banks, h, notes, Some(curves)))
}

impl Encoded {
    /// bank index the hardware picks for a coordinate (0..8192)
    pub fn bank_at(&self, g: f64) -> usize {
        let g = g.rem_euclid(CIRCLE as f64);
        let quarter = ((g / QUADRANT as f64) as usize).min(3);
        let q = QUADRANT_AT[quarter];
        let x = g - (quarter as i32 * QUADRANT) as f64;
        let n = self.axes[q].iter().position(|&b| x >= b as f64).unwrap_or(4);
        self.nibbles[5 * q + n] as usize
    }

    /// the stage on one linear RGB pixel (white = 1.0)
    pub fn apply(&self, ycc: &M3, rgb: [f64; 3]) -> [f64; 3] {
        let row = |r: usize| ycc[3 * r] * rgb[0] + ycc[3 * r + 1] * rgb[1] + ycc[3 * r + 2] * rgb[2];
        let (y, cb, cr) = (row(0), row(1), row(2));
        let m = &self.banks[self.bank_at(coordinate(cb, cr))];
        let w = weight(&self.control, (4096.0 * y).clamp(0.0, 4095.0));
        let mut out = [0.0; 3];
        for r in 0..3 {
            let v: f64 = (0..3).map(|c| m[3 * r + c] as f64 / ONE * rgb[c]).sum();
            out[r] = (rgb[r] + w * (v - rgb[r])).max(0.0);
        }
        out
    }

    pub fn to_json(&self) -> Value {
        json!({
            "axes": self.axes, "nibbles": self.nibbles.to_vec(),
            "banks": self.banks.iter().map(|b| b.iter().map(|&v| v as f64 / ONE).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "control": self.control.to_vec(),
            "banks_used": self.banks_used,
            "notes": self.notes,
            "curves": self.curves,
        })
    }
}

/// the look's parameter set
pub fn param_set(img: &[u8], table: u32) -> u32 {
    word(img, SET_TABLE + 4 * table)
}

/// RGB -> YCbCr of the look's set, 1.0 = 1.0
pub fn ycc(img: &[u8], table: u32) -> M3 {
    let o = file_offset(word(img, param_set(img, table) + 20));
    let mut m = [0.0; 9];
    for (k, v) in m.iter_mut().enumerate() {
        *v = i16_at(img, o + 2 * k) as f64 / 256.0;
    }
    m
}

/// image with a parameter set of its own for each look in `looks`
/// ((table index, encoded HSL)) at the addresses `slots` (space::plan).
/// Returns the set addresses.
pub fn patch(img: &mut [u8], looks: &[(u32, Encoded)], slots: &[u32]) -> Result<Vec<u32>, String> {
    if looks.len() > slots.len() {
        return Err("no free slot for an HSL set".into());
    }
    let mut out = vec![];
    for (&(table, ref e), &at) in looks.iter().zip(slots) {
        let old = param_set(img, table);
        let mut ptrs: Vec<u32> = (0..6).map(|k| word(img, old + 4 * k)).collect();
        let (axes, nib, ctl, hdr) = (at + 0x20, at + 0x40, at + 0x60, at + 0x90);
        let table_va = hdr + 0x18;
        for (k, v) in e.axes.iter().flatten().enumerate() {
            put_u16(img, file_offset(axes) + 2 * k, *v);
        }
        img[file_offset(nib)..file_offset(nib) + 20].copy_from_slice(&e.nibbles);
        for (k, v) in e.control.iter().enumerate() {
            put_u16(img, file_offset(ctl) + 2 * k, *v);
        }
        for k in 0..5 {
            put_u32(img, file_offset(hdr) + 4 * k, table_va);
        }
        for (b, m) in e.banks.iter().enumerate() {
            for row in 0..ROWS {
                let o = file_offset(table_va) + 36 * (ROWS * b + row);
                for (k, v) in m.iter().enumerate() {
                    put_u32(img, o + 4 * k, *v as u32);
                }
            }
        }
        ptrs[0] = axes;
        ptrs[1] = nib;
        ptrs[2] = hdr;
        ptrs[4] = ctl;
        for (k, p) in ptrs.iter().enumerate() {
            put_u32(img, file_offset(at) + 4 * k, *p);
        }
        put_u32(img, file_offset(SET_TABLE + 4 * table), at);
        out.push(at);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const YCC: M3 = [77.0 / 256.0, 150.0 / 256.0, 29.0 / 256.0, -43.0 / 256.0, -85.0 / 256.0, 128.0 / 256.0,
                     128.0 / 256.0, -107.0 / 256.0, -21.0 / 256.0];

    #[test]
    fn coordinate_and_direction_agree() {
        for g in (0..8192).step_by(97) {
            let (cb, cr) = direction(g as f64);
            assert!((coordinate(cb, cr) - g as f64).abs() < 1e-6, "{g}");
        }
    }

    #[test]
    fn k3_extract_red_layout() {
        // red: Q0 [0, 305) and Q1 [1395, 2048), as the K-3 III's Extract Color
        let h = Hsl { bands: vec![Band { center: (6144.0 + 1395.0 + 8192.0 + 305.0) / 2.0, width: 8192.0 - 6144.0 - 1395.0 + 305.0, sat: -100.0, ..Default::default() }], ..Default::default() };
        let e = encode(&h, &YCC).unwrap();
        assert_eq!(e.axes[0], [305, 0, 0, 0]);
        assert_eq!(e.axes[1], [1395, 0, 0, 0]);
        assert_eq!(&e.nibbles[0..2], &[0, 1]);
        assert_eq!(&e.nibbles[5..7], &[1, 0]);
        assert_eq!(e.bank_at(100.0), 1);
        assert_eq!(e.bank_at(400.0), 0);
        assert_eq!(e.bank_at(6144.0 + 1400.0), 1);
        assert_eq!(e.bank_at(6144.0 + 1390.0), 0);
    }

    #[test]
    fn grey_stays_grey() {
        let b = Band { center: 1000.0, width: 500.0, hue: 25.0, sat: 40.0, lum: -60.0, ..Default::default() };
        let m = band_matrix(&YCC, &b, 1.0);
        for r in 0..3 {
            assert!((m[3 * r] + m[3 * r + 1] + m[3 * r + 2] - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn stock_weight_curve() {
        let mut c = [0u16; CONTROL_FIELDS];
        c[10..21].copy_from_slice(&[0, 0, 0x15, 0x3e, 0, 1, 2, 7, 0, 0x280, 0x500]);
        assert!((weight(&c, 0x280 as f64) - 0x15 as f64 / 1024.0).abs() < 1e-9);
        let full = control(&Hsl::default());
        assert_eq!(weight(&full, 0.0), 1.0);
        assert_eq!(weight(&full, 4095.0), 1.0);
        let p = control(&Hsl { shadows: 1024.0, highlights: 3072.0, highlight_weight: 0.0, ..Default::default() });
        assert!(weight(&p, 0.0) < 0.01 && (weight(&p, 512.0) - 0.5).abs() < 0.01 && weight(&p, 2000.0) == 1.0);
        assert!((weight(&p, 3584.0) - 0.5).abs() < 0.01);
    }

    #[test]
    fn order_does_not_matter() {
        let a = Band { center: 5000.0, width: 600.0, feather: 150.0, hue: -5.0, sat: -30.0, ..Default::default() };
        let b = Band { center: 5500.0, width: 600.0, feather: 150.0, sat: 20.0, ..Default::default() };
        let e1 = encode(&Hsl { bands: vec![a.clone(), b.clone()], ..Default::default() }, &YCC).unwrap();
        let e2 = encode(&Hsl { bands: vec![b, a], ..Default::default() }, &YCC).unwrap();
        for g in (0..8192).step_by(8) {
            assert_eq!(e1.banks[e1.bank_at(g as f64)], e2.banks[e2.bank_at(g as f64)], "{g}");
        }
    }

    #[test]
    fn soft_edges_give_way_before_failing() {
        // two soft bands in yellow - red need 8 edges: the soft edges go, the bands stay
        let bands = vec![
            Band { center: 6650.0, width: 500.0, feather: 150.0, sat: -20.0, ..Default::default() },
            Band { center: 7700.0, width: 500.0, feather: 150.0, sat: 20.0, ..Default::default() },
        ];
        let e = encode(&Hsl { bands, ..Default::default() }, &YCC).unwrap();
        assert_eq!(e.notes.len(), 1);
        assert_ne!(e.bank_at(6650.0), 0);
        assert_ne!(e.bank_at(7700.0), 0);
        assert_eq!(e.bank_at(6300.0), 0);
    }

    fn curves(hue: Vec<[f64; 2]>, sat: Vec<[f64; 2]>, lum: Vec<[f64; 2]>) -> Hsl {
        Hsl { curves: Some(Curves { hue, sat, lum }), ..Default::default() }
    }

    #[test]
    fn curve_through_points_and_closed() {
        let pts = [[1000.0, 20.0], [3000.0, -10.0], [7000.0, 5.0]];
        let s = sample_curve(&pts, 8192);
        assert!((s[1000] - 20.0).abs() < 0.1 && (s[3000] + 10.0).abs() < 0.1);
        // no overshoot beyond the points, and closed over 8192 -> 0
        assert!(s.iter().all(|&v| (-10.0..=20.0).contains(&v)));
        assert!((s[0] - s[8191]).abs() < 0.1);
    }

    #[test]
    fn flat_curves_stay_unchanged() {
        let e = encode(&curves(vec![], vec![[100.0, 0.0], [5000.0, 0.0]], vec![]), &YCC).unwrap();
        assert_eq!(e.banks_used, 1);
        assert!(e.axes.iter().flatten().all(|&a| a == 0));
    }

    #[test]
    fn curves_fit_the_hardware() {
        // a wiggly curve in every channel: never more than 4 edges, 12 banks
        let pts = |a: f64| (0..16).map(|k| [k as f64 * 512.0, a * if k % 2 == 0 { 1.0 } else { -1.0 }]).collect::<Vec<_>>();
        let e = encode(&curves(pts(20.0), pts(60.0), pts(40.0)), &YCC).unwrap();
        assert!(e.banks_used <= BANKS);
        assert!(!e.notes.is_empty());
        // a green dip lands on green and leaves red alone
        let e = encode(&curves(vec![], vec![[4300.0, 0.0], [5050.0, -60.0], [5800.0, 0.0]], vec![]), &YCC).unwrap();
        assert_ne!(e.bank_at(5050.0), 0);
        assert_eq!(e.bank_at(7700.0), 0);
        // extremes still fit 12 bit
        let full = |v: f64| vec![[0.0, v]];
        for (h, s, l) in [(45.0, 100.0, 100.0), (-45.0, -100.0, -100.0), (45.0, 100.0, -100.0)] {
            encode(&curves(full(h), full(s), full(l)), &YCC).unwrap();
        }
    }

    #[test]
    fn base_under_the_curves() {
        // a film's HSL as the base: plateaus on the red and blue sectors
        let plate = |a: f64, b: f64, v: f64| vec![[a + 8.0, v], [b - 8.0, v], [b + 8.0, 0.0], [a - 8.0 + 8192.0, 0.0]];
        let set = Curves { hue: plate(6827.0, 7509.0, 8.0), sat: plate(2048.0, 2731.0, -30.0), lum: vec![] };
        let as_curves = encode(&Hsl { curves: Some(set.clone()), ..Default::default() }, &YCC).unwrap();
        let as_base = encode(&Hsl { base: Some(set.clone()), ..Default::default() }, &YCC).unwrap();
        let flat = Curves { hue: vec![[700.0, 0.0], [7700.0, 0.0]], ..Default::default() };
        let under_flat = encode(&Hsl { curves: Some(flat), base: Some(set.clone()), ..Default::default() }, &YCC).unwrap();
        for e in [&as_base, &under_flat] {
            assert_eq!((e.axes, e.nibbles, &e.banks), (as_curves.axes, as_curves.nibbles, &as_curves.banks));
        }
        assert!(!Hsl { base: Some(set.clone()), ..Default::default() }.is_neutral());
        // the editor sees only its own curves: flat, and flat steps
        let c = under_flat.curves.as_ref().unwrap();
        for ch in ["hue", "sat", "lum"] {
            assert!(c["smooth"][ch].as_array().unwrap().iter().all(|v| v.as_f64().unwrap().abs() < 1e-9), "{ch}");
            assert!(c["steps"][ch].as_array().unwrap().iter().all(|v| v.as_f64().unwrap().abs() < 0.6), "{ch}");
        }
        // saturation multiplies: -30 % under a flat -20 % is -44 % on blue
        let user = Curves { sat: vec![[0.0, -20.0]], ..Default::default() };
        let both = encode(&Hsl { curves: Some(user), base: Some(set), ..Default::default() }, &YCC).unwrap();
        let want = encode(&curves(vec![], plate(2048.0, 2731.0, -44.0).into_iter()
            .map(|p| [p[0], if p[1] == 0.0 { -20.0 } else { p[1] }]).collect(), vec![]), &YCC).unwrap();
        let blue = both.banks[both.bank_at(2400.0)];
        let blue_want = want.banks[want.bank_at(2400.0)];
        assert!(blue.iter().zip(blue_want).all(|(a, b)| (a - b).abs() <= 2), "{blue:?} {blue_want:?}");
        // and the user's part of the steps is -20 there
        let steps = &both.curves.as_ref().unwrap()["steps"]["sat"];
        assert!((steps[2400 / 16].as_f64().unwrap() + 20.0).abs() < 1.0);
    }

    #[test]
    fn too_many_edges() {
        let bands = (0..3).map(|k| Band { center: 300.0 + 600.0 * k as f64, width: 200.0, sat: 10.0 * (k + 1) as f64, ..Default::default() }).collect();
        assert!(encode(&Hsl { bands, ..Default::default() }, &YCC).is_err());
    }
}
