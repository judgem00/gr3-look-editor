//! Channel curves for every colour look (docs/plan-look-curves.md).
//!
//! Only Cross Process applies channel curves in stock firmware. At the end of
//! the context fill (RTOS 0x536dd444) it sets the variant SI+0x38A98 = 2/3/4
//! for Color Tone 1/2/3, every other look gets 0; the coordinator then takes
//! the curves from the library slots 3-5 / 6-8 / 9-11 (.data 0x55037604, in
//! RAM). The selector replaces that tail: `b STUB` at 0x536dd450 runs a stub
//! that looks the look up in its own table and, for an entry with curves,
//! writes the three pointers into slots 9..11 and sets variant 4. Cross
//! Process becomes three ordinary entries that point at its stock storage.
//! The stub returns through the context fill's own epilogue.
//!
//! At the site: r1 = look (menu index 11..22), r6 = info+0x8000 (the
//! shooting info at [r6+0x2d0]), r7 = shooting info of the stage. The
//! epilogue restores r3..r10, so the stub may use r0..r3.
//!
//! Checked in Unicorn against the whole ImageTuning Make (tools/verify-tuning.py).

use crate::addr::{file_offset, put_u32, u32_at, word};
use crate::chan;
use crate::space::RESERVED;

pub const SITE: u32 = 0x536d_d450;
/// stock words 0x536dd444..0x536dd464: cmp r1,#21 ... ldm sp,{...,pc}
const SITE_STOCK: [u32; 9] = [
    0xe351_0015, 0xe283_3044, 0xe1c5_35b0, 0x1596_32d0, 0x0a00_0017,
    0xe283_390e, 0xe3a0_2000, 0xe5c3_2a98, 0xe89d_aff8,
];
const SITE_FIRST: u32 = 0x536d_d444;

pub const STUB: u32 = RESERVED.0;
pub const TABLE: u32 = STUB + 0x80;
/// looks 11..22 (entry look - 11; 13..16 are monochrome and stay empty),
/// then Cross Process Color Tone 1..3
pub const ENTRIES: usize = 15;
const ENTRY_SIZE: u32 = 12;
pub const CROSS: u32 = 21;
const LIBRARY_9: u32 = 0x5503_7604 + 4 * 9;

/// table entry of a look (menu index) and, for Cross Process, Color Tone 0..2
pub fn entry(look: u32, tone: u32) -> Option<usize> {
    match look {
        CROSS if tone < 3 => Some(12 + tone as usize),
        CROSS => None,
        11..=22 => Some((look - 11) as usize),
        _ => None,
    }
}

fn b(cond: u32, from: usize, to: usize) -> u32 {
    let off = (to as i32 - from as i32 - 2) as u32 & 0x00ff_ffff;
    (cond << 28) | 0x0a00_0000 | off
}

fn movw(rd: u32, v: u32) -> u32 {
    0xe300_0000 | ((v >> 12) & 0xf) << 16 | rd << 12 | (v & 0xfff)
}

fn movt(rd: u32, v: u32) -> u32 {
    0xe340_0000 | ((v >> 28) & 0xf) << 16 | rd << 12 | ((v >> 16) & 0xfff)
}

const NONE: usize = 21;
const SET: usize = 22;

/// the stub
pub fn code() -> Vec<u32> {
    const HI: u32 = 0x8;
    const NE: u32 = 0x1;
    const EQ: u32 = 0x0;
    const AL: u32 = 0xe;
    vec![
        0xe241_000b,              //  0  sub   r0, r1, #11          entry = look - 11
        0xe350_000b,              //  1  cmp   r0, #11
        b(HI, 2, NONE),           //  2  bhi   none                 not a look 11..22
        0xe351_0015,              //  3  cmp   r1, #21
        b(NE, 4, 9),              //  4  bne   lookup
        0xe5d7_2797,              //  5  ldrb  r2, [r7, #0x797]     Color Tone 0..2
        0xe352_0002,              //  6  cmp   r2, #2
        b(HI, 7, NONE),           //  7  bhi   none                 as stock: variant 0
        0xe282_000c,              //  8  add   r0, r2, #12
        movw(3, TABLE),           //  9  lookup: movw r3, TABLE
        movt(3, TABLE),           // 10  movt  r3, TABLE
        0xe080_0080,              // 11  add   r0, r0, r0, lsl #1
        0xe083_3100,              // 12  add   r3, r3, r0, lsl #2   &table[entry]
        0xe893_0007,              // 13  ldm   r3, {r0, r1, r2}
        0xe350_0000,              // 14  cmp   r0, #0
        b(EQ, 15, NONE),          // 15  beq   none                 no curves
        movw(3, LIBRARY_9),       // 16  movw  r3, 0x55037628
        movt(3, LIBRARY_9),       // 17  movt  r3, 0x55037628
        0xe883_0007,              // 18  stm   r3, {r0, r1, r2}     library slots 9..11
        0xe3a0_2004,              // 19  mov   r2, #4
        b(AL, 20, SET),           // 20  b     set
        0xe3a0_2000,              // 21  none: mov r2, #0
        0xe596_32d0,              // 22  set: ldr r3, [r6, #0x2d0]  shooting info
        0xe283_390e,              // 23  add   r3, r3, #0x38000
        0xe5c3_2a98,              // 24  strb  r2, [r3, #0xa98]     variant
        0xe89d_aff8,              // 25  ldm   sp, {r3-sl, fp, sp, pc}
    ]
}

/// `b STUB` at the site
pub fn site_branch() -> u32 {
    0xea00_0000 | ((STUB.wrapping_sub(SITE + 8) >> 2) & 0x00ff_ffff)
}

pub fn is_installed(img: &[u8]) -> bool {
    word(img, SITE) == site_branch()
}

fn check_stock(img: &[u8]) -> Result<(), String> {
    for (k, &w) in SITE_STOCK.iter().enumerate() {
        if word(img, SITE_FIRST + 4 * k as u32) != w {
            return Err(format!("unexpected code at {:#x}; not the known build", SITE_FIRST + 4 * k as u32));
        }
    }
    Ok(())
}

/// table entries: Cross Process at its stock library storage (slots 3..11),
/// `curves` [(entry, [R, G, B] addresses)] on top
pub fn table(img: &[u8], curves: &[(usize, [u32; 3])]) -> [[u32; 3]; ENTRIES] {
    let lib = chan::addresses(img);
    let mut t = [[0u32; 3]; ENTRIES];
    for tone in 0..3 {
        t[12 + tone] = [lib[3 * tone + 3], lib[3 * tone + 4], lib[3 * tone + 5]];
    }
    for &(e, p) in curves {
        t[e] = p;
    }
    t
}

/// install the stub with its table; the curves themselves are written by the caller
pub fn install(img: &mut [u8], curves: &[(usize, [u32; 3])]) -> Result<(), String> {
    check_stock(img)?;
    if !crate::space::is_zero(img, STUB, TABLE + ENTRY_SIZE * ENTRIES as u32) {
        return Err("free area for the curve selector is not empty".into());
    }
    for &(e, _) in curves {
        if e >= ENTRIES || (13..=16).contains(&(e + 11)) || e == (CROSS - 11) as usize {
            return Err(format!("no curve entry {e}"));
        }
    }
    for (k, w) in code().iter().enumerate() {
        put_u32(img, file_offset(STUB) + 4 * k, *w);
    }
    for (e, ptrs) in table(img, curves).iter().enumerate() {
        for (c, p) in ptrs.iter().enumerate() {
            put_u32(img, file_offset(TABLE) + (ENTRY_SIZE as usize) * e + 4 * c, *p);
        }
    }
    put_u32(img, file_offset(SITE), site_branch());
    Ok(())
}

/// HDR without its own effects: (site, stock word, patched word)
pub const HDR_OFF: [(u32, u32, u32); 3] = [
    // local tone mapping: beq 0x53513274 -> beq 0x53513218 (the Standard path)
    (0x5351_31fc, 0x0a00_001c, 0x0a00_0005),
    // HDR strength to the pipeline: ldrb sb, [r3, #0x788] -> mov sb, #0
    (0x536d_beb4, 0xe5d3_9788, 0xe3a0_9000),
    // monochrome variant (table 24): beq 0x536dc818 -> nop, always table 20
    (0x536d_c27c, 0x0a00_0165, 0xe320_f000),
];

pub fn hdr_effects_off(img: &mut [u8]) -> Result<(), String> {
    for &(site, stock, new) in &HDR_OFF {
        let o = file_offset(site);
        let cur = u32_at(img, o);
        if cur != stock && cur != new {
            return Err(format!("unexpected code at {site:#x}; not the known build"));
        }
        put_u32(img, o, new);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arm::{Cpu, Memory};
    use std::collections::BTreeMap;

    /// sparse little-endian memory
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

    const SI: u32 = 0x6100_0000;
    const STAGE: u32 = 0x6200_0000;
    const INFO8: u32 = 0x6300_8000;
    const SP: u32 = 0x6400_0000;
    const RET: u32 = 0x6500_0000;

    /// run site + stub for a look; returns (variant, library slots 9..11)
    fn run(look: u32, tone: u8, t: &[[u32; 3]; ENTRIES]) -> (u8, [u32; 3]) {
        let mut m = Mem::default();
        m.put(SITE, site_branch());
        for (k, w) in code().iter().enumerate() {
            m.put(STUB + 4 * k as u32, *w);
        }
        for (e, p) in t.iter().enumerate() {
            for (c, &v) in p.iter().enumerate() {
                m.put(TABLE + 12 * e as u32 + 4 * c as u32, v);
            }
        }
        for k in 0..3 {
            m.put(LIBRARY_9 + 4 * k, 0xdead_0000 + k);
        }
        m.put(INFO8 + 0x2d0, SI);
        m.put(SI + 0x38a98, 0x77);
        m.write8(STAGE + 0x797, tone).unwrap();
        // the frame the epilogue unwinds: r3..r10, fp, sp, pc
        for k in 0..10 {
            m.put(SP + 4 * k, 0x100 + k);
        }
        m.put(SP + 40, RET);
        let mut cpu = Cpu::default();
        cpu.r[1] = look;
        cpu.r[6] = INFO8;
        cpu.r[7] = STAGE;
        cpu.r[13] = SP;
        cpu.r[15] = SITE;
        let mut n = 0;
        while cpu.r[15] != RET {
            cpu.step(&mut m).unwrap();
            n += 1;
            assert!(n < 100, "stub does not return");
        }
        // the epilogue restored the callee's registers
        assert_eq!(cpu.r[4], 0x101);
        assert_eq!(cpu.r[13], 0x109);
        let lib = [0, 1, 2].map(|k| m.read32(LIBRARY_9 + 4 * k).unwrap());
        (m.read8(SI + 0x38a98).unwrap(), lib)
    }

    #[test]
    fn stub_for_every_look_and_tone() {
        let mut t = [[0u32; 3]; ENTRIES];
        for (e, p) in t.iter_mut().enumerate() {
            if !(2..=5).contains(&e) && e != 10 {
                *p = [0x5300_0000 + 0x10 * e as u32, 0x5310_0000 + e as u32, 0x5320_0000 + e as u32];
            }
        }
        t[1] = [0; 3]; // Vivid without curves
        let untouched = [0xdead_0000, 0xdead_0001, 0xdead_0002];
        for look in 0..=30 {
            for tone in 0..=4u8 {
                let (variant, lib) = run(look, tone, &t);
                match entry(look, tone as u32).filter(|&e| t[e][0] != 0) {
                    Some(e) => assert_eq!((variant, lib), (4, t[e]), "look {look} tone {tone}"),
                    None => assert_eq!((variant, lib), (0, untouched), "look {look} tone {tone}"),
                }
            }
        }
    }

    #[test]
    fn branch_reaches_the_stub() {
        // b: target = site + 8 + 4 * sign-extended offset
        let off = ((site_branch() << 8) as i32 >> 6) as u32;
        assert_eq!(SITE.wrapping_add(8).wrapping_add(off), STUB);
        assert!(code().len() * 4 <= (TABLE - STUB) as usize);
    }
}
