//! Free space for the data the editor adds (docs/plan-look-curves.md).
//!
//! One pool, shared by HSL sets and channel curves:
//!
//!     0x53002100..0x53003d00   padding, 3 HSL slots       } no pointer leads
//!     0x53000d50..0x53002000   padding, 2 HSL slots       } into them
//!     library curves 0..2      the Random set of Cross Process (3 x 512 B);
//!                              unreachable once the selector is in
//!     0x53003d00..0x53004000   reserved: selector stub, its table and the
//!                              private HDR colour set (curvesel.rs)
//!
//! (0x53002000..0x53002100 holds the model header and the private Cross
//! Process set of looks.rs.)
//!
//! HSL sets keep their fixed slots, in the order above, so an export without
//! curves places them where it always has. Channel curves go first into the
//! Random library storage, then into what the HSL sets leave of the padding.

use crate::addr::file_offset;
use crate::chan;
use serde_json::{json, Value};

pub const PADDING: [(u32, u32); 2] = [(0x5300_2100, 0x5300_3d00), (0x5300_0d50, 0x5300_2000)];
pub const RESERVED: (u32, u32) = (0x5300_3d00, 0x5300_4000);
/// one MultiAxial set: pointers, axes, nibbles, control, matrix header and 60 matrices
pub const HSL_SIZE: u32 = 0x20 + 0x20 + 0x20 + 0x30 + 0x890;
/// one channel curve, 256 x u16
pub const CURVE_SIZE: u32 = 512;
/// looks that can get curves of their own besides Cross Process
pub const CURVE_LOOKS: usize = 7;

/// HSL slots in the order they are handed out
pub fn hsl_slots() -> Vec<u32> {
    let mut slots = vec![];
    for &(lo, hi) in &PADDING {
        let mut at = lo;
        while at + HSL_SIZE <= hi {
            slots.push(at);
            at += HSL_SIZE;
        }
    }
    slots
}

/// library storages of the Random set (slots 0..2)
pub fn library_spare(img: &[u8]) -> Vec<u32> {
    chan::addresses(img)[0..3].to_vec()
}

pub fn is_zero(img: &[u8], lo: u32, hi: u32) -> bool {
    img[file_offset(lo)..file_offset(hi)].iter().all(|&b| b == 0)
}

/// where everything goes
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub hsl: Vec<u32>,
    pub curves: Vec<u32>,
}

/// padding left over after `hsl` sets, as (lo, hi) runs
fn leftovers(hsl: usize) -> Vec<(u32, u32)> {
    let used: Vec<u32> = hsl_slots().into_iter().take(hsl).collect();
    let mut out = vec![];
    for &(lo, hi) in &PADDING {
        let mut at = lo;
        for &s in used.iter().filter(|&&s| s >= lo && s < hi) {
            if s > at {
                out.push((at, s));
            }
            at = at.max(s + HSL_SIZE);
        }
        if at < hi {
            out.push((at, hi));
        }
    }
    out
}

fn curve_places(img: &[u8], hsl: usize) -> Vec<u32> {
    let mut places = library_spare(img);
    for (lo, hi) in leftovers(hsl) {
        let mut at = lo;
        while at + CURVE_SIZE <= hi {
            places.push(at);
            at += CURVE_SIZE;
        }
    }
    places
}

/// addresses for `hsl` HSL sets and `curves` channel curves, or why they do not fit
pub fn plan(img: &[u8], hsl: usize, curves: usize) -> Result<Plan, String> {
    let slots = hsl_slots();
    if hsl > slots.len() {
        return Err(format!("HSL fits into at most {} image controls", slots.len()));
    }
    let places = curve_places(img, hsl);
    if curves > places.len() {
        return Err(format!(
            "not enough free space: {} RGB sets need {} KB, {} KB are left next to {hsl} HSL sets",
            curves.div_ceil(3), kb(curves as u32 * CURVE_SIZE), kb(places.len() as u32 * CURVE_SIZE)
        ));
    }
    let p = Plan { hsl: slots[..hsl].to_vec(), curves: places[..curves].to_vec() };
    for &at in &p.hsl {
        if !is_zero(img, at, at + HSL_SIZE) {
            return Err(format!("free area at {at:#x} for the HSL data is not empty"));
        }
    }
    let spare = library_spare(img);
    for &at in p.curves.iter().filter(|a| !spare.contains(a)) {
        if !is_zero(img, at, at + CURVE_SIZE) {
            return Err(format!("free area at {at:#x} for a channel curve is not empty"));
        }
    }
    Ok(p)
}

fn kb(bytes: u32) -> f64 {
    (bytes as f64 / 1024.0 * 10.0).round() / 10.0
}

fn total() -> u32 {
    PADDING.iter().map(|(lo, hi)| hi - lo).sum::<u32>() + 3 * CURVE_SIZE
}

/// the meter: how full the pool is with `hsl` HSL sets and `rgb` RGB sets
/// (Cross Process excluded, it keeps its own storage), and what still fits
pub fn report(img: &[u8], hsl: usize, rgb: usize) -> Value {
    let fits = plan(img, hsl, 3 * rgb).is_ok();
    let used = hsl as u32 * HSL_SIZE + 3 * rgb as u32 * CURVE_SIZE;
    let more = |f: &dyn Fn(usize) -> bool| (1..).take_while(|&k| f(k)).count();
    let more_rgb = if fits { more(&|k| rgb + k <= CURVE_LOOKS && plan(img, hsl, 3 * (rgb + k)).is_ok()) } else { 0 };
    let more_hsl = if fits { more(&|k| plan(img, hsl + k, 3 * rgb).is_ok()) } else { 0 };
    json!({
        "fits": fits,
        "total": total(),
        "used": used,
        "free": total().saturating_sub(used),
        "hsl": hsl,
        "rgb": rgb,
        "more_rgb": more_rgb,
        "more_hsl": more_hsl,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_and_leftovers() {
        let s = hsl_slots();
        assert_eq!(s, vec![0x5300_2100, 0x5300_2a20, 0x5300_3340, 0x5300_0d50, 0x5300_1670]);
        // no HSL: two whole runs
        assert_eq!(leftovers(0), PADDING.to_vec());
        // every slot taken: only the 16-byte tails remain
        let all = leftovers(5);
        assert!(all.iter().all(|(lo, hi)| hi - lo < CURVE_SIZE), "{all:x?}");
        // never inside an HSL slot
        for n in 0..=5 {
            for (lo, hi) in leftovers(n) {
                for &h in &s[..n] {
                    assert!(hi <= h || lo >= h + HSL_SIZE);
                }
            }
        }
    }
}
