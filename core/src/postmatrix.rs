//! A 3x3 colour matrix P after the tone curve, per look
//! (docs/plan-post-matrix.md). Checked in Unicorn by tools/verify-tuning.py q5.
//!
//! The YCbCr stage (calc 0x536e4720) computes its nine registers as
//!
//!     reg = (H x base + 0x1000) >> 13        9 bit two's complement, 256 = 1.0
//!
//! with base = RGB -> YCbCr (0x53dc1444, 256 = 1.0) and H the hue setting's
//! matrix (Q13): 0x55018e4c[table] -> set, set word 0 -> 9 pointers (hue
//! -4..+4). Every colour look shares those nine matrices. A look with P gets
//! its own copy holding H(k) B P B^-1, so the stage computes H(k) B P: P on the
//! gamma-encoded RGB (JPEG decoding inverts B), and the hue setting still
//! works. Only word 0 of a set is read, so the item is
//!
//!     at + 0x00   at + 4                 the set
//!     at + 0x04   9 pointers             hue table, step k at at + 0x28 + 36 k
//!     at + 0x28   9 x 9 i32 (Q13)        the matrices
//!
//! The matrices are chosen so that the register rows sum to exactly
//! 256 / 0 / 0: grey stays grey.

use crate::addr::{file_offset, i32_at, put_u32, word};
use serde_json::Value;

/// look -> set, read by the setting filler (0x536dc2a4) into setting+0x18
pub const TONE_TABLE: u32 = 0x5501_8e4c;
/// RGB -> YCbCr, 3x3 i32, 256 = 1.0
pub const YCC_BASE: u32 = 0x53dc_1444;
pub const STEPS: usize = 9;
/// bytes of one look's set, hue table and matrices
pub const SIZE: u32 = 4 + 4 * STEPS as u32 + 36 * STEPS as u32;
const MATRICES: u32 = 4 + 4 * STEPS as u32;
const Q13: f64 = 8192.0;
/// rows of P may miss 1.0 by this much (the editor sends floats)
const ROW_TOLERANCE: f64 = 1e-3;

type M3 = [f64; 9];
type I3 = [i32; 9];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut o = [0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            o[3 * i + j] = (0..3).map(|k| a[3 * i + k] * b[3 * k + j]).sum();
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

fn nine(img: &[u8], va: u32) -> I3 {
    let o = file_offset(va);
    std::array::from_fn(|k| i32_at(img, o + 4 * k))
}

pub fn base(img: &[u8]) -> I3 {
    nine(img, YCC_BASE)
}

/// the nine hue matrices (Q13) the look's set points at now
pub fn hue_matrices(img: &[u8], table: u32) -> Vec<I3> {
    let set = word(img, TONE_TABLE + 4 * table);
    let list = word(img, set);
    (0..STEPS as u32).map(|k| nine(img, word(img, list + 4 * k))).collect()
}

/// what the YCbCr calc writes for hue matrix `h`
pub fn registers(h: &I3, base: &I3) -> I3 {
    std::array::from_fn(|n| {
        let (i, j) = (n / 3, n % 3);
        let s: i64 = (0..3).map(|k| h[3 * i + k] as i64 * base[3 * k + j] as i64).sum();
        let v = ((s + 0x1000) >> 13) as i32;
        // the writer keeps 9 bits
        ((v & 0x1FF) ^ 0x100) - 0x100
    })
}

/// P from a request: nine finite numbers, rows summing to 1
pub fn parse(v: &Value) -> Result<M3, String> {
    let vals: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
    if vals.len() != 9 || vals.iter().any(|x| !x.is_finite()) {
        return Err("post matrix: need nine numbers".into());
    }
    let p: M3 = std::array::from_fn(|k| vals[k]);
    for r in 0..3 {
        let s = p[3 * r] + p[3 * r + 1] + p[3 * r + 2];
        if (s - 1.0).abs() > ROW_TOLERANCE {
            return Err(format!("post matrix: row {} sums to {s:.4}, needs 1 (grey stays grey)", r + 1));
        }
    }
    Ok(p)
}

/// the registers wanted for each hue step, rows exactly 256 / 0 / 0
fn wanted(hue: &[I3], base: &I3, p: &M3) -> Vec<I3> {
    let b: M3 = base.map(|v| v as f64);
    hue.iter().map(|h| {
        let ideal = mul(&mul(&h.map(|v| v as f64 / Q13), &b), p);
        let mut reg: I3 = ideal.map(|v| v.round() as i32);
        for (r, target) in [256, 0, 0].into_iter().enumerate() {
            let row = 3 * r..3 * r + 3;
            loop {
                let s: i32 = reg[row.clone()].iter().sum();
                if s == target {
                    break;
                }
                // move the entry that rounded the most against the step
                let step = if s < target { 1 } else { -1 };
                let j = row.clone()
                    .max_by(|&a, &b| ((ideal[a] - reg[a] as f64) * step as f64)
                        .total_cmp(&((ideal[b] - reg[b] as f64) * step as f64)))
                    .unwrap();
                reg[j] += step;
            }
        }
        reg
    }).collect()
}

/// undo P on camera JPEG pixels (RGB, 0..1): the camera applies P to the
/// gamma-encoded RGB, so P^-1 there gives what the look renders without it
pub fn undo(p: &M3, rgb: &mut [f32]) {
    let inv = inverse(p);
    for px in rgb.chunks_exact_mut(3) {
        let c = [px[0] as f64, px[1] as f64, px[2] as f64];
        for r in 0..3 {
            px[r] = (inv[3 * r] * c[0] + inv[3 * r + 1] * c[1] + inv[3 * r + 2] * c[2]).clamp(0.0, 1.0) as f32;
        }
    }
}

/// the nine Q13 matrices for `p` on `table`'s hue matrices in `img`
pub fn encode(img: &[u8], table: u32, p: &M3) -> Result<Vec<I3>, String> {
    let base = base(img);
    let inv = inverse(&base.map(|v| v as f64));
    let hue = hue_matrices(img, table);
    let mut out = vec![];
    for (k, want) in wanted(&hue, &base, p).iter().enumerate() {
        if want.iter().any(|&v| !(-256..=255).contains(&v)) {
            return Err(format!("post matrix too strong: at hue {:+} a YCbCr coefficient leaves -1.0 .. +1.0",
                               k as i32 - 4));
        }
        let s: I3 = mul(&want.map(|v| v as f64), &inv).map(|v| (v * Q13).round() as i32);
        if registers(&s, &base) != *want {
            return Err(format!("post matrix: hue {:+} does not encode exactly", k as i32 - 4));
        }
        out.push(s);
    }
    Ok(out)
}

/// write P for the look `tables` (one look; Bleach Bypass: its three tables)
/// at `at` (SIZE bytes of zeros), from the hue matrices the first table uses now
pub fn install(img: &mut [u8], tables: &[u32], at: u32, p: &M3) -> Result<(), String> {
    let o = file_offset(at);
    if img[o..o + SIZE as usize].iter().any(|&b| b != 0) {
        return Err(format!("free area {at:#x} for a post matrix is not empty"));
    }
    let mats = encode(img, tables[0], p)?;
    put_u32(img, o, at + 4);
    for (k, m) in mats.iter().enumerate() {
        let va = at + MATRICES + 36 * k as u32;
        put_u32(img, o + 4 + 4 * k, va);
        for (j, &v) in m.iter().enumerate() {
            put_u32(img, file_offset(va) + 4 * j, v as u32);
        }
    }
    for &t in tables {
        put_u32(img, file_offset(TONE_TABLE + 4 * t), at);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BASE: I3 = [77, 150, 29, -43, -85, 128, 128, -107, -21];
    const ID: I3 = [8192, 0, 0, 0, 8192, 0, 0, 0, 8192];
    const HUE4: I3 = [8192, 0, 0, 0, 8064, -1408, 0, 1408, 8128];

    #[test]
    fn identity_hue_gives_the_base() {
        assert_eq!(registers(&ID, &BASE), BASE);
    }

    #[test]
    fn rows_come_out_exact() {
        let p = [0.974, 0.0928, -0.0668, 0.081, 0.8078, 0.1112, 0.013, 0.1346, 0.8524];
        for h in [ID, HUE4] {
            let w = &wanted(&[h], &BASE, &p)[0];
            assert_eq!([w[0] + w[1] + w[2], w[3] + w[4] + w[5], w[6] + w[7] + w[8]], [256, 0, 0]);
        }
    }

    #[test]
    fn identity_p_keeps_hue_zero() {
        let w = &wanted(&[ID], &BASE, &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])[0];
        assert_eq!(*w, BASE);
    }

    #[test]
    fn undo_inverts_p() {
        let p = [0.65, 0.293, 0.057, 0.15, 0.793, 0.057, 0.15, 0.293, 0.557];
        let g = [0.2f64, 0.5, 0.7];
        let mut cam: Vec<f32> = (0..3).map(|r| (0..3).map(|k| p[3 * r + k] * g[k]).sum::<f64>() as f32).collect();
        undo(&p, &mut cam);
        for k in 0..3 {
            assert!((cam[k] as f64 - g[k]).abs() < 1e-6);
        }
    }

    #[test]
    fn parse_checks_rows() {
        assert!(parse(&json!([1, 0, 0, 0, 1, 0, 0, 0, 1])).is_ok());
        assert!(parse(&json!([1.1, 0, 0, 0, 1, 0, 0, 0, 1])).unwrap_err().contains("row 1"));
        assert!(parse(&json!([1, 0, 0])).is_err());
        assert!(parse(&json!("x")).is_err());
    }
}
