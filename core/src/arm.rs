//! A small ARM (A32) interpreter, enough to run the camera's GammaTuning.
//!
//! The Python editor runs that function in Unicorn. Its path is integer-only
//! and short (~465 distinct instructions, ~25k executed per curve): data
//! processing, multiplies, divides, loads/stores, LDM/STM and branches. This
//! covers those instruction classes fully (all conditions, shifts and flags)
//! and refuses anything else, so an unknown instruction is an error rather
//! than a wrong curve.

pub trait Memory {
    fn read32(&self, a: u32) -> Result<u32, String>;
    fn read16(&self, a: u32) -> Result<u16, String>;
    fn read8(&self, a: u32) -> Result<u8, String>;
    fn write32(&mut self, a: u32, v: u32) -> Result<(), String>;
    fn write16(&mut self, a: u32, v: u16) -> Result<(), String>;
    fn write8(&mut self, a: u32, v: u8) -> Result<(), String>;
}

#[derive(Default, Clone, Debug)]
pub struct Cpu {
    pub r: [u32; 16],
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
}

const PC: usize = 15;

#[inline]
fn bits(x: u32, hi: u32, lo: u32) -> u32 {
    (x >> lo) & ((1u32 << (hi - lo + 1)) - 1)
}

impl Cpu {
    fn cond(&self, c: u32) -> Result<bool, String> {
        Ok(match c {
            0x0 => self.z,
            0x1 => !self.z,
            0x2 => self.c,
            0x3 => !self.c,
            0x4 => self.n,
            0x5 => !self.n,
            0x6 => self.v,
            0x7 => !self.v,
            0x8 => self.c && !self.z,
            0x9 => !self.c || self.z,
            0xA => self.n == self.v,
            0xB => self.n != self.v,
            0xC => !self.z && self.n == self.v,
            0xD => self.z || self.n != self.v,
            0xE => true,
            _ => return Err("unconditional (0xF) instruction space".into()),
        })
    }

    /// register value as an operand: PC reads as the instruction + 8
    #[inline]
    fn reg(&self, i: u32, pc: u32) -> u32 {
        if i as usize == PC { pc.wrapping_add(8) } else { self.r[i as usize] }
    }

    /// shift `v` by type `t` (0 LSL, 1 LSR, 2 ASR, 3 ROR), returns (value, carry)
    fn shift(&self, v: u32, t: u32, amount: u32, imm: bool) -> (u32, bool) {
        let c = self.c;
        match (t, imm, amount) {
            (_, false, 0) => (v, c),
            (0, true, 0) => (v, c),
            (0, _, n) if n < 32 => (v << n, (v >> (32 - n)) & 1 != 0),
            (0, _, 32) => (0, v & 1 != 0),
            (0, _, _) => (0, false),
            (1, true, 0) | (1, false, 32) => (0, v >> 31 != 0),
            (1, _, n) if n < 32 => (v >> n, (v >> (n - 1)) & 1 != 0),
            (1, _, _) => (0, false),
            (2, true, 0) => (((v as i32) >> 31) as u32, v >> 31 != 0),
            (2, _, n) if n < 32 => (((v as i32) >> n) as u32, (v >> (n - 1)) & 1 != 0),
            (2, _, _) => (((v as i32) >> 31) as u32, v >> 31 != 0),
            (3, true, 0) => (((c as u32) << 31) | (v >> 1), v & 1 != 0), // RRX
            (3, _, n) => {
                let n = n & 31;
                if n == 0 { (v, v >> 31 != 0) } else { (v.rotate_right(n), (v >> (n - 1)) & 1 != 0) }
            }
            _ => unreachable!(),
        }
    }

    fn set_nz(&mut self, v: u32) {
        self.n = v >> 31 != 0;
        self.z = v == 0;
    }

    /// a + b + carry with flags
    fn adc(a: u32, b: u32, carry: bool) -> (u32, bool, bool) {
        let s = a as u64 + b as u64 + carry as u64;
        let r = s as u32;
        let v = ((a ^ r) & (b ^ r)) >> 31 != 0;
        (r, s >> 32 != 0, v)
    }

    /// shifter operand of a load/store or data-processing register form
    fn shifted_reg(&self, insn: u32, pc: u32) -> (u32, bool) {
        let rm = self.reg(bits(insn, 3, 0), pc);
        let t = bits(insn, 6, 5);
        if insn & 0x10 == 0 {
            self.shift(rm, t, bits(insn, 11, 7), true)
        } else {
            let rs = self.reg(bits(insn, 11, 8), pc) & 0xFF;
            self.shift(rm, t, rs, false)
        }
    }

    /// execute one instruction at r15; returns Err for anything unsupported
    pub fn step(&mut self, mem: &mut dyn Memory) -> Result<(), String> {
        let pc = self.r[PC];
        let insn = mem.read32(pc)?;
        let next = pc.wrapping_add(4);
        let fail = |what: &str| Err(format!("{what} {insn:#010x} at {pc:#010x}"));
        if !self.cond(insn >> 28)? {
            self.r[PC] = next;
            return Ok(());
        }
        let mut new_pc = next;
        match bits(insn, 27, 25) {
            0b000 | 0b001 => {
                let imm = bits(insn, 25, 25) == 1;
                if !imm && insn & 0x0FFF_FFD0 == 0x012F_FF10 {
                    // BX / BLX register
                    let target = self.reg(bits(insn, 3, 0), pc);
                    if target & 1 != 0 {
                        return fail("switch to Thumb");
                    }
                    if insn & 0x20 != 0 {
                        self.r[14] = next;
                    }
                    new_pc = target;
                } else if !imm && insn & 0x0FC0_00F0 == 0x0000_0090 {
                    // MUL / MLA
                    let (rd, ra, rs, rm) = (bits(insn, 19, 16), bits(insn, 15, 12), bits(insn, 11, 8), bits(insn, 3, 0));
                    let mut v = self.r[rm as usize].wrapping_mul(self.r[rs as usize]);
                    if insn & (1 << 21) != 0 {
                        v = v.wrapping_add(self.r[ra as usize]);
                    }
                    self.r[rd as usize] = v;
                    if insn & (1 << 20) != 0 {
                        self.set_nz(v);
                    }
                } else if !imm && insn & 0x0FF0_00F0 == 0x0060_0090 {
                    // MLS
                    let (rd, ra, rs, rm) = (bits(insn, 19, 16), bits(insn, 15, 12), bits(insn, 11, 8), bits(insn, 3, 0));
                    self.r[rd as usize] = self.r[ra as usize].wrapping_sub(self.r[rm as usize].wrapping_mul(self.r[rs as usize]));
                } else if !imm && insn & 0x0F80_00F0 == 0x0080_0090 {
                    // UMULL / UMLAL / SMULL / SMLAL
                    let (hi, lo, rs, rm) = (bits(insn, 19, 16) as usize, bits(insn, 15, 12) as usize, bits(insn, 11, 8) as usize, bits(insn, 3, 0) as usize);
                    let signed = insn & (1 << 22) != 0;
                    let mut p: u64 = if signed {
                        (self.r[rm] as i32 as i64).wrapping_mul(self.r[rs] as i32 as i64) as u64
                    } else {
                        (self.r[rm] as u64) * (self.r[rs] as u64)
                    };
                    if insn & (1 << 21) != 0 {
                        p = p.wrapping_add(((self.r[hi] as u64) << 32) | self.r[lo] as u64);
                    }
                    self.r[lo] = p as u32;
                    self.r[hi] = (p >> 32) as u32;
                    if insn & (1 << 20) != 0 {
                        self.n = p >> 63 != 0;
                        self.z = p == 0;
                    }
                } else if !imm && insn & 0x0E00_0090 == 0x0000_0090 && insn & 0x60 != 0 {
                    self.extra_load_store(insn, pc, mem)?;
                    if bits(insn, 15, 12) as usize == PC && insn & (1 << 20) != 0 {
                        return fail("load into pc (halfword)");
                    }
                } else if imm && insn & 0x0FF0_0000 == 0x0300_0000 {
                    // MOVW
                    self.r[bits(insn, 15, 12) as usize] = (bits(insn, 19, 16) << 12) | bits(insn, 11, 0);
                } else if imm && insn & 0x0FF0_0000 == 0x0340_0000 {
                    // MOVT
                    let rd = bits(insn, 15, 12) as usize;
                    self.r[rd] = (self.r[rd] & 0xFFFF) | (((bits(insn, 19, 16) << 12) | bits(insn, 11, 0)) << 16);
                } else if !imm && insn & 0x0FFF_0FF0 == 0x016F_0F10 {
                    // CLZ
                    self.r[bits(insn, 15, 12) as usize] = self.reg(bits(insn, 3, 0), pc).leading_zeros();
                } else {
                    let op = bits(insn, 24, 21);
                    let s = insn & (1 << 20) != 0;
                    if (8..=11).contains(&op) && !s {
                        return fail("unsupported misc instruction");
                    }
                    if !imm && insn & 0x90 == 0x90 {
                        return fail("unsupported instruction");
                    }
                    let (b, sc) = if imm {
                        let rot = bits(insn, 11, 8) * 2;
                        let v = bits(insn, 7, 0).rotate_right(rot);
                        (v, if rot == 0 { self.c } else { v >> 31 != 0 })
                    } else {
                        self.shifted_reg(insn, pc)
                    };
                    let rn = bits(insn, 19, 16);
                    let rd = bits(insn, 15, 12) as usize;
                    let a = self.reg(rn, pc);
                    let (res, write, arith) = match op {
                        0x0 => (a & b, true, None),
                        0x1 => (a ^ b, true, None),
                        0x2 => { let (r, c, v) = Self::adc(a, !b, true); (r, true, Some((c, v))) }
                        0x3 => { let (r, c, v) = Self::adc(b, !a, true); (r, true, Some((c, v))) }
                        0x4 => { let (r, c, v) = Self::adc(a, b, false); (r, true, Some((c, v))) }
                        0x5 => { let (r, c, v) = Self::adc(a, b, self.c); (r, true, Some((c, v))) }
                        0x6 => { let (r, c, v) = Self::adc(a, !b, self.c); (r, true, Some((c, v))) }
                        0x7 => { let (r, c, v) = Self::adc(b, !a, self.c); (r, true, Some((c, v))) }
                        0x8 => (a & b, false, None),
                        0x9 => (a ^ b, false, None),
                        0xA => { let (r, c, v) = Self::adc(a, !b, true); (r, false, Some((c, v))) }
                        0xB => { let (r, c, v) = Self::adc(a, b, false); (r, false, Some((c, v))) }
                        0xC => (a | b, true, None),
                        0xD => (b, true, None),
                        0xE => (a & !b, true, None),
                        _ => (!b, true, None),
                    };
                    if s {
                        if write && rd == PC {
                            return fail("flag-setting write to pc");
                        }
                        self.set_nz(res);
                        match arith {
                            Some((c, v)) => { self.c = c; self.v = v; }
                            None => self.c = sc,
                        }
                    }
                    if write {
                        if rd == PC {
                            if res & 1 != 0 {
                                return fail("switch to Thumb");
                            }
                            new_pc = res;
                        } else {
                            self.r[rd] = res;
                        }
                    }
                }
            }
            0b010 | 0b011 => {
                if bits(insn, 25, 25) == 1 && insn & 0x10 != 0 {
                    // media instructions
                    if insn & 0x0F80_03F0 == 0x0680_0070 && bits(insn, 19, 16) == 0xF {
                        // SXTB/SXTH/UXTB/UXTH (no accumulate)
                        let rm = self.reg(bits(insn, 3, 0), pc).rotate_right(bits(insn, 11, 10) * 8);
                        let v = match bits(insn, 22, 20) {
                            0b010 => rm as u8 as i8 as i32 as u32,
                            0b011 => rm as u16 as i16 as i32 as u32,
                            0b110 => rm & 0xFF,
                            0b111 => rm & 0xFFFF,
                            _ => return fail("unsupported extend"),
                        };
                        self.r[bits(insn, 15, 12) as usize] = v;
                    } else if insn & 0x0FD0_F0F0 == 0x0710_F010 {
                        // SDIV / UDIV: Rd = Rn / Rm, x / 0 = 0
                        let (rd, rm, rn) = (bits(insn, 19, 16) as usize, bits(insn, 11, 8) as usize, bits(insn, 3, 0) as usize);
                        let (n, m) = (self.r[rn], self.r[rm]);
                        self.r[rd] = if m == 0 {
                            0
                        } else if insn & (1 << 21) != 0 {
                            n / m
                        } else {
                            (n as i32).wrapping_div(m as i32) as u32
                        };
                    } else {
                        return fail("unsupported media instruction");
                    }
                } else if let Some(t) = self.load_store(insn, pc, mem)? {
                    if t & 1 != 0 {
                        return fail("switch to Thumb");
                    }
                    new_pc = t;
                }
            }
            0b100 => {
                if let Some(t) = self.block_transfer(insn, mem)? {
                    if t & 1 != 0 {
                        return fail("switch to Thumb");
                    }
                    new_pc = t;
                }
            }
            0b101 => {
                let off = ((bits(insn, 23, 0) << 8) as i32 >> 6) as u32;
                if insn & (1 << 24) != 0 {
                    self.r[14] = next;
                }
                new_pc = pc.wrapping_add(8).wrapping_add(off);
            }
            _ => return fail("unsupported coprocessor/svc instruction"),
        }
        self.r[PC] = new_pc;
        Ok(())
    }

    /// LDR/STR/LDRB/STRB; returns the new pc if the load targets it
    fn load_store(&mut self, insn: u32, pc: u32, mem: &mut dyn Memory) -> Result<Option<u32>, String> {
        let (p, u, byte, w, load) = (insn & (1 << 24) != 0, insn & (1 << 23) != 0, insn & (1 << 22) != 0,
                                     insn & (1 << 21) != 0, insn & (1 << 20) != 0);
        if !p && w {
            return Err(format!("unsupported LDRT/STRT {insn:#010x} at {pc:#010x}"));
        }
        let (rn, rt) = (bits(insn, 19, 16), bits(insn, 15, 12));
        let off = if insn & (1 << 25) == 0 { bits(insn, 11, 0) } else { self.shifted_reg(insn, pc).0 };
        let base = self.reg(rn, pc);
        let target = if u { base.wrapping_add(off) } else { base.wrapping_sub(off) };
        let addr = if p { target } else { base };
        let mut new_pc = None;
        if load {
            let v = if byte { mem.read8(addr)? as u32 } else { mem.read32(addr)? };
            if !p || w {
                self.r[rn as usize] = target;
            }
            if rt as usize == PC { new_pc = Some(v) } else { self.r[rt as usize] = v }
        } else {
            let v = if rt as usize == PC { pc.wrapping_add(8) } else { self.r[rt as usize] };
            if byte { mem.write8(addr, v as u8)? } else { mem.write32(addr, v)? }
            if !p || w {
                self.r[rn as usize] = target;
            }
        }
        Ok(new_pc)
    }

    /// LDRH/STRH/LDRSB/LDRSH/LDRD/STRD
    fn extra_load_store(&mut self, insn: u32, pc: u32, mem: &mut dyn Memory) -> Result<(), String> {
        let (p, u, i, w, load) = (insn & (1 << 24) != 0, insn & (1 << 23) != 0, insn & (1 << 22) != 0,
                                  insn & (1 << 21) != 0, insn & (1 << 20) != 0);
        let (rn, rt) = (bits(insn, 19, 16), bits(insn, 15, 12) as usize);
        let off = if i { (bits(insn, 11, 8) << 4) | bits(insn, 3, 0) } else { self.reg(bits(insn, 3, 0), pc) };
        let base = self.reg(rn, pc);
        let target = if u { base.wrapping_add(off) } else { base.wrapping_sub(off) };
        let addr = if p { target } else { base };
        match (bits(insn, 6, 5), load) {
            (1, true) => self.r[rt] = mem.read16(addr)? as u32,
            (1, false) => mem.write16(addr, self.r[rt] as u16)?,
            (2, true) => self.r[rt] = mem.read8(addr)? as i8 as i32 as u32,
            (3, true) => self.r[rt] = mem.read16(addr)? as i16 as i32 as u32,
            (2, false) => {
                self.r[rt] = mem.read32(addr)?;
                self.r[rt + 1] = mem.read32(addr.wrapping_add(4))?;
            }
            (3, false) => {
                mem.write32(addr, self.r[rt])?;
                mem.write32(addr.wrapping_add(4), self.r[rt + 1])?;
            }
            _ => return Err(format!("bad extra load/store {insn:#010x}")),
        }
        if !p || w {
            self.r[rn as usize] = target;
        }
        Ok(())
    }

    /// LDM/STM (incl. PUSH/POP); returns the new pc if pc was loaded
    fn block_transfer(&mut self, insn: u32, mem: &mut dyn Memory) -> Result<Option<u32>, String> {
        let (p, u, s, w, load) = (insn & (1 << 24) != 0, insn & (1 << 23) != 0, insn & (1 << 22) != 0,
                                  insn & (1 << 21) != 0, insn & (1 << 20) != 0);
        if s {
            return Err(format!("unsupported user-mode LDM/STM {insn:#010x}"));
        }
        let rn = bits(insn, 19, 16) as usize;
        let list = insn & 0xFFFF;
        let count = list.count_ones();
        let base = self.r[rn];
        // lowest address first, whatever the direction
        let mut addr = match (u, p) {
            (true, false) => base,
            (true, true) => base.wrapping_add(4),
            (false, false) => base.wrapping_sub(4 * count).wrapping_add(4),
            (false, true) => base.wrapping_sub(4 * count),
        };
        let wb = if u { base.wrapping_add(4 * count) } else { base.wrapping_sub(4 * count) };
        let mut new_pc = None;
        let pc_value = self.r[PC].wrapping_add(8);
        let mut loaded = [0u32; 16];
        for i in 0..16 {
            if list & (1 << i) == 0 {
                continue;
            }
            if load {
                loaded[i] = mem.read32(addr)?;
            } else {
                mem.write32(addr, if i == PC { pc_value } else { self.r[i] })?;
            }
            addr = addr.wrapping_add(4);
        }
        if w {
            self.r[rn] = wb;
        }
        if load {
            for i in 0..16 {
                if list & (1 << i) != 0 {
                    if i == PC { new_pc = Some(loaded[i]) } else { self.r[i] = loaded[i] }
                }
            }
        }
        Ok(new_pc)
    }
}
