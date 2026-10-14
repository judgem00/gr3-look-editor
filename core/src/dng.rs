//! GR III DNGs, decoded into what the preview needs (ported from the
//! earlier Python editor, which used LibRaw through rawpy).
//!
//! The raw data is a 14-bit lossless JPEG. The linear image is computed as
//! LibRaw's half-size postprocess does it, which the Python editor used:
//! black subtracted, scaled to 16 bit by 65535 / (white - black) in float32
//! (white lowered to the data maximum if that lies above 75 % of it), one
//! pixel per 2x2 Bayer block with G = (G1 + G2) >> 1, then the default crop.
//! Verified bit for bit against rawpy on GR III files.

pub const PREVIEW_LONG_EDGE: usize = 1600;
/// order of the int16 values in makernote tag 0x0247
const PARAM_ORDER: [&str; 7] = ["saturation", "hue", "key", "contrast", "highlight", "shadow", "sharpness"];
const NOT_SET: i16 = -0x8000;

// ---- TIFF -------------------------------------------------------------------

struct Tiff<'a> {
    d: &'a [u8],
    le: bool,
}

#[derive(Clone, Debug)]
struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    /// file offset of the value
    at: usize,
}

type Ifd = Vec<Entry>;

fn type_size(t: u16) -> usize {
    match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    }
}

impl<'a> Tiff<'a> {
    fn u16(&self, o: usize) -> Result<u16, String> {
        let b = self.d.get(o..o + 2).ok_or("truncated DNG")?;
        Ok(if self.le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    }

    fn u32(&self, o: usize) -> Result<u32, String> {
        let b: [u8; 4] = self.d.get(o..o + 4).ok_or("truncated DNG")?.try_into().unwrap();
        Ok(if self.le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }

    /// entries of the IFD at `o`; offsets are relative to `base`
    fn ifd(&self, o: usize, base: usize) -> Result<Ifd, String> {
        let n = self.u16(o)? as usize;
        let mut out = Vec::with_capacity(n);
        for k in 0..n {
            let e = o + 2 + 12 * k;
            let (tag, typ, count) = (self.u16(e)?, self.u16(e + 2)?, self.u32(e + 4)?);
            let size = type_size(typ) * count as usize;
            let at = if size <= 4 { e + 8 } else { base + self.u32(e + 8)? as usize };
            out.push(Entry { tag, typ, count, at });
        }
        Ok(out)
    }

    /// value k of an entry as a number
    fn num(&self, e: &Entry, k: usize) -> Result<f64, String> {
        let s = type_size(e.typ);
        let o = e.at + s * k;
        Ok(match e.typ {
            1 | 7 => *self.d.get(o).ok_or("truncated DNG")? as f64,
            3 => self.u16(o)? as f64,
            8 => self.u16(o)? as i16 as f64,
            4 | 13 => self.u32(o)? as f64,
            9 => self.u32(o)? as i32 as f64,
            5 => self.u32(o)? as f64 / self.u32(o + 4)? as f64,
            10 => self.u32(o)? as i32 as f64 / self.u32(o + 4)? as i32 as f64,
            11 => f32::from_bits(self.u32(o)?) as f64,
            12 => {
                let b: [u8; 8] = self.d[o..o + 8].try_into().unwrap();
                if self.le { f64::from_le_bytes(b) } else { f64::from_be_bytes(b) }
            }
            _ => return Err(format!("tag {:#x}: unsupported type {}", e.tag, e.typ)),
        })
    }

    fn nums(&self, e: &Entry) -> Result<Vec<f64>, String> {
        (0..e.count as usize).map(|k| self.num(e, k)).collect()
    }

    fn bytes(&self, e: &Entry) -> &'a [u8] {
        let n = type_size(e.typ) * e.count as usize;
        self.d.get(e.at..e.at + n).unwrap_or(&[])
    }
}

fn find(ifd: &Ifd, tag: u16) -> Option<&Entry> {
    ifd.iter().find(|e| e.tag == tag)
}

// ---- lossless JPEG (ITU T.81 process 14) ------------------------------------------

struct Huffman {
    /// (code length, code) -> symbol, as canonical tables
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    vals: Vec<u8>,
    /// fast path: 9-bit prefix -> (length, symbol), length 0 = slow path
    fast: Vec<(u8, u8)>,
}

impl Huffman {
    fn new(counts: &[u8; 16], vals: Vec<u8>) -> Huffman {
        let mut h = Huffman { maxcode: [-1; 18], valptr: [0; 17], mincode: [0; 17], vals, fast: vec![(0, 0); 512] };
        let (mut code, mut k) = (0i32, 0i32);
        for len in 1..=16 {
            let n = counts[len - 1] as i32;
            if n > 0 {
                h.valptr[len] = k;
                h.mincode[len] = code;
                for i in 0..n {
                    if len <= 9 {
                        let c = (code + i) << (9 - len);
                        for f in 0..(1 << (9 - len)) {
                            h.fast[(c + f) as usize] = (len as u8, h.vals[(k + i) as usize]);
                        }
                    }
                }
                code += n;
                k += n;
                h.maxcode[len] = code - 1;
            }
            code <<= 1;
        }
        h.maxcode[17] = i32::MAX;
        h
    }
}

struct Bits<'a> {
    d: &'a [u8],
    p: usize,
    acc: u64,
    n: u32,
}

impl Bits<'_> {
    fn fill(&mut self) {
        while self.n <= 56 {
            let mut b = 0u8;
            if self.p < self.d.len() {
                b = self.d[self.p];
                if b == 0xFF {
                    let next = self.d.get(self.p + 1).copied().unwrap_or(0);
                    if next == 0 {
                        self.p += 2;
                    } else {
                        b = 0; // a marker: feed zeros from here on
                    }
                } else {
                    self.p += 1;
                }
            }
            self.acc |= (b as u64) << (56 - self.n);
            self.n += 8;
        }
    }

    fn peek(&mut self, k: u32) -> u32 {
        if self.n < k {
            self.fill();
        }
        (self.acc >> (64 - k)) as u32
    }

    fn skip(&mut self, k: u32) {
        self.acc <<= k;
        self.n -= k;
    }

    fn get(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        let v = self.peek(k);
        self.skip(k);
        v
    }

    fn symbol(&mut self, h: &Huffman) -> Result<u8, String> {
        let (len, sym) = h.fast[self.peek(9) as usize];
        if len > 0 {
            self.skip(len as u32);
            return Ok(sym);
        }
        let mut code = self.get(1) as i32;
        let mut len = 1;
        while code > h.maxcode[len] {
            code = (code << 1) | self.get(1) as i32;
            len += 1;
            if len > 16 {
                return Err("bad Huffman code in raw data".into());
            }
        }
        Ok(h.vals[(h.valptr[len] + code - h.mincode[len]) as usize])
    }
}

/// decode one lossless JPEG; returns (width in samples, height, samples)
fn ljpeg(d: &[u8]) -> Result<(usize, usize, Vec<u16>), String> {
    if d.get(..2) != Some(&[0xFF, 0xD8]) {
        return Err("raw data is not a JPEG stream".into());
    }
    let mut p = 2;
    let mut tables: Vec<Option<Huffman>> = (0..4).map(|_| None).collect();
    let (mut prec, mut height, mut width, mut comps) = (0u32, 0usize, 0usize, 0usize);
    let mut comp_ids: Vec<u8> = vec![];
    loop {
        if p + 4 > d.len() || d[p] != 0xFF {
            return Err("bad JPEG marker in raw data".into());
        }
        let m = d[p + 1];
        let len = u16::from_be_bytes([d[p + 2], d[p + 3]]) as usize;
        let seg = d.get(p + 4..p + 2 + len).ok_or("truncated raw data")?;
        match m {
            0xC3 => {
                prec = seg[0] as u32;
                height = u16::from_be_bytes([seg[1], seg[2]]) as usize;
                width = u16::from_be_bytes([seg[3], seg[4]]) as usize;
                comps = seg[5] as usize;
                comp_ids = (0..comps).map(|i| seg[6 + 3 * i]).collect();
            }
            0xC4 => {
                let mut q = 0;
                while q < seg.len() {
                    let id = (seg[q] & 0x0F) as usize;
                    let counts: [u8; 16] = seg[q + 1..q + 17].try_into().unwrap();
                    let n: usize = counts.iter().map(|&c| c as usize).sum();
                    let vals = seg[q + 17..q + 17 + n].to_vec();
                    tables[id & 3] = Some(Huffman::new(&counts, vals));
                    q += 17 + n;
                }
            }
            0xC0..=0xC2 | 0xC5..=0xCF if m != 0xC8 && m != 0xCC => {
                return Err("raw data is not lossless JPEG".into());
            }
            0xDA => {
                let ns = seg[0] as usize;
                let mut tsel = vec![0usize; comps];
                for i in 0..ns {
                    let id = seg[1 + 2 * i];
                    let ci = comp_ids.iter().position(|&c| c == id).ok_or("bad scan component")?;
                    tsel[ci] = (seg[2 + 2 * i] >> 4) as usize;
                }
                let pred = seg[1 + 2 * ns];
                let pt = (seg[3 + 2 * ns] & 0x0F) as u32;
                if ns != comps || !(1..=7).contains(&pred) {
                    return Err("unsupported lossless JPEG layout".into());
                }
                let hs: Vec<&Huffman> = tsel.iter().map(|&t| tables[t].as_ref().ok_or("missing Huffman table")).collect::<Result<_, _>>()?;
                let mut bits = Bits { d, p: p + 2 + len, acc: 0, n: 0 };
                let row = width * comps;
                let mut out = vec![0u16; row * height];
                let initial = 1i32 << (prec - pt - 1);
                for y in 0..height {
                    for x in 0..width {
                        for c in 0..comps {
                            let h = hs[c];
                            let t = bits.symbol(h)? as u32;
                            let diff = match t {
                                0 => 0,
                                16 => 32768,
                                _ => {
                                    let v = bits.get(t) as i32;
                                    if v < 1 << (t - 1) { v - (1 << t) + 1 } else { v }
                                }
                            };
                            let i = y * row + x * comps + c;
                            let px = if y == 0 && x == 0 {
                                initial
                            } else if y == 0 {
                                out[i - comps] as i32
                            } else if x == 0 {
                                out[i - row] as i32
                            } else {
                                let (ra, rb, rc) = (out[i - comps] as i32, out[i - row] as i32, out[i - row - comps] as i32);
                                match pred {
                                    1 => ra,
                                    2 => rb,
                                    3 => rc,
                                    4 => ra + rb - rc,
                                    5 => ra + ((rb - rc) >> 1),
                                    6 => rb + ((ra - rc) >> 1),
                                    _ => (ra + rb) >> 1,
                                }
                            };
                            out[i] = (px + diff) as u16;
                        }
                    }
                }
                return Ok((row, height, out));
            }
            _ => {}
        }
        p += 2 + len;
    }
}

// ---- the shot ------------------------------------------------------------------

pub struct Shot {
    /// linear camera RGB, white = 1.0, no white balance; default crop,
    /// downscaled, not yet oriented
    pub linear: Vec<f32>,
    pub width: usize,
    pub height: usize,
    /// LibRaw flip code: 3 = 180, 5 = 90 ccw, 6 = 90 cw
    pub flip: u32,
    pub wb: [f64; 3],
    pub model: Option<String>,
    pub image_tone: Option<u16>,
    pub params: Vec<(&'static str, i32)>,
    /// makernote 0x0079, second byte: shadow correction level (tonecontrol.rs)
    pub shadow_correction: Option<u8>,
    /// file range of the largest embedded JPEG
    pub jpeg: Option<(usize, usize)>,
}

fn ascii(t: &Tiff, e: &Entry) -> String {
    let b = t.bytes(e);
    String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(&[])).trim().to_string()
}

pub fn decode(d: &[u8]) -> Result<Shot, String> {
    let le = match d.get(..4) {
        Some(b"II*\0") => true,
        Some(b"MM\0*") => false,
        _ => return Err("not a DNG (TIFF) file".into()),
    };
    let t = Tiff { d, le };
    let ifd0 = t.ifd(t.u32(4)? as usize, 0)?;
    let mut ifds = vec![ifd0.clone()];
    if let Some(e) = find(&ifd0, 0x14A) {
        for k in 0..e.count as usize {
            ifds.push(t.ifd(t.num(e, k)? as usize, 0)?);
        }
    }
    let raw = ifds
        .iter()
        .find(|i| find(i, 0xFE).map(|e| t.num(e, 0).unwrap_or(1.0)) == Some(0.0) && find(i, 0x106).map(|e| t.num(e, 0).unwrap_or(0.0)) == Some(32803.0))
        .ok_or("no raw (CFA) image in this DNG")?;
    let get = |tag: u16| -> Result<Vec<f64>, String> { find(raw, tag).map(|e| t.nums(e)).transpose().map(|v| v.unwrap_or_default()) };
    if get(0x103)?.first() != Some(&7.0) {
        return Err("only lossless-JPEG compressed DNGs are supported".into());
    }
    if find(raw, 0xC618).is_some() {
        return Err("DNGs with a linearization table are not supported".into());
    }
    let (rw, rh) = (get(0x100)?[0] as usize, get(0x101)?[0] as usize);
    // the raw samples, from strips or tiles
    let mut samples = vec![0u16; rw * rh];
    let (offs, counts, tw, th) = if let (Some(o), Some(c)) = (find(raw, 0x144), find(raw, 0x145)) {
        (t.nums(o)?, t.nums(c)?, get(0x142)?[0] as usize, get(0x143)?[0] as usize)
    } else {
        let rps = get(0x116)?.first().map(|v| *v as usize).unwrap_or(rh);
        (get(0x111)?, get(0x117)?, rw, rps)
    };
    let across = rw.div_ceil(tw);
    for (k, (&o, &c)) in offs.iter().zip(&counts).enumerate() {
        let seg = d.get(o as usize..(o + c) as usize).ok_or("raw data runs past the file")?;
        let (sw, sh, px) = ljpeg(seg)?;
        let (x0, y0) = ((k % across) * tw, (k / across) * th);
        for y in 0..sh.min(rh.saturating_sub(y0)) {
            let n = sw.min(rw - x0);
            samples[(y0 + y) * rw + x0..(y0 + y) * rw + x0 + n].copy_from_slice(&px[y * sw..y * sw + n]);
        }
    }
    let area = get(0xC68D)?; // top, left, bottom, right
    let (top, left, bottom, right) = if area.len() == 4 {
        (area[0] as usize, area[1] as usize, area[2] as usize, area[3] as usize)
    } else {
        (0, 0, rh, rw)
    };
    let cfa = find(raw, 0x828E).map(|e| t.bytes(e).to_vec()).unwrap_or_default();
    if cfa != [0, 1, 1, 2] {
        return Err(format!("unsupported CFA pattern {cfa:?} (expected RGGB)"));
    }
    let black = get(0xC61A)?;
    if black.iter().any(|&b| b != black[0]) {
        return Err("per-channel black levels are not supported".into());
    }
    let black = black.first().copied().unwrap_or(0.0) as i32;
    let white = get(0xC61D)?.first().copied().unwrap_or(65535.0) as i32;
    let (vh, vw) = (bottom - top, right - left);

    // LibRaw: black subtracted, data maximum over the visible area
    let px = |r: usize, c: usize| (samples[(top + r) * rw + left + c] as i32 - black).max(0);
    let mut dmax = 0;
    for r in 0..vh {
        for c in 0..vw {
            dmax = dmax.max(px(r, c));
        }
    }
    let mut maximum = white - black;
    if dmax > 0 && dmax < maximum && dmax as f64 > maximum as f64 * 0.75 {
        maximum = dmax;
    }
    let scale = 65535f32 / maximum as f32;
    let sc = |v: i32| ((v as f32 * scale) as i64).min(65535) as u16;

    // default crop in half-size coordinates
    let origin = get(0xC61F)?;
    let size = get(0xC620)?;
    let (cx, cy) = (origin.first().copied().unwrap_or(0.0) as usize, origin.get(1).copied().unwrap_or(0.0) as usize);
    let (cw, ch) = (size.first().map(|v| *v as usize).unwrap_or(vw), size.get(1).map(|v| *v as usize).unwrap_or(vh));
    let (hy0, hx0, hh, hw) = (cy / 2, cx / 2, ch / 2, cw / 2);
    let (ih, iw) = (vh.div_ceil(2), vw.div_ceil(2));
    if hy0 + hh > ih || hx0 + hw > iw {
        return Err("default crop lies outside the image".into());
    }
    let mut half = vec![0f32; hh * hw * 3];
    for y in 0..hh {
        for x in 0..hw {
            let (r, c) = (2 * (hy0 + y), 2 * (hx0 + x));
            let g = (sc(px(r, c + 1)) as u32 + sc(px(r + 1, c)) as u32) >> 1;
            let o = (y * hw + x) * 3;
            half[o] = sc(px(r, c)) as f32 / 65535f32;
            half[o + 1] = g as f32 / 65535f32;
            half[o + 2] = sc(px(r + 1, c + 1)) as f32 / 65535f32;
        }
    }
    let factor = ((hh.max(hw) as f64 / PREVIEW_LONG_EDGE as f64).round_ties_even() as usize).max(1);
    let (linear, width, height) = downscale(&half, hw, hh, factor);

    // white balance: LibRaw's cam_mul = 1 / AsShotNeutral, normalised to green
    let neutral = find(&ifd0, 0xC628).map(|e| t.nums(e)).transpose()?.unwrap_or_else(|| vec![1.0, 1.0, 1.0]);
    let mul: Vec<f32> = neutral.iter().map(|&v| (1.0 / v) as f32).collect();
    let wb = [mul[0] as f64 / mul[1] as f64, 1.0, mul[2] as f64 / mul[1] as f64];

    let flip = match find(&ifd0, 0x112).map(|e| t.num(e, 0)).transpose()?.unwrap_or(1.0) as u32 {
        3 => 3,
        6 => 6,
        8 => 5,
        _ => 0,
    };
    let model = find(&ifd0, 0x110).map(|e| ascii(&t, e));

    // makernote (DNGPrivateData: "RICOH\0II" + IFD, offsets from its start)
    let (mut image_tone, mut params, mut shadow_correction) = (None, vec![], None);
    if let Some(e) = find(&ifd0, 0xC634) {
        let base = e.at;
        if d.get(base..base + 8) == Some(b"RICOH\0II") {
            let mt = Tiff { d, le: true };
            if let Ok(mn) = mt.ifd(base + 8, base) {
                image_tone = find(&mn, 0x4F).and_then(|e| mt.num(e, 0).ok()).map(|v| v as u16);
                shadow_correction = find(&mn, 0x79).and_then(|e| mt.bytes(e).get(1).copied());
                if let Some(e) = find(&mn, 0x247) {
                    let raw = mt.bytes(e);
                    let val = |k: usize| i16::from_le_bytes([raw[2 * k], raw[2 * k + 1]]);
                    if raw.len() >= 2 * PARAM_ORDER.len() {
                        for (k, name) in PARAM_ORDER.iter().enumerate() {
                            let v = val(k);
                            if v != NOT_SET && (-4..=4).contains(&v) {
                                params.push((*name, v as i32));
                            }
                        }
                    }
                    // last value: the Cross Process "Farbton" setting 1..3
                    if raw.len() >= 44 && (1..=3).contains(&val(21)) {
                        params.push(("cross_variant", val(21) as i32));
                    }
                }
            }
        }
    }

    // the largest embedded JPEG (what LibRaw's extract_thumb returns)
    let mut jpeg = None;
    let mut best = 0usize;
    for i in &ifds {
        let is_jpeg = find(i, 0x106).and_then(|e| t.num(e, 0).ok()) == Some(6.0)
            || find(i, 0x103).and_then(|e| t.num(e, 0).ok()) == Some(6.0);
        let (o, c) = (find(i, 0x111).or(find(i, 0x201)), find(i, 0x117).or(find(i, 0x202)));
        if let (true, Some(o), Some(c)) = (is_jpeg, o, c) {
            let (o, c) = (t.num(o, 0)? as usize, t.num(c, 0)? as usize);
            if c > best && d.get(o..o + 2) == Some(&[0xFF, 0xD8]) {
                best = c;
                jpeg = Some((o, o + c));
            }
        }
    }

    Ok(Shot { linear, width, height, flip, wb, model, image_tone, params, shadow_correction, jpeg })
}

/// mean over factor x factor blocks
fn downscale(img: &[f32], w: usize, h: usize, f: usize) -> (Vec<f32>, usize, usize) {
    if f <= 1 {
        return (img.to_vec(), w, h);
    }
    let (ow, oh) = (w / f, h / f);
    let mut out = vec![0f32; ow * oh * 3];
    let n = (f * f) as f32;
    for y in 0..oh {
        for x in 0..ow {
            for c in 0..3 {
                let mut s = 0f32;
                for dy in 0..f {
                    for dx in 0..f {
                        s += img[((y * f + dy) * w + x * f + dx) * 3 + c];
                    }
                }
                out[(y * ow + x) * 3 + c] = s / n;
            }
        }
    }
    (out, ow, oh)
}

/// rotate an interleaved image with `ch` channels by LibRaw flip code;
/// returns (pixels, width, height)
pub fn orient<T: Copy + Default>(img: &[T], w: usize, h: usize, ch: usize, flip: u32) -> (Vec<T>, usize, usize) {
    let (ow, oh) = if flip == 5 || flip == 6 { (h, w) } else { (w, h) };
    let mut out = vec![T::default(); img.len()];
    for y in 0..oh {
        for x in 0..ow {
            let (sx, sy) = match flip {
                3 => (w - 1 - x, h - 1 - y),
                5 => (w - 1 - y, x),     // 90 ccw (np.rot90 k=1)
                6 => (y, h - 1 - x),     // 90 cw (np.rot90 k=-1)
                _ => (x, y),
            };
            let (o, s) = ((y * ow + x) * ch, (sy * w + sx) * ch);
            out[o..o + ch].copy_from_slice(&img[s..s + ch]);
        }
    }
    (out, ow, oh)
}
