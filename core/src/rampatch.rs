//! A look as a RAM patch, for the SD card test path (docs/plan-ram-loader.md,
//! docs/firmware/ram-loader.md): the bytes in which an exported image differs
//! from the image the camera runs, in RTOS addresses.
//!
//! Both images are compared in their RAM view, the part of the image the RTOS
//! loads: 0x53000000 up to the end of the .data copy source. Code and
//! read-only data stay where they are; the .data copy source (0x5407b540..)
//! is written to .data at 0x55000000, where the firmware reads it. A RAM dump
//! (RTOS.BIN of tools/ramload) is such a view already.
//!
//! LOOK.PAT, little-endian:
//!
//!     0x00   "GRLP0001"
//!     0x08   number of runs            0x0c  bytes in all runs
//!     0x10   CRC-32 of the base code   0x14  CRC-32 of the patched code
//!     0x18   0                         0x1c  0
//!            (code = 0x53000000..0x5407b540: the loader checks the RAM
//!            against the first before it writes, and against the second
//!            after)
//!     0x20   runs: address, length, kind (u32 each), the bytes before,
//!            the bytes after, each padded to 4
//!     end-4  CRC-32 of everything before it
//!
//! The runs come in the order the loader writes them: free space and code
//! caves first (no pointer or branch leads there yet), then .data (the
//! pointers to it), then code (the branches into the stubs). Where the base
//! already holds a look, its stubs and sets are overwritten while its
//! pointers still lead there; the loader runs while the camera starts, so
//! that window is short.

use crate::addr::{BIAS, DATA_LMA, DATA_SIZE, DATA_VMA};
use crate::space::{PADDING, RESERVED};

pub const MAGIC: &[u8; 8] = b"GRLP0001";
pub const RTOS: u32 = 0x5300_0000;
/// end of the RAM view: the end of the .data copy source
pub const VIEW_END: u32 = DATA_LMA + DATA_SIZE;
pub const VIEW_SIZE: usize = (VIEW_END - RTOS) as usize;
/// the startup literals (.data source, start, size) that every view must hold
const STARTUP: u32 = 0x5300_0450;
/// runs closer than this are joined
const GAP: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// the editor's free space (space.rs): nothing points there before the
    /// patch
    Free = 0,
    /// .data, written at 0x55000000
    Data = 1,
    /// code and read-only data the firmware runs
    Code = 2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// RTOS address in RAM (.data at 0x55000000, not at its copy source)
    pub address: u32,
    pub kind: Kind,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Patch {
    pub runs: Vec<Run>,
    pub base_code_crc: u32,
    pub patched_code_crc: u32,
}

pub fn crc32(d: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    !d.iter().fold(!0u32, |c, &b| table[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8))
}

/// the RAM view of an unpacked image or of a RAM dump; checks the startup
/// literals, so a view of another build is refused
pub fn view(d: &[u8]) -> Result<&[u8], String> {
    // a dump is the view rounded up to 4 KiB; an unpacked image has the
    // boot code before the RTOS and more after it
    let v = if (VIEW_SIZE..=VIEW_SIZE + 0x1000).contains(&d.len()) {
        &d[..VIEW_SIZE]
    } else if d.len() >= (VIEW_END - BIAS) as usize {
        &d[(RTOS - BIAS) as usize..(VIEW_END - BIAS) as usize]
    } else {
        return Err(format!("{} bytes: neither a firmware image nor a RAM dump", d.len()));
    };
    let lit = |i: u32| u32::from_le_bytes(v[(STARTUP - RTOS + 4 * i) as usize..][..4].try_into().unwrap());
    if (lit(0), lit(1), lit(2)) != (DATA_LMA, DATA_VMA, DATA_SIZE) {
        return Err("not the RTOS of GR III 2.10 / GR IIIx 1.60: the startup literals differ".into());
    }
    Ok(v)
}

/// whether a view offset lies in the .data copy source
fn in_data(off: usize) -> bool {
    RTOS + off as u32 >= DATA_LMA
}

/// RAM address of a view offset
fn ram(off: usize) -> u32 {
    let a = RTOS + off as u32;
    if in_data(off) { a - DATA_LMA + DATA_VMA } else { a }
}

fn kind(address: u32) -> Kind {
    let free = PADDING.iter().chain([&RESERVED]).any(|&(lo, hi)| (lo..hi).contains(&address))
        || (0x5300_2000..0x5300_2100).contains(&address);
    if free {
        Kind::Free
    } else if address >= DATA_VMA {
        Kind::Data
    } else {
        Kind::Code
    }
}

/// the runs that turn the view `base` into the view `patched`
pub fn diff(base: &[u8], patched: &[u8]) -> Patch {
    assert_eq!(base.len(), VIEW_SIZE);
    assert_eq!(patched.len(), VIEW_SIZE);
    let mut spans: Vec<(usize, usize)> = vec![];
    for blk in (0..VIEW_SIZE).step_by(4096) {
        let end = (blk + 4096).min(VIEW_SIZE);
        if base[blk..end] == patched[blk..end] {
            continue;
        }
        for i in blk..end {
            if base[i] == patched[i] {
                continue;
            }
            match spans.last_mut() {
                // join across short gaps, but never across a change of kind
                // or the start of .data
                Some((_, e)) if i - *e < GAP as usize && in_data(*e - 1) == in_data(i)
                    && kind(ram(*e - 1)) == kind(ram(i)) => *e = i + 1,
                _ => spans.push((i, i + 1)),
            }
        }
    }
    let mut runs: Vec<Run> = spans.into_iter().map(|(s, e)| {
        // code that was all zero is a code cave (video.rs HOLD_CAVE): nothing
        // branches there before the patch, so it goes with the free space
        let k = match kind(ram(s)) {
            Kind::Code if base[s..e].iter().all(|&b| b == 0) => Kind::Free,
            k => k,
        };
        Run { address: ram(s), kind: k, before: base[s..e].to_vec(), after: patched[s..e].to_vec() }
    }).collect();
    runs.sort_by_key(|r| (r.kind, r.address));
    let code = (DATA_LMA - RTOS) as usize;
    Patch { runs, base_code_crc: crc32(&base[..code]), patched_code_crc: crc32(&patched[..code]) }
}

/// the view with the runs written into it; checks the before bytes
pub fn apply(view: &[u8], patch: &Patch) -> Result<Vec<u8>, String> {
    let mut v = view.to_vec();
    for r in &patch.runs {
        let lma = if r.address >= DATA_VMA { r.address - DATA_VMA + DATA_LMA } else { r.address };
        let off = (lma - RTOS) as usize;
        let at = v.get_mut(off..off + r.before.len()).ok_or(format!("run at {:#x} outside the view", r.address))?;
        if at != &r.before[..] {
            return Err(format!("run at {:#x}: the bytes before do not match", r.address));
        }
        at.copy_from_slice(&r.after);
    }
    Ok(v)
}

fn pad(out: &mut Vec<u8>) {
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

pub fn encode(p: &Patch) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    let total: usize = p.runs.iter().map(|r| r.before.len()).sum();
    for w in [p.runs.len() as u32, total as u32, p.base_code_crc, p.patched_code_crc, 0, 0] {
        out.extend_from_slice(&w.to_le_bytes());
    }
    for r in &p.runs {
        for w in [r.address, r.before.len() as u32, r.kind as u32] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.extend_from_slice(&r.before);
        pad(&mut out);
        out.extend_from_slice(&r.after);
        pad(&mut out);
    }
    let crc = crc32(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

pub fn decode(d: &[u8]) -> Result<Patch, String> {
    if d.len() < 0x24 || &d[..8] != MAGIC {
        return Err("not a RAM patch (GRLP0001)".into());
    }
    let (body, tail) = d.split_at(d.len() - 4);
    if crc32(body) != u32::from_le_bytes(tail.try_into().unwrap()) {
        return Err("RAM patch: CRC mismatch".into());
    }
    let word = |o: usize| -> Result<u32, String> {
        Ok(u32::from_le_bytes(body.get(o..o + 4).ok_or("RAM patch: truncated")?.try_into().unwrap()))
    };
    let (count, total) = (word(8)? as usize, word(12)? as usize);
    let mut p = Patch { runs: vec![], base_code_crc: word(16)?, patched_code_crc: word(20)? };
    let mut o = 0x20;
    for _ in 0..count {
        let (address, len, k) = (word(o)?, word(o + 4)? as usize, word(o + 8)?);
        let kind = match k { 0 => Kind::Free, 1 => Kind::Data, 2 => Kind::Code, _ => return Err(format!("RAM patch: kind {k}")) };
        o += 12;
        let padded = (len + 3) & !3;
        let before = body.get(o..o + len).ok_or("RAM patch: truncated")?.to_vec();
        let after = body.get(o + padded..o + padded + len).ok_or("RAM patch: truncated")?.to_vec();
        o += 2 * padded;
        p.runs.push(Run { address, kind, before, after });
    }
    if o != body.len() || p.runs.iter().map(|r| r.before.len()).sum::<usize>() != total {
        return Err("RAM patch: sizes do not add up".into());
    }
    Ok(p)
}

/// bytes of `patched` outside the RAM view in which it differs from `stock`,
/// apart from the image checksum (last word): changes no RAM patch can carry
pub fn outside_view(stock: &[u8], patched: &[u8]) -> Vec<usize> {
    let (lo, hi) = ((RTOS - BIAS) as usize, (VIEW_END - BIAS) as usize);
    let n = stock.len().min(patched.len()).saturating_sub(4);
    let mut out = vec![];
    for blk in (0..n).step_by(4096) {
        let end = (blk + 4096).min(n);
        if stock[blk..end] != patched[blk..end] {
            out.extend((blk..end).filter(|&i| (i < lo || i >= hi) && stock[i] != patched[i]));
        }
    }
    out
}
