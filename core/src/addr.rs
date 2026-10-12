//! Little-endian access to the unpacked firmware image by RTOS address.
//!
//! RTOS code and read-only data sit at RAM = file offset + BIAS. The startup
//! code copies .data from the image to 0x55000000, so .data addresses have to
//! be moved back to where they are stored.

pub const BIAS: u32 = 0x52FE_F790;
pub const DATA_VMA: u32 = 0x5500_0000;
pub const DATA_LMA: u32 = 0x5407_b540;
pub const DATA_SIZE: u32 = 0x3a080;

pub fn file_offset(va: u32) -> usize {
    if (DATA_VMA..DATA_VMA + DATA_SIZE).contains(&va) {
        (va - DATA_VMA + DATA_LMA - BIAS) as usize
    } else {
        va.wrapping_sub(BIAS) as usize
    }
}

pub fn u16_at(d: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([d[off], d[off + 1]])
}

pub fn u32_at(d: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(d[off..off + 4].try_into().unwrap())
}

pub fn i16_at(d: &[u8], off: usize) -> i16 {
    u16_at(d, off) as i16
}

pub fn i32_at(d: &[u8], off: usize) -> i32 {
    u32_at(d, off) as i32
}

pub fn put_u16(d: &mut [u8], off: usize, v: u16) {
    d[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

pub fn put_u32(d: &mut [u8], off: usize, v: u32) {
    d[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// the u32 stored at RTOS address `va`
pub fn word(d: &[u8], va: u32) -> u32 {
    u32_at(d, file_offset(va))
}

/// 256 x u16 at `va`
pub fn curve_at(d: &[u8], va: u32) -> Vec<u16> {
    let o = file_offset(va);
    (0..256).map(|i| u16_at(d, o + 2 * i)).collect()
}

pub fn put_curve(d: &mut [u8], va: u32, vals: &[u16]) {
    let o = file_offset(va);
    for (i, v) in vals.iter().enumerate() {
        put_u16(d, o + 2 * i, *v);
    }
}

/// Python's round(): half to even
pub fn round_even(x: f64) -> f64 {
    x.round_ties_even()
}

/// value clamped into a u16 the way the Python editor writes curves
pub fn clamp_u16(v: f64) -> u16 {
    round_even(v).clamp(0.0, 65535.0) as u16
}
