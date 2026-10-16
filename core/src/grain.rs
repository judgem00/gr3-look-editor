//! Grain for every colour look (docs/grain.md).
//!
//! In stock firmware the grain ("Particle", SI+0x795, 0 = off, 1..3) is a
//! setting of the monochrome looks and Negative Film only. It is applied by
//! the DigitalFilterProcess after development, together with shading and
//! clarity:
//!
//! - The gate (RTOS 0x535131c0, r0 = filter object, r1 = shooting info)
//!   decides per shot whether the filter runs (obj+0x24). For Standard,
//!   Vivid, Positive Film, Bleach Bypass, Retro and Cross Process it looks at
//!   shading and clarity only; for the monochrome looks and Negative Film at
//!   the grain too; for HDR at its level.
//! - The parameter fill (0x5350304c, r5 = shooting info) copies the grain
//!   into the filter's parameters (+4); a value other than 0 makes the filter
//!   engine (0x53680dbc) run its grain pass (0x5367ad3c). The fill has no
//!   look check, and in-camera RAW development calls it without the gate.
//!
//! The patch adds a table of 12 grain values (looks 11..22, 0 = as stock) and
//! two stubs: at the gate, a look with a value runs the filter; at the fill,
//! that value replaces SI+0x795. The setting itself stays untouched, so the
//! DNG records what the camera menu said.

use crate::addr::{file_offset, put_u32, word};

/// gate entry: ldrb r3, [r1, #0x77c] (the look)
pub const GATE: u32 = 0x5351_31c0;
const GATE_STOCK: u32 = 0xe5d1_377c;
/// in the parameter fill: ldrb r3, [r5, #0x795] (the grain)
pub const FILL: u32 = 0x5350_31b0;
const FILL_STOCK: u32 = 0xe5d5_3795;

/// looks 11..22 (menu index - 11)
pub const LOOKS: usize = 12;
/// highest grain value; the fill maps 0..3 to the filter, anything else to 0
pub const MAX: u8 = 3;

const GATE_WORDS: usize = 13;
const FILL_WORDS: usize = 10;
/// both stubs and the table
pub const SIZE: u32 = 4 * (GATE_WORDS + FILL_WORDS) as u32 + LOOKS as u32;

const EQ: u32 = 0x0;
const HI: u32 = 0x8;
const AL: u32 = 0xe;

fn movw(rd: u32, v: u32) -> u32 {
    0xe300_0000 | ((v >> 12) & 0xf) << 16 | rd << 12 | (v & 0xfff)
}

fn movt(rd: u32, v: u32) -> u32 {
    0xe340_0000 | ((v >> 28) & 0xf) << 16 | rd << 12 | ((v >> 16) & 0xfff)
}

/// branch (link) at `from` to `to`
fn branch(cond: u32, link: bool, from: u32, to: u32) -> u32 {
    (cond << 28) | 0x0a00_0000 | (link as u32) << 24 | ((to.wrapping_sub(from + 8) >> 2) & 0x00ff_ffff)
}

fn gate_at(at: u32) -> u32 {
    at
}

fn fill_at(at: u32) -> u32 {
    at + 4 * GATE_WORDS as u32
}

fn table_at(at: u32) -> u32 {
    fill_at(at) + 4 * FILL_WORDS as u32
}

/// the two stubs for a block at `at`
pub fn code(at: u32) -> Vec<u32> {
    let (g, f, t) = (gate_at(at), fill_at(at), table_at(at));
    let pc = |k: usize| g + 4 * k as u32;
    let gate = vec![
        0xe5d1_377c,                        //  0  ldrb  r3, [r1, #0x77c]   look
        0xe243_200b,                        //  1  sub   r2, r3, #11
        0xe352_000b,                        //  2  cmp   r2, #11
        branch(HI, false, pc(3), pc(12)),   //  3  bhi   stock              not a look 11..22
        movw(12, t),                        //  4  movw  ip, TABLE
        movt(12, t),                        //  5  movt  ip, TABLE
        0xe7dc_2002,                        //  6  ldrb  r2, [ip, r2]
        0xe352_0000,                        //  7  cmp   r2, #0
        branch(EQ, false, pc(8), pc(12)),   //  8  beq   stock              no grain of ours
        0xe3a0_2001,                        //  9  mov   r2, #1
        0xe5c0_2024,                        // 10  strb  r2, [r0, #0x24]    the filter runs
        0xe12f_ff1e,                        // 11  bx    lr
        branch(AL, false, pc(12), GATE + 4), // 12 stock: b 0x535131c4   (r3 = look)
    ];
    let fill = vec![
        0xe5d5_377c,                        //  0  ldrb  r3, [r5, #0x77c]   look
        0xe243_300b,                        //  1  sub   r3, r3, #11
        0xe353_000b,                        //  2  cmp   r3, #11
        movw(12, t),                        //  3  movw  ip, TABLE
        movt(12, t),                        //  4  movt  ip, TABLE
        0x97dc_3003,                        //  5  ldrbls r3, [ip, r3]
        0x83a0_3000,                        //  6  movhi r3, #0
        0xe353_0000,                        //  7  cmp   r3, #0
        0x05d5_3795,                        //  8  ldrbeq r3, [r5, #0x795]  the camera's setting
        0xe12f_ff1e,                        //  9  bx    lr
    ];
    debug_assert_eq!((gate.len(), fill.len(), f), (GATE_WORDS, FILL_WORDS, at + 4 * GATE_WORDS as u32));
    [gate, fill].concat()
}

/// the words at the two sites: `b gate stub`, `bl fill stub`
pub fn site_words(at: u32) -> [(u32, u32, u32); 2] {
    [
        (GATE, GATE_STOCK, branch(AL, false, GATE, gate_at(at))),
        (FILL, FILL_STOCK, branch(AL, true, FILL, fill_at(at))),
    ]
}

/// grain per look name (0 = as stock): the table, or why not
pub fn table(grain: &[(u32, u8)]) -> Result<[u8; LOOKS], String> {
    let mut t = [0u8; LOOKS];
    for &(look, g) in grain {
        if !(11..11 + LOOKS as u32).contains(&look) {
            return Err(format!("no grain for look {look}"));
        }
        if g > MAX {
            return Err(format!("grain {g}: 0..{MAX}"));
        }
        t[(look - 11) as usize] = g;
    }
    Ok(t)
}

/// install the stubs at `at` (SIZE bytes, zero) with `grain` [(menu index, 1..3)]
pub fn install(img: &mut [u8], at: u32, grain: &[(u32, u8)]) -> Result<(), String> {
    let t = table(grain)?;
    for (site, stock, _) in site_words(at) {
        if word(img, site) != stock {
            return Err(format!("unexpected code at {site:#x}; not the known build"));
        }
    }
    if !crate::space::is_zero(img, at, at + SIZE) {
        return Err(format!("free area at {at:#x} for the grain stubs is not empty"));
    }
    for (k, w) in code(at).iter().enumerate() {
        put_u32(img, file_offset(at) + 4 * k, *w);
    }
    let o = file_offset(table_at(at));
    img[o..o + LOOKS].copy_from_slice(&t);
    for (site, _, new) in site_words(at) {
        put_u32(img, file_offset(site), new);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arm::{Cpu, Memory};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Mem(BTreeMap<u32, u8>);

    impl Mem {
        fn put(&mut self, a: u32, v: u32) {
            for (k, b) in v.to_le_bytes().iter().enumerate() {
                self.0.insert(a + k as u32, *b);
            }
        }
        fn get(&self, a: u32) -> Result<u8, String> {
            self.0.get(&a).copied().ok_or(format!("unmapped read {a:#x}"))
        }
    }

    impl Memory for Mem {
        fn read32(&self, a: u32) -> Result<u32, String> {
            Ok(u32::from_le_bytes([self.get(a)?, self.get(a + 1)?, self.get(a + 2)?, self.get(a + 3)?]))
        }
        fn read16(&self, a: u32) -> Result<u16, String> {
            Ok(u16::from_le_bytes([self.get(a)?, self.get(a + 1)?]))
        }
        fn read8(&self, a: u32) -> Result<u8, String> {
            self.get(a)
        }
        fn write32(&mut self, a: u32, v: u32) -> Result<(), String> {
            self.put(a, v);
            Ok(())
        }
        fn write16(&mut self, a: u32, v: u16) -> Result<(), String> {
            self.0.insert(a, v as u8);
            self.0.insert(a + 1, (v >> 8) as u8);
            Ok(())
        }
        fn write8(&mut self, a: u32, v: u8) -> Result<(), String> {
            self.0.insert(a, v);
            Ok(())
        }
    }

    const AT: u32 = 0x5300_3f00;
    const SI: u32 = 0x6100_0000;
    const OBJ: u32 = 0x6200_0000;
    const RET: u32 = 0x6500_0000;
    /// stand-in for the stock code after the gate's first word
    const STOCK_GATE_REST: u32 = GATE + 4;

    fn mem(t: &[u8; LOOKS], look: u8, setting: u8) -> Mem {
        let mut m = Mem::default();
        for (k, w) in code(AT).iter().enumerate() {
            m.put(AT + 4 * k as u32, *w);
        }
        for (k, &v) in t.iter().enumerate() {
            m.0.insert(table_at(AT) + k as u32, v);
        }
        for (site, _, new) in site_words(AT) {
            m.put(site, new);
        }
        m.0.insert(SI + 0x77c, look);
        m.0.insert(SI + 0x795, setting);
        m.0.insert(OBJ + 0x24, 0x55);
        m
    }

    /// run from `pc` until RET or `stop`; returns the cpu and the stop reached
    fn run(m: &mut Mem, pc: u32, stop: u32) -> (Cpu, u32) {
        let mut cpu = Cpu::default();
        cpu.r[0] = OBJ;
        cpu.r[1] = SI;
        cpu.r[5] = SI;
        cpu.r[14] = RET;
        cpu.r[15] = pc;
        for _ in 0..50 {
            if cpu.r[15] == RET || cpu.r[15] == stop {
                let end = cpu.r[15];
                return (cpu, end);
            }
            cpu.step(m).unwrap();
        }
        panic!("stub does not return");
    }

    #[test]
    fn gate_runs_the_filter_for_a_look_with_grain() {
        let t = table(&[(11, 2), (21, 1)]).unwrap();
        for (look, ours) in [(11u8, true), (12, false), (21, true), (22, false), (13, false), (9, false), (30, false)] {
            let mut m = mem(&t, look, 0);
            let (cpu, end) = run(&mut m, GATE, STOCK_GATE_REST);
            if ours {
                assert_eq!(end, RET, "look {look}");
                assert_eq!(m.0[&(OBJ + 0x24)], 1, "look {look}");
            } else {
                // on to the stock gate, with the look in r3 as the stock code had it
                assert_eq!(end, STOCK_GATE_REST, "look {look}");
                assert_eq!(cpu.r[3], look as u32);
                assert_eq!(m.0[&(OBJ + 0x24)], 0x55);
            }
        }
    }

    #[test]
    fn fill_takes_our_grain_or_the_setting() {
        let t = table(&[(11, 3), (22, 2)]).unwrap();
        // (look, camera setting) -> r3 the fill continues with
        for (look, setting, want) in [(11u8, 0u8, 3u32), (11, 1, 3), (12, 0, 0), (12, 2, 2), (22, 1, 2),
                                      (13, 2, 2), (5, 1, 1), (40, 3, 3)] {
            let mut m = mem(&t, look, setting);
            // the site's bl goes to the stub, which returns behind the site
            let (cpu, end) = run(&mut m, FILL, FILL + 4);
            assert_eq!(end, FILL + 4);
            assert_eq!(cpu.r[3], want, "look {look} setting {setting}");
        }
    }

    #[test]
    fn table_checks() {
        assert!(table(&[(10, 1)]).is_err());
        assert!(table(&[(23, 1)]).is_err());
        assert!(table(&[(11, 4)]).is_err());
        assert_eq!(table(&[(22, 3)]).unwrap()[11], 3);
        assert_eq!(SIZE, 0x68);
    }
}
