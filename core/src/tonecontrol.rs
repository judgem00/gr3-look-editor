//! The camera's shadow correction: ToneControlTuning (RTOS 0x536e9558,
//! table builder 0x536e93ac; gr3x-fw-hack docs/tone-curves.md).
//!
//! On linear RGB behind the colour matrix and before the tone curve, every
//! pixel is multiplied by a gain from a 1024-entry table, indexed by its
//! 12-bit value / 4. The table is 1 + S · (base − 1) over the first 256
//! entries and 1 above; the base (0x53dc1f5c, 256 × u16, 1.0 = 0x1000) runs
//! from 2.0 at black to 1.0 at about 15 % of the input. The strength S
//! comes from the setting (makernote 0x0079, second byte):
//!
//!     1 Low 0.25    2 Medium 0.5    3 Strong 1.0    4 (Auto in the menu) and above: off
//!
//! Level 0 would derive S from a histogram of the AE statistics (at most
//! 0.25); no shot has shown it, and it is not emulated. Which value of a
//! pixel indexes the table is not in the code (the hardware does it); the
//! largest channel fitted MR022009 best (mean difference to the camera JPEG
//! 7.4 -> 3.1 levels with Strong), luma and green within 0.1 of it.

use crate::addr::{file_offset, u16_at};

const BASE: u32 = 0x53dc_1f5c;
pub const TABLE: usize = 1024;

/// the 256 values of the base curve
pub fn base(img: &[u8]) -> Vec<u16> {
    let o = file_offset(BASE);
    (0..256).map(|i| u16_at(img, o + 2 * i)).collect()
}

/// S as a fraction (0..1) for the makernote level
pub fn strength(level: u8) -> f64 {
    match level {
        1 => 0.25,
        2 => 0.5,
        3 => 1.0,
        _ => 0.0,
    }
}

/// the gain table (1.0 = 1.0) as CalcTable builds it, None for no correction
pub fn table(base: &[u16], s: f64) -> Option<Vec<f32>> {
    if s <= 0.0 {
        return None;
    }
    let s = (s * 1024.0).round() as i64;
    let mut t = vec![1.0f32; TABLE];
    for (i, &b) in base.iter().enumerate() {
        // C division truncates towards zero, as the camera's code
        t[i] = (4096 + s * (b as i64 - 4096) / 1024) as f32 / 4096.0;
    }
    Some(t)
}

/// the correction on one pixel in 12-bit units
#[inline]
pub fn apply(t: &[f32], v: [f64; 3]) -> [f64; 3] {
    let m = v[0].max(v[1]).max(v[2]);
    let g = t[((m / 4.0) as usize).min(TABLE - 1)] as f64;
    [v[0] * g, v[1] * g, v[2] * g]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strong_doubles_black_and_leaves_mid_tones() {
        let base: Vec<u16> = (0..256).map(|i| if i < 150 { (8192 - i * 4096 / 150) as u16 } else { 4096 }).collect();
        let t = table(&base, 1.0).unwrap();
        assert!((t[0] - 2.0).abs() < 1e-6 && t[200] == 1.0 && t[1000] == 1.0);
        let low = table(&base, 0.25).unwrap();
        assert!((low[0] - 1.25).abs() < 1e-3);
        assert!(table(&base, 0.0).is_none());
        let v = apply(&t, [0.0, 2.0, 1.0]);
        assert!((v[1] - 4.0).abs() < 1e-6);
    }
}
