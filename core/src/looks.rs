//! Read and patch the colour data of the image controls (ported from
//! the earlier Python editor; docs/look-mapping.md has the background):
//!
//!     look -> internal index -> 0x55018d94[index] -> ColorMatrix set
//!          set+0x00 sensor CCM (shared)   set+0x04 look matrix   set+0x10 luma

use crate::addr::{file_offset, i16_at, i32_at, put_u16, put_u32, round_even, word};
use serde_json::{json, Value};

const CM_TABLE: u32 = 0x5501_8d94;
/// the three per-look tables the setting filler (0x536dc0f4) indexes with the
/// look: parameter set, +0xc block, tone set
const LOOK_TABLES: [u32; 3] = [0x5501_8d94, 0x5501_8cf0, 0x5501_8e4c];
pub const SCALE: f64 = 8192.0;
const TONE_TABLE: u32 = 0x5501_8e4c;
const YCC_BASE: u32 = 0x53dc_1444;

/// (look index, table index, name)
pub const LOOKS: [(u32, u32, &str); 15] = [
    (11, 11, "Standard"), (12, 12, "Vivid"), (13, 13, "MonotoneGR"),
    (14, 14, "SoftMonotone"), (15, 15, "HardMonotone"), (16, 16, "HighContrastBW"),
    (17, 17, "PositiveFilm"), (18, 18, "BleechBypass_GR"),
    (18, 22, "BleechBypass_GR · Var. 2"), (18, 23, "BleechBypass_GR · Var. 3"),
    (19, 19, "Retro"), (20, 20, "HdrCuisine"), (20, 24, "HdrCuisine · Var."),
    (21, 21, "CrossProcess"), (22, 25, "NegativeFilm"),
];
const MONO_MIX: [u32; 3] = [13, 14, 16];

/// Pentax makernote ImageTone value -> firmware name
pub fn image_tone_look(v: u16) -> Option<(&'static str, &'static str)> {
    // (exiftool's label, firmware name)
    Some(match v {
        256 => ("Standard", "Standard"),
        257 => ("Vivid", "Vivid"),
        258 => ("Monotone", "MonotoneGR"),
        259 => ("Soft Monotone", "SoftMonotone"),
        260 => ("Hard Monotone", "HardMonotone"),
        261 => ("Hi-contrast B&W", "HighContrastBW"),
        262 => ("Positive Film", "PositiveFilm"),
        263 => ("Bleach Bypass 2", "BleechBypass_GR"),
        264 => ("Retro", "Retro"),
        265 => ("HDR Tone", "HdrCuisine"),
        266 => ("Cross Processing 2", "CrossProcess"),
        267 => ("Negative Film", "NegativeFilm"),
        8 => ("Bleach Bypass", "BleechBypass_GR"),
        10 => ("Cross Processing", "CrossProcess"),
        _ => return None,
    })
}

pub fn table_of(name: &str) -> Option<u32> {
    LOOKS.iter().find(|l| l.2 == name).map(|l| l.1)
}

/// image where look table entry `target` points at the same look data as
/// `source` -- all three per-look tables
pub fn inherit(img: &mut [u8], target: u32, source: u32) {
    for t in LOOK_TABLES {
        let src = word(img, t + 4 * source);
        put_u32(img, file_offset(t + 4 * target), src);
    }
}

// Cross Process is the slot for the user's own look. To edit it alone it gets
// a private copy of its ColorMatrix set and matrix, placed in zero padding of
// the read-only image section (0x53002080..0x53004000). A set is 21 words
// (0x54 bytes, the spacing of the sets in .data); words 8..16 are pointers too.
// Copying only the first 8 put matrix bytes where those pointers belong, and
// the camera did not start.
const CROSS_TABLE: u32 = 21;
const PRIVATE_SET: u32 = 0x5300_2080;
const PRIVATE_MATRIX: u32 = 0x5300_20d4;
const SET_WORDS: u32 = 21;

pub fn private_cross(img: &mut [u8]) -> Result<(), String> {
    let entry = file_offset(CM_TABLE + 4 * CROSS_TABLE);
    let cur = crate::addr::u32_at(img, entry);
    if cur != PRIVATE_SET {
        let (lo, hi) = (file_offset(PRIVATE_SET), file_offset(PRIVATE_MATRIX) + 18);
        if img[lo..hi].iter().any(|&b| b != 0) {
            return Err("free area for the Cross Process copy is not empty".into());
        }
        let mut words: Vec<u32> = (0..SET_WORDS).map(|k| word(img, cur + 4 * k)).collect();
        let m = file_offset(words[1]);
        let matrix: Vec<u8> = img[m..m + 18].to_vec();
        let pm = file_offset(PRIVATE_MATRIX);
        img[pm..pm + 18].copy_from_slice(&matrix);
        words[1] = PRIVATE_MATRIX;
        for (k, w) in words.iter().enumerate() {
            put_u32(img, file_offset(PRIVATE_SET) + 4 * k, *w);
        }
        put_u32(img, entry, PRIVATE_SET);
    }
    Ok(())
}

fn matrix(img: &[u8], va: u32) -> Vec<f64> {
    let o = file_offset(va);
    (0..9).map(|k| i16_at(img, o + 2 * k) as f64 / SCALE).collect()
}

fn cm_set(img: &[u8], table: u32) -> u32 {
    word(img, CM_TABLE + 4 * table)
}

pub fn look_matrix_address(img: &[u8], table: u32) -> u32 {
    word(img, cm_set(img, table) + 4)
}

pub fn sensor_ccm(img: &[u8]) -> Vec<f64> {
    matrix(img, word(img, cm_set(img, 11)))
}

pub fn luma_weights(img: &[u8]) -> Vec<f64> {
    let o = file_offset(word(img, cm_set(img, 11) + 0x10));
    (0..3).map(|k| i32_at(img, o + 4 * k) as f64 / 256.0).collect()
}

/// the nine matrices of the saturation setting -4..+4
pub fn sat_matrices(img: &[u8], table: u32) -> Vec<Vec<f64>> {
    let t = word(img, cm_set(img, table) + 8);
    (0..9).map(|k| matrix(img, word(img, t + 4 * k))).collect()
}

/// the nine matrices of the hue setting -4..+4 (Cb/Cr rotation, after the curve)
pub fn hue_matrices(img: &[u8], table: u32) -> Vec<Vec<f64>> {
    let t = word(img, word(img, TONE_TABLE + 4 * table));
    (0..9)
        .map(|k| {
            let o = file_offset(word(img, t + 4 * k));
            (0..9).map(|j| i32_at(img, o + 4 * j) as f64 / SCALE).collect()
        })
        .collect()
}

pub fn ycc_base(img: &[u8]) -> Vec<f64> {
    let o = file_offset(YCC_BASE);
    (0..9).map(|k| i32_at(img, o + 4 * k) as f64 / 256.0).collect()
}

pub fn looks(img: &[u8]) -> Vec<Value> {
    let addr: Vec<u32> = LOOKS.iter().map(|l| look_matrix_address(img, l.1)).collect();
    LOOKS
        .iter()
        .zip(&addr)
        .map(|(&(index, table, name), &va)| {
            let shared: Vec<&str> = LOOKS.iter().zip(&addr).filter(|(l, a)| **a == va && l.2 != name).map(|(l, _)| l.2).collect();
            json!({
                "name": name, "index": index, "table": table, "address": va,
                "file_offset": file_offset(va), "matrix": matrix(img, va),
                "mono_mix": MONO_MIX.contains(&index),
                "sat_matrices": sat_matrices(img, table),
                "hue_matrices": hue_matrices(img, table),
                "shared_with": shared,
            })
        })
        .collect()
}

/// image with {look name: 9 floats} written in. Errors for unknown looks,
/// out-of-range values, or two edits on one shared matrix that disagree.
pub fn patch(img: &mut [u8], matrices: &serde_json::Map<String, Value>) -> Result<(), String> {
    let mut writes: Vec<(u32, String, Vec<i64>)> = vec![];
    for (name, values) in matrices {
        let table = table_of(name).ok_or(format!("unknown look {name:?}"))?;
        let vals: Vec<f64> = values.as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect()).unwrap_or_default();
        if vals.len() != 9 {
            return Err(format!("{name}: a matrix needs nine values"));
        }
        let fixed: Vec<i64> = vals.iter().map(|v| round_even(v * SCALE) as i64).collect();
        if !fixed.iter().all(|v| (-32768..=32767).contains(v)) {
            return Err(format!("{name}: values must stay within about -4.0 .. +4.0"));
        }
        let va = look_matrix_address(img, table);
        if let Some(w) = writes.iter().find(|w| w.0 == va) {
            if w.2 != fixed {
                return Err(format!("{name} and {} share one matrix but were given different values", w.1));
            }
            continue;
        }
        writes.push((va, name.clone(), fixed));
    }
    for (va, _, fixed) in writes {
        let o = file_offset(va);
        for (k, v) in fixed.iter().enumerate() {
            put_u16(img, o + 2 * k, *v as i16 as u16);
        }
    }
    Ok(())
}
