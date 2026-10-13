//! The camera's tone curve, computed by the camera's own code.
//!
//! GammaTuning (RTOS 0x536e7b64) builds the 256-entry, 14-bit tone curve from
//! four image-control settings -- key, contrast, contrast highlight, contrast
//! shadow -- and a gain, by blending 15 base curves stored in the image:
//!
//!     0x550204d4[family] -> 5 curve pointers, one per white-level band
//!     family 0 = high contrast, 1 = normal, 2 = low contrast
//!
//! As in the earlier Python editor (there in Unicorn) the original ARM code runs on
//! the firmware image, here in the interpreter of arm.rs. Edited base curves go
//! through exactly the arithmetic the camera uses.

use crate::addr::{file_offset, u16_at, word, BIAS, DATA_SIZE, DATA_VMA};
use crate::arm::{Cpu, Memory};

const GAMMA_FN: u32 = 0x536e_7b64;
const FAMILIES: u32 = 0x5502_04d4;
pub const CURVE_LEN: usize = 256;
const CODE_START: u32 = 0x5300_0000;
const CODE_END: u32 = 0x540c_6000;
const DATA_MAP: u32 = 0x40000;
const STACK: u32 = 0x6000_0000;
const STACK_SIZE: u32 = 0x10000;
const SCRATCH: u32 = 0x6100_0000;
const SCRATCH_SIZE: u32 = 0x1000;
const STOP: u32 = 0x6200_0000;
const MAX_STEPS: usize = 5_000_000;

#[derive(Clone, Copy, Debug, Default)]
pub struct ToneParams {
    pub key: i32,
    pub contrast: i32,
    pub highlight: i32,
    pub shadow: i32,
    pub gain: i32,
    pub flag: u8,
}

/// (family, band, address) of the 15 base curves
pub fn base_addresses(image: &[u8]) -> Vec<(usize, usize, u32)> {
    let mut out = vec![];
    for fam in 0..3 {
        let cset = word(image, FAMILIES + 4 * fam as u32);
        for band in 0..5 {
            out.push((fam, band, word(image, cset + 4 * band as u32)));
        }
    }
    out
}

/// the 15 base curves, in base_addresses order
pub fn base_curves(image: &[u8]) -> Vec<Vec<u16>> {
    base_addresses(image).iter().map(|&(_, _, va)| crate::addr::curve_at(image, va)).collect()
}

/// image with replaced base curves: {index into base_addresses: 256 values}
pub fn with_curves(image: &mut [u8], curves: &[(usize, Vec<u16>)]) {
    let addrs = base_addresses(image);
    for (i, vals) in curves {
        crate::addr::put_curve(image, addrs[*i].2, vals);
    }
}

struct Mem<'a> {
    image: &'a [u8],
    /// replaced base curves: (address, 512 bytes)
    overlay: &'a [(u32, Vec<u8>)],
    stack: Vec<u8>,
    scratch: Vec<u8>,
}

enum Loc {
    Image(usize),
    Stack(usize),
    Scratch(usize),
    Zero,
}

impl Mem<'_> {
    fn locate(&self, a: u32, n: u32) -> Result<Loc, String> {
        let inside = |lo: u32, size: u32| a >= lo && a.wrapping_add(n) <= lo + size;
        if inside(CODE_START, CODE_END - CODE_START) {
            Ok(Loc::Image((a - BIAS) as usize))
        } else if inside(DATA_VMA, DATA_SIZE) {
            Ok(Loc::Image(file_offset(a)))
        } else if inside(DATA_VMA, DATA_MAP) {
            Ok(Loc::Zero) // mapped beyond .data's stored size: zero, as in Unicorn
        } else if inside(STACK, STACK_SIZE) {
            Ok(Loc::Stack((a - STACK) as usize))
        } else if inside(SCRATCH, SCRATCH_SIZE) {
            Ok(Loc::Scratch((a - SCRATCH) as usize))
        } else {
            Err(format!("access to unmapped memory {a:#010x}"))
        }
    }

    fn read(&self, a: u32, n: u32) -> Result<u32, String> {
        for (va, b) in self.overlay {
            let end = va + b.len() as u32;
            if a < end && a + n > *va {
                // per byte, so a word straddling the edge of a curve works too
                let mut v = 0u32;
                for k in 0..n {
                    let x = a + k;
                    let byte = if x >= *va && x < end { b[(x - va) as usize] as u32 } else { self.read(x, 1)? };
                    v |= byte << (8 * k);
                }
                return Ok(v);
            }
        }
        let bytes: &[u8] = match self.locate(a, n)? {
            Loc::Image(o) => &self.image[o..o + n as usize],
            Loc::Stack(o) => &self.stack[o..o + n as usize],
            Loc::Scratch(o) => &self.scratch[o..o + n as usize],
            Loc::Zero => return Ok(0),
        };
        Ok(bytes.iter().rev().fold(0u32, |v, b| (v << 8) | *b as u32))
    }

    fn write(&mut self, a: u32, n: u32, v: u32) -> Result<(), String> {
        let buf = match self.locate(a, n)? {
            Loc::Stack(o) => &mut self.stack[o..o + n as usize],
            Loc::Scratch(o) => &mut self.scratch[o..o + n as usize],
            _ => return Err(format!("write to firmware memory {a:#010x}")),
        };
        for (k, b) in buf.iter_mut().enumerate() {
            *b = (v >> (8 * k)) as u8;
        }
        Ok(())
    }
}

impl Memory for Mem<'_> {
    fn read32(&self, a: u32) -> Result<u32, String> { self.read(a, 4) }
    fn read16(&self, a: u32) -> Result<u16, String> { Ok(self.read(a, 2)? as u16) }
    fn read8(&self, a: u32) -> Result<u8, String> { Ok(self.read(a, 1)? as u8) }
    fn write32(&mut self, a: u32, v: u32) -> Result<(), String> { self.write(a, 4, v) }
    fn write16(&mut self, a: u32, v: u16) -> Result<(), String> { self.write(a, 2, v as u32) }
    fn write8(&mut self, a: u32, v: u8) -> Result<(), String> { self.write(a, 1, v as u32) }
}

/// GammaTuning's curve builder on `image`, with base curves optionally
/// replaced: `curves` holds (index into base_addresses, 256 values)
pub struct Engine {
    overlay: Vec<(u32, Vec<u8>)>,
}

impl Engine {
    pub fn new(image: &[u8], curves: &[(usize, Vec<u16>)]) -> Engine {
        let addrs = base_addresses(image);
        let overlay = curves
            .iter()
            .map(|(i, vals)| (addrs[*i].2, vals.iter().flat_map(|v| v.to_le_bytes()).collect()))
            .collect();
        Engine { overlay }
    }

    pub fn curve(&self, image: &[u8], p: ToneParams) -> Result<Vec<u16>, String> {
        let mut mem = Mem { image, overlay: &self.overlay, stack: vec![0; STACK_SIZE as usize],
                            scratch: vec![0; SCRATCH_SIZE as usize] };
        // struct: 0, key, contrast, -shadow, highlight, flag (u8 + 3 pad), gain
        let par = 0x800usize;
        let fields = [0, p.key, p.contrast, -p.shadow, p.highlight];
        for (k, v) in fields.iter().enumerate() {
            mem.scratch[par + 4 * k..par + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
        mem.scratch[par + 20] = p.flag;
        mem.scratch[par + 24..par + 28].copy_from_slice(&p.gain.to_le_bytes());
        let mut cpu = Cpu::default();
        cpu.r[0] = SCRATCH;
        cpu.r[1] = SCRATCH;
        cpu.r[2] = SCRATCH + par as u32;
        cpu.r[13] = STACK + STACK_SIZE - 0x100;
        cpu.r[14] = STOP;
        cpu.r[15] = GAMMA_FN;
        let mut steps = 0;
        while cpu.r[15] != STOP {
            cpu.step(&mut mem)?;
            steps += 1;
            if steps > MAX_STEPS {
                return Err("GammaTuning did not return".into());
            }
        }
        Ok((0..CURVE_LEN).map(|i| u16_at(&mem.scratch, 2 * i)).collect())
    }
}

