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

/// as inherit, with the source's entries read from `stock`, so that a source
/// that is a target itself passes on its stock data
pub fn inherit_from(img: &mut [u8], stock: &[u8], target: u32, source: u32) {
    for t in LOOK_TABLES {
        put_u32(img, file_offset(t + 4 * target), word(stock, t + 4 * source));
    }
}

// A look edited on its own gets a private copy of its ColorMatrix set and
// matrix (private_set_at), placed in zero padding of the read-only image
// section. A set is 21 words (0x54 bytes, the spacing of the sets in .data);
// words 8..16 are pointers too. Copying only the first 8 put matrix bytes
// where those pointers belong, and the camera did not start.
// Cross Process (the earlier editor's slot for the user's look) and HDR with
// its effects off have fixed places; other targets get theirs from space.rs.
const CROSS_TABLE: u32 = 21;
pub const HDR_TABLE: u32 = 20;
/// (table, set, matrix)
const PRIVATE: [(u32, u32, u32); 2] = [
    (CROSS_TABLE, 0x5300_2080, 0x5300_20d4),
    (HDR_TABLE, 0x5300_3e40, 0x5300_3e94),
];
const SET_WORDS: u32 = 21;

// Toning (docs/plan-gui-redesign.md, Phase 0). Words 11..14 of a ColorMatrix
// set point at tables of i16 offsets that 0x536e73d0 indexes with the look's
// toning: Bleach Bypass's variant setting SI+0x786 (1..3), Retro's toning
// SI+0x787 (0..8, 4 neutral), 0 for every other colour look. Only Bleach
// Bypass and Retro have tables that are not all zero.
const TONING_WORDS: [u32; 4] = [11, 12, 13, 14];
/// a table of zeros, long enough for any toning index (0..8)
pub const ZERO_TONING: u32 = 0x53dc_2430;
/// one toning table as the editor writes it: 10 x i16
pub const TONING_TABLE: u32 = 20;
/// a set (21 words) followed by its matrix, rounded up
pub const SET_SIZE: u32 = 0x68;
pub const BLEACH_TABLES: [u32; 3] = [18, 22, 23];
const RETRO_TABLE: u32 = 19;

/// toning index a look's own image control uses at its default setting
pub fn toning_index(table: u32) -> u32 {
    match table {
        18 => 1,
        22 => 2,
        23 => 3,
        RETRO_TABLE => 4,
        _ => 0,
    }
}

/// does the look's own menu move its toning index (Bleach Bypass, Retro)?
pub fn varying_toning(table: u32) -> bool {
    BLEACH_TABLES.contains(&table) || table == RETRO_TABLE
}

/// the source's toning offsets at its default index, words 13 and 14
/// (words 11 and 12 are zero tables for every colour look)
fn toning_entry(img: &[u8], source: u32) -> [i16; 2] {
    let set = cm_set(img, source);
    let k = toning_index(source);
    [13, 14].map(|w| i16_at(img, file_offset(word(img, set + 4 * w) + 2 * k)))
}

/// How a private set reads the source's toning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Toning {
    /// as copied (the target is its own source)
    Keep,
    /// the target reads index 0: pointers moved by 2 x the source's index
    Shift(u32),
    /// the target's own menu moves the index: words 13 and 14 point at two
    /// tables that repeat the source's entry (zeros when it is 0, 0)
    Repeat([i16; 2]),
}

impl Toning {
    /// what a target needs to look like its source (tables)
    pub fn of(img: &[u8], target: u32, source: u32) -> Toning {
        if target == source {
            Toning::Keep
        } else if varying_toning(target) {
            Toning::Repeat(toning_entry(img, source))
        } else {
            Toning::Shift(toning_index(source))
        }
    }

    /// does it need two toning tables (2 x TONING_TABLE bytes) of its own?
    pub fn needs_tables(&self) -> bool {
        matches!(self, Toning::Repeat(e) if *e != [0, 0])
    }
}

/// Give the looks `tables` one ColorMatrix set of their own at `set`
/// (SET_SIZE bytes, matrix at +0x54): a copy of the set the first of them
/// points at now, with its matrix; all of them point there afterwards.
/// `tables_at`: where Repeat writes its two toning tables.
pub fn private_set_at(img: &mut [u8], tables: &[u32], set: u32, toning: Toning,
                      tables_at: Option<u32>) -> Result<(), String> {
    let mat = set + 0x54;
    let (lo, hi) = (file_offset(set), file_offset(mat) + 18);
    if img[lo..hi].iter().any(|&b| b != 0) {
        return Err(format!("free area {set:#x} for a private colour set is not empty"));
    }
    let cur = cm_set(img, tables[0]);
    let mut words: Vec<u32> = (0..SET_WORDS).map(|k| word(img, cur + 4 * k)).collect();
    let m = file_offset(words[1]);
    let matrix: Vec<u8> = img[m..m + 18].to_vec();
    let pm = file_offset(mat);
    img[pm..pm + 18].copy_from_slice(&matrix);
    words[1] = mat;
    match toning {
        Toning::Keep => {}
        Toning::Shift(k) => {
            for w in TONING_WORDS {
                words[w as usize] += 2 * k;
            }
        }
        Toning::Repeat(e) => {
            for w in TONING_WORDS {
                words[w as usize] = ZERO_TONING;
            }
            if e != [0, 0] {
                let at = tables_at.ok_or("no place for the toning tables")?;
                for (n, &v) in e.iter().enumerate() {
                    let t = at + n as u32 * TONING_TABLE;
                    let o = file_offset(t);
                    if img[o..o + TONING_TABLE as usize].iter().any(|&b| b != 0) {
                        return Err(format!("free area {t:#x} for a toning table is not empty"));
                    }
                    for k in 0..TONING_TABLE as usize / 2 {
                        put_u16(img, o + 2 * k, v as u16);
                    }
                    words[13 + n] = t;
                }
            }
        }
    }
    for (k, w) in words.iter().enumerate() {
        put_u32(img, file_offset(set) + 4 * k, *w);
    }
    for &t in tables {
        put_u32(img, file_offset(CM_TABLE + 4 * t), set);
    }
    Ok(())
}

/// fixed private sets: Cross Process always, HDR with its effects off
pub fn fixed_set(table: u32) -> Option<u32> {
    PRIVATE.iter().find(|p| p.0 == table).map(|p| p.1)
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
