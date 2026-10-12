//! Per-channel tone curves (R, G, B) of Cross Process (port of
//! editor/chancurves.py). 12 tables of 256 x u16, reached through .data
//! 0x55037604[0..11]; the camera builds each channel from the computed tone
//! curve through one of them (RTOS 0x536e8660):
//!
//!     channel[i] = C(base[i])      C interpolated on base >> 6, step 64
//!
//! Sets: random 0-2, preset 1 (Farbton 1) 3-5, preset 2 6-8, preset 3 9-11.

use crate::addr::{curve_at, put_curve, round_even, word};
use serde_json::Value;

const TABLE: u32 = 0x5503_7604;
pub const COUNT: usize = 12;
pub const SETS: [(&str, [usize; 3]); 4] = [
    ("Zufall", [0, 1, 2]),
    ("Preset 1", [3, 4, 5]),
    ("Preset 2", [6, 7, 8]),
    ("Preset 3", [9, 10, 11]),
];

/// C[k] = 64 k passes the tone curve through; the last entry keeps white white
pub fn linear() -> Vec<u16> {
    let mut v: Vec<u16> = (0..255).map(|k| 64 * k as u16).collect();
    v.push(16383);
    v
}

pub fn addresses(img: &[u8]) -> Vec<u32> {
    (0..COUNT).map(|i| word(img, TABLE + 4 * i as u32)).collect()
}

pub fn read(img: &[u8]) -> Vec<Vec<u16>> {
    addresses(img).iter().map(|&va| curve_at(img, va)).collect()
}

/// image with {index: 256 values} written in
pub fn write(img: &mut [u8], curves: &[(usize, Vec<f64>)]) -> Result<(), String> {
    let addrs = addresses(img);
    for (idx, vals) in curves {
        if *idx >= COUNT || vals.len() != 256 {
            return Err(format!("channel curve {idx}: need 256 values"));
        }
        let v: Vec<u16> = vals.iter().map(|x| round_even(*x).clamp(0.0, 65535.0) as u16).collect();
        put_curve(img, addrs[*idx], &v);
    }
    Ok(())
}

/// channel table from the computed tone curve, as 0x536e8660 does it
pub fn compose(base: &[u16], curve: &[i64]) -> Vec<i64> {
    base.iter()
        .map(|&b| {
            let (k, f) = ((b >> 6) as usize, (b & 0x3F) as i64);
            if k >= 0xFF {
                return curve[0xFF];
            }
            let mut d = (curve[k + 1] - curve[k]) * f;
            if d < 0 {
                d += 0x3F; // C division, toward zero
            }
            (curve[k] + (d >> 6)) & 0xFFFF
        })
        .collect()
}

// ---- colour grading wheels (see chancurves.py) ----

const WHEELS: [&str; 4] = ["shadows", "midtones", "highlights", "global"];
const TINT: f64 = 0.2;
const LUMA: f64 = 0.2;
const FULL: f64 = 16383.0;

fn num(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0)
}

pub fn grade_is_neutral(grade: &Value) -> bool {
    if grade.is_null() || grade.as_object().is_some_and(|o| o.is_empty()) {
        return true;
    }
    WHEELS.iter().all(|w| {
        let wh = grade.get(*w).cloned().unwrap_or(Value::Null);
        num(&wh, "amount").abs() < 1e-4 && num(&wh, "lum").abs() < 1e-4
    })
}

/// [R, G, B] channel curves with the wheels applied
pub fn grade(curves: &[Vec<i64>], grade: &Value) -> Vec<Vec<i64>> {
    if grade_is_neutral(grade) {
        return curves.to_vec();
    }
    let balance = num(grade, "balance").clamp(-1.0, 1.0);
    let expo = 2f64.powf(-balance);
    let x: Vec<f64> = (0..256).map(|i| (i as f64 * 64.0 / FULL).min(1.0)).collect();
    let weight = |name: &str, xv: f64| {
        let xb = xv.powf(expo);
        match name {
            "shadows" => (1.0 - xb).powi(2),
            "midtones" => 4.0 * xb * (1.0 - xb),
            "highlights" => xb.powi(2),
            _ => 1.0,
        }
    };
    curves
        .iter()
        .enumerate()
        .map(|(ch, c)| {
            let mut y: Vec<f64> = c.iter().map(|&v| v as f64 / FULL).collect();
            for name in WHEELS {
                let wh = grade.get(name).cloned().unwrap_or(Value::Null);
                let amount = num(&wh, "amount").clamp(0.0, 1.0);
                let lum = num(&wh, "lum").clamp(-1.0, 1.0);
                let hue = (num(&wh, "hue") - 120.0 * ch as f64).to_radians();
                let off = TINT * amount * hue.cos() + LUMA * lum;
                for (yi, xv) in y.iter_mut().zip(&x) {
                    *yi += weight(name, *xv) * off;
                }
            }
            y.iter().map(|v| round_even(v.clamp(0.0, 1.0) * FULL) as i64).collect()
        })
        .collect()
}
