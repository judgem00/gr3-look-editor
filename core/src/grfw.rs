//! The GR III firmware container (FWDC*.BIN): parse, decode, verify, rebuild.
//! A port of fwpack/grfw.py; see docs/firmware-packing.md for the format.
//!
//!     0x00   header, 0x80 bytes (+0x30 project, +0x78 project twice, +0x7c magic)
//!     0x80   compressed stream: frames, then a 0x0000 end marker, padding to 4
//!     end-0x18 footer: project twice, magic, type, stream length, decoded length, checksum
//!
//! Both checksums are "sum of little-endian u32 words == 0".

use crate::addr::{put_u32, u32_at};

pub const MAGIC: u32 = 0xA55A_5AA5;
const HEADER_SIZE: usize = 0x80;
const FOOTER_SIZE: usize = 0x18;
const LITERAL_MAX: usize = 0x7FFF;
/// every frame of the original decodes to 0x6000 bytes; new literal frames
/// keep to that grid so the camera's decoder sees nothing unusual
const FRAME_OUTPUT: usize = 0x6000;

#[derive(Clone, Debug)]
pub struct Frame {
    pub pos: usize,
    pub size: usize,
    pub literal: bool,
    pub out_start: usize,
    pub out_end: usize,
    /// lowest output position any copy reads from
    pub src_min: Option<usize>,
}

pub struct Container {
    pub data: Vec<u8>,
    pub project: u32,
    pub footer: [u32; 6],
    pub frames: Vec<Frame>,
}

pub fn word_sum(d: &[u8]) -> u32 {
    d.chunks_exact(4)
        .fold(0u32, |s, w| s.wrapping_add(u32::from_le_bytes(w.try_into().unwrap())))
}

impl Container {
    pub fn parse(data: Vec<u8>) -> Result<Container, String> {
        if data.len() < HEADER_SIZE + FOOTER_SIZE || &data[..5] != b"RICOH" {
            return Err("not a RICOH firmware file".into());
        }
        if data.len() % 4 != 0 {
            return Err("length is not a multiple of 4".into());
        }
        let f0 = data.len() - FOOTER_SIZE;
        let mut footer = [0u32; 6];
        for (i, v) in footer.iter_mut().enumerate() {
            *v = u32_at(&data, f0 + 4 * i);
        }
        let project = u32_at(&data, 0x30);
        let mut c = Container { data, project, footer, frames: vec![] };
        c.frames = c.parse_frames()?;
        Ok(c)
    }

    pub fn fw_type(&self) -> u32 {
        self.footer[2]
    }

    pub fn decoded_len(&self) -> usize {
        self.footer[4] as usize
    }

    fn parse_frames(&self) -> Result<Vec<Frame>, String> {
        let d = &self.data;
        let mut frames = vec![];
        let mut p = HEADER_SIZE;
        let end = HEADER_SIZE + self.footer[3] as usize - 2;
        if end + 2 > d.len() {
            return Err("stream length runs past the file".into());
        }
        loop {
            let prefix = u16::from_be_bytes([d[p], d[p + 1]]) as usize;
            if prefix == 0 {
                break;
            }
            let fr = Frame {
                pos: p, size: prefix & 0x7FFF, literal: prefix & 0x8000 != 0,
                out_start: 0, out_end: 0, src_min: None,
            };
            p += 2 + fr.size;
            frames.push(fr);
            if p > end {
                return Err(format!("frame at {:#x} runs past the stream", frames.last().unwrap().pos));
            }
        }
        if p != end {
            return Err(format!("end marker at {p:#x}, footer says {end:#x}"));
        }
        Ok(frames)
    }

    /// problems with header, footer and file checksum, in the order FwDecoder
    /// checks them (empty = ok)
    pub fn check(&self, expected_project: Option<u32>, expected_type: Option<u32>) -> Vec<String> {
        let mut problems = vec![];
        let h = &self.data[..HEADER_SIZE];
        let f = &self.footer;
        let proj = expected_project.unwrap_or(self.project);
        if self.project != proj {
            problems.push(format!("header project number {:#x} != {proj:#x}", self.project));
        }
        let twice = proj | (proj << 16);
        if u32_at(h, 0x78) != twice {
            problems.push("header +0x78 is not the project number twice".into());
        }
        if u32_at(h, 0x7C) != MAGIC {
            problems.push("header magic missing".into());
        }
        if f[0] != twice {
            problems.push("footer project number mismatch".into());
        }
        if f[1] != MAGIC {
            problems.push("footer magic missing".into());
        }
        if let Some(t) = expected_type {
            if f[2] != t {
                problems.push(format!("footer firmware type {} != {t}", f[2]));
            }
        }
        if f[5] != 0xFFFF_FFFF && word_sum(&self.data) != 0 {
            problems.push(format!("file checksum wrong (word sum {:#010x})", word_sum(&self.data)));
        }
        problems
    }

    /// the decoded image; also fills in each frame's output range and the
    /// lowest position its copies read from
    pub fn decode(&mut self) -> Result<Vec<u8>, String> {
        let d = &self.data;
        let mut out: Vec<u8> = Vec::with_capacity(self.decoded_len());
        for fr in self.frames.iter_mut() {
            fr.out_start = out.len();
            let (mut p, end) = (fr.pos + 2, fr.pos + 2 + fr.size);
            if fr.literal {
                out.extend_from_slice(&d[p..end]);
            } else {
                let mut src_min: Option<usize> = None;
                let mut done = false;
                while p < end && !done {
                    let flags = u16::from_be_bytes([d[p], d[p + 1]]);
                    p += 2;
                    for bit in (0..16).rev() {
                        if p >= end {
                            break;
                        }
                        if (flags >> bit) & 1 == 0 {
                            out.push(d[p]);
                            p += 1;
                            continue;
                        }
                        let (b0, b1) = (d[p] as usize, d[p + 1] as usize);
                        p += 2;
                        let mut length = b0 & 7;
                        let offset = ((b0 & 0xF8) << 5) | b1;
                        if length == 7 {
                            length += d[p] as usize;
                            p += 1;
                            if length == 7 + 0xFF {
                                let mut nxt = 0xFF;
                                while nxt == 0xFF {
                                    nxt = d[p] as usize;
                                    p += 1;
                                    length += nxt;
                                }
                            }
                        }
                        if offset == 0 {
                            done = true;
                            break;
                        }
                        if offset > out.len() {
                            return Err(format!("copy before start of output in frame at {:#x}", fr.pos));
                        }
                        let start = out.len() - offset;
                        let n = length + 3;
                        src_min = Some(src_min.map_or(start, |m| m.min(start)));
                        // byte by byte: an overlapping copy repeats the pattern
                        for k in 0..n {
                            let b = out[start + k];
                            out.push(b);
                        }
                    }
                }
                if p != end {
                    return Err(format!("frame at {:#x} decoded {} of {} bytes", fr.pos, p - fr.pos - 2, fr.size));
                }
                fr.src_min = src_min;
            }
            fr.out_end = out.len();
        }
        Ok(out)
    }
}

/// the image with its last word set so its word sum is zero
pub fn fix_image_checksum(image: &mut [u8]) {
    let n = image.len();
    put_u32(image, n - 4, 0);
    let s = word_sum(image);
    put_u32(image, n - 4, s.wrapping_neg());
}

fn literal_frames(chunk: &[u8], out: &mut Vec<u8>) {
    let step = if chunk.len() <= LITERAL_MAX { chunk.len() } else { FRAME_OUTPUT };
    for part in chunk.chunks(step.max(1)) {
        out.extend_from_slice(&((0x8000 | part.len()) as u16).to_be_bytes());
        out.extend_from_slice(part);
    }
}

/// A firmware file for `image`, built from `original` (already decoded once,
/// `base` = its image) with as little change as possible: frames are copied
/// byte for byte unless their output differs, or they copy from bytes that
/// differ -- those are re-emitted as literal frames. Returns (bytes, number of
/// re-encoded frames).
pub fn repack(original: &Container, base: &[u8], image: &[u8]) -> Result<(Vec<u8>, usize), String> {
    if image.len() != original.decoded_len() {
        return Err(format!("image is {} bytes, the container holds {}; size changes are not supported",
                           image.len(), original.decoded_len()));
    }
    if base.len() != original.decoded_len() {
        return Err("original does not decode to the length its footer states".into());
    }
    // changed bytes as sorted, disjoint [start, end) runs; an edit touches few
    let mut runs: Vec<(usize, usize)> = vec![];
    for (i, (a, b)) in base.iter().zip(image).enumerate() {
        if a != b {
            match runs.last_mut() {
                Some(r) if r.1 == i => r.1 = i + 1,
                _ => runs.push((i, i + 1)),
            }
        }
    }
    // does any changed byte fall into [lo, hi)?
    let dirty = |lo: usize, hi: usize| {
        let k = runs.partition_point(|r| r.1 <= lo);
        k < runs.len() && runs[k].0 < hi
    };

    let mut stream = Vec::with_capacity(original.data.len() + 0x10000);
    let mut redo = 0;
    for fr in &original.frames {
        let again = dirty(fr.out_start, fr.out_end)
            // conservative: everything between its lowest source and its end
            || fr.src_min.is_some_and(|m| dirty(m, fr.out_end));
        if again {
            redo += 1;
            literal_frames(&image[fr.out_start..fr.out_end], &mut stream);
        } else {
            stream.extend_from_slice(&original.data[fr.pos..fr.pos + 2 + fr.size]);
        }
    }
    stream.extend_from_slice(&[0, 0]);
    let stream_len = stream.len();
    while (HEADER_SIZE + stream.len()) % 4 != 0 {
        stream.push(0);
    }
    let mut body = Vec::with_capacity(HEADER_SIZE + stream.len() + FOOTER_SIZE);
    body.extend_from_slice(&original.data[..HEADER_SIZE]);
    body.extend_from_slice(&stream);
    for v in [original.footer[0], original.footer[1], original.footer[2], stream_len as u32, image.len() as u32, 0] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    let n = body.len();
    let sum = word_sum(&body);
    put_u32(&mut body, n - 4, sum.wrapping_neg());
    Ok((body, redo))
}
