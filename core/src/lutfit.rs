//! The LUT fitter (docs/plan-spektrafilm-fitter.md): a look for one image
//! control -- look matrix, RGB curves, HSL and the post matrix P -- fitted
//! to a 3D LUT. The LUT is a spektrafilm film (tools/spektrafilm-lut.py) or
//! any `.cube` the user has.
//!
//! The model is the camera's, on the core's own encodings, for scene colour
//! L (linear sRGB after the sensor CCM):
//!
//!     x = M L              look matrix (Q13)
//!     x = HSL(x)           MultiAxial (multiaxial.rs): 12 sectors while
//!                          fitting, then the encoded lane and banks
//!     t = x * scale / 16   index into the tone curve (18 % grey at sRGB 0.46)
//!     o = T_c(t) / 16384   T_c = chan::compose(Standard's tone curve, C_c)
//!     o = P o              post matrix (postmatrix.rs), as the registers give it
//!
//! o is the sRGB-encoded JPEG value. The target is the LUT on L, by the
//! LUT's input space (`Input`). The fit is least squares on per-sample
//! ΔE2000 (Levenberg-Marquardt with bounds), as tools/spektrafilm-fit.py.

use crate::addr::round_even;
use crate::chan;
use crate::gamma::{Engine, ToneParams};
use crate::looks;
use crate::multiaxial::{self, Band, Encoded, Hsl};
use crate::postmatrix;
use crate::session::look_sets;
use serde_json::{json, Value};

pub(crate) type M3 = [f64; 9];

/// image controls a LUT can be fitted for: their source is Standard. Bleach
/// Bypass (P on three tables), Cross Process (Color Tone curves) and HDR Tone
/// (effects) are left out.
pub const TARGETS: [&str; 5] = ["Standard", "Vivid", "PositiveFilm", "Retro", "NegativeFilm"];

// ---- the LUT ---------------------------------------------------------------------

/// what a LUT expects as input
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    /// sRGB-encoded image: the LUT on the camera's Standard rendering
    Display,
    /// linear sRGB, in the LUT's domain (clamped to it)
    Linear,
    /// u = (log2(L / 0.18) - lo) / span (tools/spektrafilm-lut.py: -8, 12)
    GrLog { lo: f64, span: f64 },
}

impl Input {
    /// "display", "linear", "grlog" or "grlog LO SPAN"
    pub fn parse(s: &str) -> Result<Input, String> {
        let w: Vec<&str> = s.split_whitespace().collect();
        match w.as_slice() {
            ["display"] => Ok(Input::Display),
            ["linear"] => Ok(Input::Linear),
            ["grlog"] => Ok(Input::GrLog { lo: -8.0, span: 12.0 }),
            ["grlog", lo, span] => {
                let (lo, span) = (lo.parse::<f64>(), span.parse::<f64>());
                match (lo, span) {
                    (Ok(lo), Ok(span)) if lo.is_finite() && span.is_finite() && span > 0.0 => Ok(Input::GrLog { lo, span }),
                    _ => Err(format!("input {s:?}: grlog needs two numbers, the span above 0")),
                }
            }
            _ => Err(format!("input {s:?}: display, linear or grlog")),
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Input::Display => json!("display"),
            Input::Linear => json!("linear"),
            Input::GrLog { lo, span } => json!(format!("grlog {lo} {span}")),
        }
    }
}

/// a 3D LUT from a .cube file
#[derive(Clone, Debug)]
pub struct Lut {
    pub size: usize,
    /// output per grid point, R fastest: index r + n (g + n b)
    pub data: Vec<[f64; 3]>,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub title: String,
    /// from a `# GR-INPUT:` comment, if there is one
    pub input: Option<Input>,
}

pub const MAX_SIZE: usize = 65;

fn floats(s: &str, n: usize, what: &str) -> Result<Vec<f64>, String> {
    let v: Vec<f64> = s.split_whitespace().map(|x| x.parse::<f64>()).collect::<Result<_, _>>()
        .map_err(|_| format!("{what}: not a number in {s:?}"))?;
    if v.len() != n || v.iter().any(|x| !x.is_finite()) {
        return Err(format!("{what}: need {n} numbers, got {s:?}"));
    }
    Ok(v)
}

/// a .cube (Resolve / Adobe): LUT_3D_SIZE, DOMAIN_MIN / MAX (or
/// LUT_3D_INPUT_RANGE), TITLE, comments; 1D LUTs are refused
pub fn parse_cube(text: &str) -> Result<Lut, String> {
    let mut size = None;
    let (mut min, mut max) = ([0.0; 3], [1.0; 3]);
    let mut title = String::new();
    let mut input = None;
    let mut data: Vec<[f64; 3]> = vec![];
    for (no, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let at = |e: String| format!("line {}: {e}", no + 1);
        if line.is_empty() {
            continue;
        }
        if let Some(c) = line.strip_prefix('#') {
            if let Some(v) = c.trim().strip_prefix("GR-INPUT:") {
                input = Some(Input::parse(v.trim()).map_err(at)?);
            }
            continue;
        }
        let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match key {
            "TITLE" => title = rest.trim_matches('"').to_string(),
            "LUT_3D_SIZE" => {
                let n: usize = rest.parse().map_err(|_| at(format!("LUT_3D_SIZE {rest:?}")))?;
                if !(2..=MAX_SIZE).contains(&n) {
                    return Err(at(format!("LUT_3D_SIZE {n}: need 2 .. {MAX_SIZE}")));
                }
                size = Some(n);
            }
            "LUT_1D_SIZE" => return Err("a 1D LUT: the fitter needs a 3D LUT".into()),
            "DOMAIN_MIN" => min = floats(rest, 3, "DOMAIN_MIN").map_err(at)?.try_into().unwrap(),
            "DOMAIN_MAX" => max = floats(rest, 3, "DOMAIN_MAX").map_err(at)?.try_into().unwrap(),
            "LUT_3D_INPUT_RANGE" => {
                let v = floats(rest, 2, "LUT_3D_INPUT_RANGE").map_err(at)?;
                (min, max) = ([v[0]; 3], [v[1]; 3]);
            }
            k if k.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.') => {
                let v = floats(line, 3, "value").map_err(at)?;
                data.push([v[0], v[1], v[2]]);
            }
            // other keywords (LUT_1D_INPUT_RANGE, vendor extensions) carry nothing for us
            _ => {}
        }
    }
    let n = size.ok_or("no LUT_3D_SIZE: not a 3D .cube")?;
    if data.len() != n * n * n {
        return Err(format!("LUT_3D_SIZE {n} needs {} values, the file has {}", n * n * n, data.len()));
    }
    if (0..3).any(|c| max[c] <= min[c]) {
        return Err("DOMAIN_MAX must lie above DOMAIN_MIN".into());
    }
    Ok(Lut { size: n, data, min, max, title, input })
}

impl Lut {
    /// tetrahedral lookup, `u` in the LUT's domain (clamped to it)
    pub fn lookup(&self, u: [f64; 3]) -> [f64; 3] {
        let n = self.size;
        let mut i = [0usize; 3];
        let mut f = [0.0; 3];
        for c in 0..3 {
            let x = ((u[c] - self.min[c]) / (self.max[c] - self.min[c])).clamp(0.0, 1.0) * (n - 1) as f64;
            i[c] = (x as usize).min(n - 2);
            f[c] = x - i[c] as f64;
        }
        let at = |d: [usize; 3]| self.data[(i[0] + d[0]) + n * ((i[1] + d[1]) + n * (i[2] + d[2]))];
        // axes by falling fraction: the tetrahedron 000 -> a -> a+b -> 111
        let mut ax = [0usize, 1, 2];
        ax.sort_by(|&a, &b| f[b].total_cmp(&f[a]));
        let mut d = [0usize; 3];
        let v0 = at(d);
        d[ax[0]] = 1;
        let v1 = at(d);
        d[ax[1]] = 1;
        let v2 = at(d);
        let v3 = at([1, 1, 1]);
        let (f1, f2, f3) = (f[ax[0]], f[ax[1]], f[ax[2]]);
        std::array::from_fn(|c| (1.0 - f1) * v0[c] + (f1 - f2) * v1[c] + (f2 - f3) * v2[c] + f3 * v3[c])
    }
}

// ---- colour ----------------------------------------------------------------------

pub(crate) fn srgb_decode(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

pub(crate) fn srgb_encode(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

const RGB_XYZ: M3 = [0.4124, 0.3576, 0.1805, 0.2126, 0.7152, 0.0722, 0.0193, 0.1192, 0.9505];
const WHITE: [f64; 3] = [0.950_455_93, 1.0, 1.089_057_75];

/// sRGB-encoded -> CIELAB (D65)
pub fn lab(enc: [f64; 3]) -> [f64; 3] {
    let lin = enc.map(srgb_decode);
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let xyz: [f64; 3] = std::array::from_fn(|r| (0..3).map(|c| RGB_XYZ[3 * r + c] * lin[c]).sum::<f64>() / WHITE[r]);
    let (fx, fy, fz) = (f(xyz[0]), f(xyz[1]), f(xyz[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000
pub fn de2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (l1, a1, b1) = (a[0], a[1], a[2]);
    let (l2, a2, b2) = (b[0], b[1], b[2]);
    let c_bar = (a1.hypot(b1) + a2.hypot(b2)) / 2.0;
    let c7 = c_bar.powi(7);
    let g = 0.5 * (1.0 - (c7 / (c7 + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| if b == 0.0 && a == 0.0 { 0.0 } else { b.atan2(a).to_degrees().rem_euclid(360.0) };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dhh = 2.0 * (c1p * c2p).sqrt() * (dh / 2.0).to_radians().sin();
    let l_bar = (l1 + l2) / 2.0;
    let cp_bar = (c1p + c2p) / 2.0;
    let hp_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hp_bar - 30.0).to_radians().cos() + 0.24 * (2.0 * hp_bar).to_radians().cos()
        + 0.32 * (3.0 * hp_bar + 6.0).to_radians().cos() - 0.20 * (4.0 * hp_bar - 63.0).to_radians().cos();
    let d_theta = 30.0 * (-((hp_bar - 275.0) / 25.0).powi(2)).exp();
    let cp7 = cp_bar.powi(7);
    let rc = 2.0 * (cp7 / (cp7 + 25f64.powi(7))).sqrt();
    let l50 = (l_bar - 50.0).powi(2);
    let sl = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let sc = 1.0 + 0.045 * cp_bar;
    let sh = 1.0 + 0.015 * cp_bar * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;
    let (x, y, z) = (dl / sl, dc / sc, dhh / sh);
    (x * x + y * y + z * z + rt * y * z).sqrt()
}

fn mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|n| (0..3).map(|k| a[3 * (n / 3) + k] * b[3 * k + n % 3]).sum())
}

pub(crate) fn apply(m: &M3, v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|r| m[3 * r] * v[0] + m[3 * r + 1] * v[1] + m[3 * r + 2] * v[2])
}

pub(crate) fn inverse(m: &M3) -> M3 {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det,
     (f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det,
     (d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det]
}

pub(crate) const IDENTITY: M3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

/// 6 values -> 3x3 with rows summing to 1 (off-diagonals free)
fn rows_sum1(v: &[f64]) -> M3 {
    let mut m = [0.0; 9];
    (m[1], m[2], m[3], m[5], m[6], m[7]) = (v[0], v[1], v[2], v[3], v[4], v[5]);
    for r in 0..3 {
        m[4 * r] = 1.0 - (0..3).filter(|&c| c != r).map(|c| m[3 * r + c]).sum::<f64>();
    }
    m
}

fn off_diagonals(m: &M3) -> [f64; 6] {
    [m[1], m[2], m[3], m[5], m[6], m[7]]
}

// ---- the camera ------------------------------------------------------------------

/// what the model needs from the firmware, for one target
pub struct Camera<'a> {
    img: &'a [u8],
    table: u32,
    pub target: String,
    /// Standard's tone curve at default sliders, 256 x 0..16383
    pub tone: Vec<u16>,
    /// RGB -> YCbCr of the HSL stage
    pub ycc: M3,
    /// Standard's look matrix (for display-referred LUTs)
    pub standard: M3,
    /// linear -> 12-bit tone input, so that 18 % grey lands at sRGB 0.46
    pub scale: f64,
    /// fitted for the camera's highlight correction (docs/plan-highlight-correction.md)
    pub hc: bool,
    /// EV of input headroom over the default camera (0 without HC, ~1 with):
    /// the samples reach that much further into the highlights
    pub extra_ev: f64,
}

/// GammaTuning gain of the model: the default, and with highlight correction,
/// which reads the sensor out one stop lower and halves the gain (MR029426-432)
pub const GAIN: i32 = 0x1800;
pub const GAIN_HC: i32 = 0x0c00;

/// Standard's tone curve at default sliders and `gain`, and the tone index
/// (fractional) where it reaches sRGB 0.46
fn tone_at(img: &[u8], gain: i32) -> Result<(Vec<u16>, f64), String> {
    let p = ToneParams { key: 0, contrast: 0, highlight: 0, shadow: 0, gain, flag: 0 };
    let tone = Engine::new(img, &[]).curve(img, p)?;
    let want = 0.46 * 16384.0;
    let k = tone.windows(2).position(|w| w[1] as f64 >= want).ok_or("tone curve never reaches mid grey")?;
    let (a, b) = (tone[k] as f64, tone[k + 1] as f64);
    let mid = k as f64 + if b > a { (want - a) / (b - a) } else { 0.0 };
    Ok((tone, mid))
}

impl<'a> Camera<'a> {
    pub fn new(img: &'a [u8], target: &str) -> Result<Camera<'a>, String> {
        Camera::with_hc(img, target, false)
    }

    /// the camera with or without its highlight correction
    pub fn with_hc(img: &'a [u8], target: &str, hc: bool) -> Result<Camera<'a>, String> {
        if !TARGETS.contains(&target) {
            return Err(format!("a LUT can be fitted for {}, not {target:?}", TARGETS.join(", ")));
        }
        let table = looks::table_of(target).ok_or(format!("unknown look {target:?}"))?;
        let (tone, mid) = tone_at(img, if hc { GAIN_HC } else { GAIN })?;
        let extra_ev = if hc { (tone_at(img, GAIN)?.1 / mid).log2() } else { 0.0 };
        let standard: Vec<f64> = looks::looks(img).into_iter()
            .find(|l| l["name"] == "Standard")
            .and_then(|l| l["matrix"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect()))
            .ok_or("Standard's matrix not found")?;
        Ok(Camera {
            img,
            table,
            target: target.to_string(),
            tone,
            ycc: multiaxial::ycc(img, table),
            standard: std::array::from_fn(|k| standard[k]),
            scale: mid * 16.0 / 0.18,
            hc,
            extra_ev,
        })
    }

    /// P as the camera applies it at hue 0: B^-1 times the registers
    pub fn quantise_post(&self, p: &M3) -> Result<M3, String> {
        let s = postmatrix::encode(self.img, self.table, p)?;
        let base = postmatrix::base(self.img);
        let reg = postmatrix::registers(&s[postmatrix::STEPS / 2], &base);
        Ok(mul(&inverse(&base.map(|v| v as f64)), &reg.map(|v| v as f64)))
    }

    /// the 256-entry table of one channel: the tone curve through C
    fn table_from(&self, c: impl Fn(f64) -> f64) -> [f64; 256] {
        std::array::from_fn(|i| c(self.tone[i] as f64 / 64.0))
    }

    /// the same, as the camera computes it from a stored curve
    fn table_exact(&self, curve: &[u16]) -> [f64; 256] {
        let c: Vec<i64> = curve.iter().map(|&v| v as i64).collect();
        let t = chan::compose(&self.tone, &c);
        std::array::from_fn(|i| t[i] as f64)
    }
}

/// the colour stages with one set of values, ready to render samples
struct Stages<'c> {
    m: M3,
    hsl: HslStage<'c>,
    tables: [[f64; 256]; 3],
    post: M3,
}

enum HslStage<'c> {
    None,
    /// one matrix per sample (sector or bank chosen per round)
    PerSample { mats: Vec<M3>, which: &'c [usize], weight: Option<[u16; multiaxial::CONTROL_FIELDS]> },
    /// the encoded stage as the camera picks banks
    Encoded(&'c Encoded),
}

impl Stages<'_> {
    fn render(&self, cam: &Camera, i: usize, l: [f64; 3]) -> [f64; 3] {
        let mut x = apply(&self.m, l);
        match &self.hsl {
            HslStage::None => {}
            HslStage::PerSample { mats, which, weight } => {
                let m = &mats[which[i]];
                let w = weight.map_or(1.0, |c| {
                    let y = cam.ycc[0] * x[0] + cam.ycc[1] * x[1] + cam.ycc[2] * x[2];
                    multiaxial::weight(&c, (4096.0 * y).clamp(0.0, 4095.0))
                });
                let v = apply(m, x);
                x = std::array::from_fn(|r| (x[r] + w * (v[r] - x[r])).max(0.0));
            }
            HslStage::Encoded(e) => x = e.apply(&cam.ycc, x),
        }
        let mut o = [0.0; 3];
        for c in 0..3 {
            let t = (x[c] * cam.scale).clamp(0.0, 4095.0) / 16.0;
            let k = (t as usize).min(254);
            let f = t - k as f64;
            o[c] = (self.tables[c][k] * (1.0 - f) + self.tables[c][k + 1] * f) / 16384.0;
        }
        apply(&self.post, o).map(|v| v.clamp(0.0, 1.0))
    }
}

/// hue coordinate of a linear RGB value on the HSL stage's Cb/Cr plane
fn hue_of(cam: &Camera, x: [f64; 3]) -> f64 {
    let ycc = apply(&cam.ycc, x);
    multiaxial::coordinate(ycc[1], ycc[2])
}

// ---- samples ---------------------------------------------------------------------

/// ColorChecker 24 (after November 2014), linear sRGB (D65, Bradford), as
/// colour-science gives it
const CHECKER: [[f64; 3]; 24] = [
    [0.173552, 0.078740, 0.053261], [0.559462, 0.277344, 0.211948], [0.105091, 0.189552, 0.326939],
    [0.105064, 0.150213, 0.052210], [0.228860, 0.213500, 0.423468], [0.114492, 0.506633, 0.412294],
    [0.744991, 0.201721, 0.032517], [0.060618, 0.102593, 0.383731], [0.560558, 0.080721, 0.114323],
    [0.109831, 0.042541, 0.136827], [0.329676, 0.494956, 0.048865], [0.768979, 0.356555, 0.025343],
    [0.022508, 0.048705, 0.280817], [0.044436, 0.290683, 0.064583], [0.446369, 0.036763, 0.040679],
    [0.838030, 0.571753, 0.012731], [0.523925, 0.079249, 0.286564], [0.000000, 0.234158, 0.375062],
    [0.879191, 0.884767, 0.834953], [0.584440, 0.592124, 0.584582], [0.357678, 0.367060, 0.365287],
    [0.190087, 0.190860, 0.189828], [0.085935, 0.088738, 0.089788], [0.031360, 0.031500, 0.032311],
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tag {
    Grey,
    Chart,
    Random,
    Sweep,
}

/// SplitMix64: the sample sets must be the same on every platform
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
    fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i64 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// scene colours for the fit: a grey ramp, the ColorChecker at 5 exposures,
/// random colours and a hue sweep (as tools/spektrafilm-fit.py)
pub struct Samples {
    l: Vec<[f64; 3]>,
    tag: Vec<Tag>,
    /// the ColorChecker at the middle exposure
    chart: std::ops::Range<usize>,
}

impl Samples {
    pub fn new(seed: u64, random: usize, sweep: usize) -> Samples {
        Samples::with_extra(seed, random, sweep, 0.0)
    }

    /// the same, with the grey ramp and the random colours `extra` EV
    /// further into the highlights (a camera with more input headroom)
    pub fn with_extra(seed: u64, random: usize, sweep: usize, extra: f64) -> Samples {
        let mut rng = Rng(seed);
        let off = rng.uniform(-0.25, 0.25);
        let mut l = vec![];
        let mut tag = vec![];
        for k in 0..120 {
            let ev = -6.0 + (9.3 + extra) * k as f64 / 119.0 + off;
            l.push([0.18 * 2f64.powf(ev); 3]);
            tag.push(Tag::Grey);
        }
        let chart_start = l.len() + 2 * 24;
        for e in [-2.0, -1.0, 0.0, 1.0, 2.0] {
            let g = 2f64.powf(e + off);
            for c in CHECKER {
                l.push(c.map(|v| v * g));
                tag.push(Tag::Chart);
            }
        }
        for _ in 0..random {
            let c: [f64; 3] = std::array::from_fn(|_| rng.next().powf(2.2));
            let mx = c.iter().cloned().fold(0.0, f64::max).max(1e-3);
            let g = 0.18 * 2f64.powf(rng.uniform(-4.0, 2.2 + extra)) * 1.3 / mx;
            l.push(c.map(|v| v * g));
            tag.push(Tag::Random);
        }
        for _ in 0..sweep {
            let (h, s) = (rng.next(), rng.uniform(0.2, 0.9));
            let c = hsv_to_rgb(h, s, 1.0).map(srgb_decode);
            let mx = c.iter().cloned().fold(0.0, f64::max).max(1e-6);
            let g = 0.18 * 2f64.powf(rng.uniform(-2.0, 1.5)) * 1.5 / mx;
            l.push(c.map(|v| v * g));
            tag.push(Tag::Sweep);
        }
        Samples { l, tag, chart: chart_start..chart_start + 24 }
    }

    pub fn len(&self) -> usize {
        self.l.len()
    }

    pub fn is_empty(&self) -> bool {
        self.l.is_empty()
    }

    fn weights(&self) -> Vec<f64> {
        self.tag.iter().map(|t| match t { Tag::Grey => 25.0, Tag::Chart => 6.0, _ => 1.0 }).collect()
    }
}

/// the LUT's answer for every sample, as CIELAB
fn target_lab(cam: &Camera, lut: &Lut, input: Input, s: &Samples) -> Vec<[f64; 3]> {
    let standard = Stages {
        m: cam.standard,
        hsl: HslStage::None,
        tables: [cam.table_exact(&chan::linear()); 3],
        post: IDENTITY,
    };
    s.l.iter().enumerate().map(|(i, &l)| {
        let u = match input {
            Input::Display => standard.render(cam, i, l),
            Input::Linear => l,
            Input::GrLog { lo, span } => l.map(|v| (((v.max(1e-12) / 0.18).log2() - lo) / span).clamp(0.0, 1.0)),
        };
        // in the LUT's domain units (lookup clamps to the domain)
        lab(lut.lookup(u).map(|v| v.clamp(0.0, 1.0)))
    }).collect()
}

// ---- the parameters --------------------------------------------------------------

pub const SECTORS: usize = 12;
const SECTOR: f64 = multiaxial::CIRCLE as f64 / SECTORS as f64;
/// curve knots over C's domain (tone >> 6): 0, 8, .., 256
const NK: usize = 33;
const KNOT_STEP: f64 = 8.0;
/// sector bounds: hue in degrees, saturation and luminance in % (editor units)
const HSL_LIMIT: [f64; 3] = [30.0, 90.0, 90.0];

/// which parts are free in a pass; the others are fixed
#[derive(Clone, Copy)]
struct Layout {
    matrix: bool,
    hsl: bool,
    post: bool,
}

/// the look while it is fitted
#[derive(Clone)]
struct Look {
    m: M3,
    /// per channel: log steps of the curve knots and, last, the log headroom
    /// above the top knot; knot k = 16383 * (sum of exp up to k) / (sum of all)
    u: [[f64; NK + 1]; 3],
    /// per sector: hue, sat, lum
    sectors: Option<[[f64; 3]; SECTORS]>,
    post: M3,
}

impl Look {
    fn knots(&self, c: usize) -> [f64; NK] {
        // never above 16383, and every knot keeps its gradient (a hard cap
        // at white stalls the fit there)
        let e = self.u[c].map(f64::exp);
        let total: f64 = e.iter().sum();
        let mut acc = 0.0;
        std::array::from_fn(|k| {
            acc += e[k];
            16383.0 * acc / total
        })
    }

    /// C at z = tone / 64
    fn curve(knots: &[f64; NK], z: f64) -> f64 {
        let x = (z / KNOT_STEP).clamp(0.0, (NK - 1) as f64);
        let k = (x as usize).min(NK - 2);
        let f = x - k as f64;
        knots[k] * (1.0 - f) + knots[k + 1] * f
    }

    /// the stored curve: C[k] for k = 0..255
    fn curve_u16(&self, c: usize) -> Vec<u16> {
        let kn = self.knots(c);
        (0..256).map(|k| Look::curve(&kn, k as f64).round().clamp(0.0, 16383.0) as u16).collect()
    }

    fn get(&self, lay: Layout) -> Vec<f64> {
        let mut p = vec![];
        if lay.matrix {
            p.extend(off_diagonals(&self.m));
        }
        for c in 0..3 {
            p.extend(self.u[c]);
        }
        if lay.hsl {
            for s in self.sectors.unwrap() {
                p.extend(s);
            }
        }
        if lay.post {
            p.extend(off_diagonals(&self.post));
        }
        p
    }

    fn set(&mut self, lay: Layout, p: &[f64]) {
        let mut i = 0;
        if lay.matrix {
            self.m = rows_sum1(&p[0..6]);
            i = 6;
        }
        for c in 0..3 {
            self.u[c].copy_from_slice(&p[i..i + NK + 1]);
            i += NK + 1;
        }
        if lay.hsl {
            let mut s = [[0.0; 3]; SECTORS];
            for (k, v) in s.iter_mut().enumerate() {
                v.copy_from_slice(&p[i + 3 * k..i + 3 * k + 3]);
            }
            self.sectors = Some(s);
            i += 3 * SECTORS;
        }
        if lay.post {
            self.post = rows_sum1(&p[i..i + 6]);
        }
    }

    fn bounds(&self, lay: Layout) -> (Vec<f64>, Vec<f64>) {
        let (mut lo, mut hi) = (vec![], vec![]);
        let mut push = |n: usize, a: f64, b: f64| {
            lo.extend(std::iter::repeat_n(a, n));
            hi.extend(std::iter::repeat_n(b, n));
        };
        if lay.matrix {
            push(6, -1.5, 1.5);
        }
        push(3 * (NK + 1), -6.0, 10.0);
        if lay.hsl {
            for _ in 0..SECTORS {
                for lim in HSL_LIMIT {
                    push(1, -lim, lim);
                }
            }
        }
        if lay.post {
            push(6, -0.6, 0.6);
        }
        (lo, hi)
    }

    fn sector_matrices(&self, cam: &Camera) -> Vec<M3> {
        self.sectors.map_or(vec![], |s| {
            s.iter().enumerate().map(|(k, v)| {
                let b = Band { center: (k as f64 + 0.5) * SECTOR, hue: v[0], sat: v[1], lum: v[2], ..Default::default() };
                multiaxial::band_matrix(&cam.ycc, &b, 1.0)
            }).collect()
        })
    }
}

/// the HSL the look has in a pass
enum HslMode<'e> {
    None,
    Sectors,
    Encoded(&'e Encoded),
}

// ---- least squares ---------------------------------------------------------------

/// Cholesky solve of a symmetric positive definite n x n system
fn solve(a: &[f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let s: f64 = a[i * n + j] - (0..j).map(|k| l[i * n + k] * l[j * n + k]).sum::<f64>();
            if i == j {
                if s <= 0.0 {
                    return None;
                }
                l[i * n + i] = s.sqrt();
            } else {
                l[i * n + j] = s / l[j * n + j];
            }
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        y[i] = (b[i] - (0..i).map(|k| l[i * n + k] * y[k]).sum::<f64>()) / l[i * n + i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        x[i] = (y[i] - (i + 1..n).map(|k| l[k * n + i] * x[k]).sum::<f64>()) / l[i * n + i];
    }
    Some(x)
}

/// Levenberg-Marquardt on residuals f(p), p kept within [lo, hi];
/// at most `max_eval` evaluations of f besides the Jacobians
fn least_squares(f: &dyn Fn(&[f64]) -> Vec<f64>, p0: &[f64], lo: &[f64], hi: &[f64], max_eval: usize) -> Vec<f64> {
    let n = p0.len();
    let clamp = |p: &mut Vec<f64>| {
        for k in 0..n {
            p[k] = p[k].clamp(lo[k], hi[k]);
        }
    };
    let mut p = p0.to_vec();
    clamp(&mut p);
    let mut r = f(&p);
    let cost = |r: &[f64]| r.iter().map(|v| v * v).sum::<f64>();
    let mut c = cost(&r);
    let mut lambda = 1e-3;
    let mut evals = 1;
    while evals < max_eval {
        // forward differences
        let m = r.len();
        let mut jac = vec![0.0; m * n];
        for k in 0..n {
            let mut h = 1.5e-8 * p[k].abs().max(1.0);
            if p[k] + h > hi[k] {
                h = -h;
            }
            let mut q = p.clone();
            q[k] += h;
            let rq = f(&q);
            for i in 0..m {
                jac[i * n + k] = (rq[i] - r[i]) / h;
            }
        }
        let mut a = vec![0.0; n * n];
        let mut g = vec![0.0; n];
        for i in 0..m {
            let row = &jac[i * n..(i + 1) * n];
            for j in 0..n {
                if row[j] == 0.0 {
                    continue;
                }
                g[j] += row[j] * r[i];
                for k in 0..=j {
                    a[j * n + k] += row[j] * row[k];
                }
            }
        }
        for j in 0..n {
            for k in 0..j {
                a[k * n + j] = a[j * n + k];
            }
        }
        let mut improved = false;
        while evals < max_eval && lambda < 1e12 {
            let mut d = a.clone();
            for j in 0..n {
                d[j * n + j] += lambda * a[j * n + j].max(1e-9);
            }
            let neg: Vec<f64> = g.iter().map(|v| -v).collect();
            let Some(step) = solve(&d, &neg, n) else {
                lambda *= 4.0;
                continue;
            };
            let mut q: Vec<f64> = p.iter().zip(&step).map(|(a, b)| a + b).collect();
            clamp(&mut q);
            let rq = f(&q);
            evals += 1;
            let cq = cost(&rq);
            if cq < c {
                let gain = (c - cq) / c.max(1e-300);
                (p, r, c) = (q, rq, cq);
                lambda = (lambda / 3.0).max(1e-12);
                improved = gain > 1e-9;
                break;
            }
            lambda *= 4.0;
        }
        if !improved {
            break;
        }
    }
    p
}

// ---- the fit ---------------------------------------------------------------------

/// what to fit
#[derive(Clone, Debug)]
pub struct Options {
    pub post: bool,
    pub hsl: bool,
    /// the LUT's input; None: the file's `GR-INPUT`, else display
    pub input: Option<Input>,
    /// random colours and hue-sweep colours per sample set
    pub random: usize,
    pub sweep: usize,
    /// least-squares evaluations per round
    pub max_eval: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options { post: true, hsl: true, input: None, random: 1200, sweep: 800, max_eval: 60 }
    }
}

/// a fitted look
#[derive(Clone, Debug)]
pub struct Fit {
    pub target: String,
    pub input: Input,
    /// look matrix, Q13 as it is stored
    pub matrix: M3,
    /// R, G, B channel curves, 256 x 0..16383
    pub curves: [Vec<u16>; 3],
    /// the editor's HSL ({curves: flat, base: the fit, protect}), None without HSL
    pub hsl: Option<Value>,
    /// P, rows summing to 1, None without
    pub post: Option<M3>,
    pub report: Value,
}

struct Ctx<'a, 'c> {
    cam: &'a Camera<'c>,
    s: &'a Samples,
    lab: &'a [[f64; 3]],
    w: Vec<f64>,
}

impl Ctx<'_, '_> {
    fn stages<'e>(&self, look: &Look, mode: &HslMode<'e>, which: &'e [usize], exact: Option<&[Vec<u16>; 3]>) -> Stages<'e> {
        let tables = match exact {
            Some(c) => std::array::from_fn(|k| self.cam.table_exact(&c[k])),
            None => std::array::from_fn(|k| {
                let kn = look.knots(k);
                self.cam.table_from(|z| Look::curve(&kn, z))
            }),
        };
        let hsl = match mode {
            HslMode::None => HslStage::None,
            HslMode::Sectors => HslStage::PerSample { mats: look.sector_matrices(self.cam), which, weight: None },
            HslMode::Encoded(e) => {
                if which.is_empty() {
                    HslStage::Encoded(e)
                } else {
                    let mats = e.banks.iter().map(|b| b.map(|v| v as f64 / 1024.0)).collect();
                    HslStage::PerSample { mats, which, weight: Some(e.control) }
                }
            }
        };
        Stages { m: look.m, hsl, tables, post: look.post }
    }

    fn de(&self, st: &Stages) -> Vec<f64> {
        self.s.l.iter().enumerate().map(|(i, &l)| de2000(lab(st.render(self.cam, i, l)), self.lab[i])).collect()
    }

    /// sector or bank of every sample under the look's matrix
    fn which(&self, look: &Look, mode: &HslMode) -> Vec<usize> {
        self.s.l.iter().map(|&l| {
            let g = hue_of(self.cam, apply(&look.m, l));
            match mode {
                HslMode::Encoded(e) => e.bank_at(g),
                _ => ((g / SECTOR) as usize).min(SECTORS - 1),
            }
        }).collect()
    }

    /// `rounds` rounds of least squares on the parts `lay` frees; the hue
    /// sector (bank) of each sample is fixed within a round
    fn fit(&self, look: &mut Look, lay: Layout, mode: &HslMode, rounds: usize, max_eval: usize) {
        for _ in 0..rounds {
            let which = match mode {
                HslMode::None => vec![],
                _ => self.which(look, mode),
            };
            let (lo, hi) = look.bounds(lay);
            let base = look.clone();
            let f = |p: &[f64]| -> Vec<f64> {
                let mut lk = base.clone();
                lk.set(lay, p);
                let st = self.stages(&lk, mode, &which, None);
                self.de(&st).iter().zip(&self.w).map(|(d, w)| w.sqrt() * (d * d + 1e-4).sqrt()).collect()
            };
            let p = least_squares(&f, &look.get(lay), &lo, &hi, max_eval);
            look.set(lay, &p);
        }
    }
}

/// ΔE2000 summary: all, p95, ColorChecker, skin, warm, saturated, grey
fn metrics(de: &[f64], s: &Samples, lab_t: &[[f64; 3]]) -> Value {
    let mean = |sel: &dyn Fn(usize) -> bool| -> f64 {
        let v: Vec<f64> = (0..de.len()).filter(|&i| sel(i)).map(|i| de[i]).collect();
        if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
    };
    let mut sorted = de.to_vec();
    sorted.sort_by(f64::total_cmp);
    let pos = 0.95 * (sorted.len() - 1) as f64;
    let (k, f) = (pos as usize, pos.fract());
    let p95 = sorted[k] * (1.0 - f) + sorted[(k + 1).min(sorted.len() - 1)] * f;
    let lch = |i: usize| {
        let [_, a, b] = lab_t[i];
        (a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0))
    };
    let r = |v: f64| (v * 1000.0).round() / 1000.0;
    json!({
        "all": r(mean(&|_| true)),
        "p95": r(p95),
        "chart": r(mean(&|i| s.chart.contains(&i))),
        "skin": r(mean(&|i| i == s.chart.start || i == s.chart.start + 1)),
        "warm": r(mean(&|i| { let (c, h) = lch(i); c > 20.0 && h > 30.0 && h < 100.0 })),
        "saturated": r(mean(&|i| lch(i).0 > 40.0)),
        "grey": r(mean(&|i| s.tag[i] == Tag::Grey)),
    })
}

/// the editor's HSL for sector values: a plateau per sector as the base
/// (multiaxial::Hsl::base), so the core's encoding finds the sector edges as
/// steps, and the user's curves stay flat to edit on top
fn hsl_curves(sectors: &[[f64; 3]; SECTORS]) -> Value {
    let pts = |ch: usize| -> Vec<[f64; 2]> {
        let mut v = vec![];
        for (k, s) in sectors.iter().enumerate() {
            let val = (s[ch] * 10.0).round() / 10.0;
            v.push([(k as f64 * SECTOR + 8.0).round(), val]);
            v.push([((k + 1) as f64 * SECTOR - 8.0).round(), val]);
        }
        v
    };
    json!({
        "curves": { "hue": [], "sat": [], "lum": [] },
        "base": { "hue": pts(0), "sat": pts(1), "lum": pts(2) },
        "protect": { "shadows": 0, "highlights": 4096, "highlight_weight": 1 },
    })
}

/// the encoded HSL for sector values; too strong values are scaled down
/// until the stage takes them
fn encode_hsl(cam: &Camera, sectors: &[[f64; 3]; SECTORS]) -> Result<(Value, Encoded, f64), String> {
    let mut k = 1.0;
    loop {
        let s = sectors.map(|v| v.map(|x| x * k));
        let v = hsl_curves(&s);
        match multiaxial::encode(&Hsl::from_json(&v), &cam.ycc) {
            Ok(e) => return Ok((v, e, k)),
            Err(_) if k > 0.3 => k *= 0.85,
            Err(e) => return Err(format!("HSL: {e}")),
        }
    }
}

/// fit `lut` for the camera's target; `progress` hears each pass
pub fn fit(cam: &Camera, lut: &Lut, opt: &Options, progress: &mut dyn FnMut(&str)) -> Result<Fit, String> {
    let input = opt.input.or(lut.input).unwrap_or(Input::Display);
    let train = Samples::with_extra(1, opt.random, opt.sweep, cam.extra_ev);
    let held = Samples::with_extra(2, opt.random, opt.sweep, cam.extra_ev);
    let lab_t = target_lab(cam, lut, input, &train);
    let lab_v = target_lab(cam, lut, input, &held);
    let ctx = Ctx { cam, s: &train, lab: &lab_t, w: train.weights() };
    let check = Ctx { cam, s: &held, lab: &lab_v, w: held.weights() };

    // start: identity matrix, curves that put grey where the LUT does
    let mut look = Look { m: IDENTITY, u: [[0.0; NK + 1]; 3], sectors: None, post: IDENTITY };
    let grey: Vec<usize> = (0..train.len()).filter(|&i| train.tag[i] == Tag::Grey).collect();
    for c in 0..3 {
        let mut pts: Vec<(f64, f64)> = grey.iter().map(|&i| {
            let t = (train.l[i][1] * cam.scale).clamp(0.0, 4095.0) / 16.0;
            let k = (t as usize).min(254);
            let z = (cam.tone[k] as f64 * (1.0 - t.fract()) + cam.tone[k + 1] as f64 * t.fract()) / 64.0;
            // the LUT's grey back to sRGB-encoded, this channel
            let enc = lab_to_srgb(lab_t[i])[c] * 16384.0;
            (z, enc)
        }).collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut run = 0.0;
        for p in pts.iter_mut() {
            run = f64::max(run, p.1);
            p.1 = run;
        }
        // rising knots below white, then steps and headroom as logs
        let mut prev = 0.0;
        for k in 0..NK {
            let want = interp(&pts, k as f64 * KNOT_STEP).clamp(prev + 1.0, 16383.0 - (NK - k) as f64);
            look.u[c][k] = (want - prev).ln();
            prev = want;
        }
        look.u[c][NK] = (16383.0 - prev).ln();
    }
    let lay_a = Layout { matrix: true, hsl: false, post: false };
    progress("matrix and curves");
    ctx.fit(&mut look, lay_a, &HslMode::None, 2, opt.max_eval);
    let mut report = json!({
        "target": cam.target, "lut": lut.title, "lut_size": lut.size, "input": input.to_json(),
        "options": { "post": opt.post, "hsl": opt.hsl },
        "samples": train.len(),
    });
    let none: Vec<usize> = vec![];
    report["matrix_curves"] = metrics(&check.de(&check.stages(&look, &HslMode::None, &none, None)), &held, &lab_v);

    if opt.hsl || opt.post {
        progress("HSL and post matrix");
        if opt.hsl {
            look.sectors = Some([[0.0; 3]; SECTORS]);
        }
        let lay = Layout { matrix: true, hsl: opt.hsl, post: opt.post };
        let mode = if opt.hsl { HslMode::Sectors } else { HslMode::None };
        ctx.fit(&mut look, lay, &mode, 2, opt.max_eval);
        let which = check.which(&look, &mode);
        report["model"] = metrics(&check.de(&check.stages(&look, &mode, &which, None)), &held, &lab_v);
    }

    // HSL as the camera gets it; matrix, curves and P follow it
    let mut hsl_json = None;
    let encoded = match look.sectors {
        Some(s) if opt.hsl => {
            progress("HSL encoded");
            let (v, e, k) = encode_hsl(cam, &s)?;
            if k < 1.0 {
                report["hsl_scaled"] = json!((k * 1000.0).round() / 1000.0);
            }
            report["hsl_notes"] = json!(e.notes);
            hsl_json = Some(v);
            Some(e)
        }
        _ => None,
    };
    let mode = encoded.as_ref().map_or(HslMode::None, HslMode::Encoded);
    if encoded.is_some() {
        ctx.fit(&mut look, Layout { matrix: true, hsl: false, post: opt.post }, &mode, 1, opt.max_eval);
    }
    // P as the registers give it, then matrix and curves once more
    let post = if opt.post {
        let p = look.post;
        look.post = cam.quantise_post(&p)?;
        ctx.fit(&mut look, Layout { matrix: true, hsl: false, post: false }, &mode, 1, opt.max_eval);
        Some(p)
    } else {
        None
    };
    // the stored values: Q13 matrix, u16 curves; scored as the camera computes
    look.m = look.m.map(|v| round_even(v * looks::SCALE) / looks::SCALE);
    let curves: [Vec<u16>; 3] = std::array::from_fn(|c| look.curve_u16(c));
    let st = check.stages(&look, &mode, &none, Some(&curves));
    report["final"] = metrics(&check.de(&st), &held, &lab_v);
    report["train_final"] = metrics(&ctx.de(&ctx.stages(&look, &mode, &none, Some(&curves))), &train, &lab_t);
    Ok(Fit { target: cam.target.clone(), input, matrix: look.m, curves, hsl: hsl_json, post, report })
}

fn interp(pts: &[(f64, f64)], x: f64) -> f64 {
    if pts.is_empty() {
        return 0.0;
    }
    if x <= pts[0].0 {
        return pts[0].1;
    }
    for w in pts.windows(2) {
        if x <= w[1].0 {
            let d = w[1].0 - w[0].0;
            return if d > 0.0 { w[0].1 + (w[1].1 - w[0].1) * (x - w[0].0) / d } else { w[1].1 };
        }
    }
    pts[pts.len() - 1].1
}

/// CIELAB -> sRGB-encoded (for the start values only)
fn lab_to_srgb(lab: [f64; 3]) -> [f64; 3] {
    let fy = (lab[0] + 16.0) / 116.0;
    let (fx, fz) = (fy + lab[1] / 500.0, fy - lab[2] / 200.0);
    let inv = |f: f64| if f.powi(3) > 216.0 / 24389.0 { f.powi(3) } else { (116.0 * f - 16.0) * 27.0 / 24389.0 };
    let xyz = [inv(fx) * WHITE[0], inv(fy) * WHITE[1], inv(fz) * WHITE[2]];
    apply(&inverse(&RGB_XYZ), xyz).map(srgb_encode)
}

/// a fitted look as the camera renders it (stored values, encoded HSL,
/// P as the registers give it at hue 0)
pub struct Renderer<'a, 'c> {
    cam: &'a Camera<'c>,
    m: M3,
    hsl: Option<Encoded>,
    tables: [[f64; 256]; 3],
    post: M3,
}

impl Renderer<'_, '_> {
    /// scene-linear sRGB in, sRGB-encoded out
    pub fn render(&self, l: &[[f64; 3]]) -> Vec<[f64; 3]> {
        let hsl = self.hsl.as_ref().map_or(HslStage::None, HslStage::Encoded);
        let st = Stages { m: self.m, hsl, tables: self.tables, post: self.post };
        l.iter().enumerate().map(|(i, &v)| st.render(self.cam, i, v)).collect()
    }
}

impl Fit {
    /// a target as a v2 preset holds it (`spec`), back as a Fit: its own
    /// matrix (else `default_matrix`), curves (else linear), HSL and P
    pub fn from_spec(target: &str, spec: &Value, default_matrix: M3) -> Result<Fit, String> {
        let nums = |v: &Value| -> Option<Vec<f64>> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()) };
        let matrix = match nums(&spec["matrices"][target]) {
            Some(m) if m.len() == 9 => std::array::from_fn(|k| m[k]),
            Some(_) => return Err(format!("{target}: a matrix needs nine values")),
            None => default_matrix,
        };
        let curves: [Vec<u16>; 3] = match spec["curves"].as_array() {
            Some(c) if c.len() == 3 => {
                let mut out: [Vec<u16>; 3] = Default::default();
                for (k, v) in c.iter().enumerate() {
                    let vals = nums(v).filter(|v| v.len() == 256).ok_or(format!("{target}: a curve needs 256 values"))?;
                    out[k] = vals.iter().map(|&x| x.round().clamp(0.0, 65535.0) as u16).collect();
                }
                out
            }
            _ => [chan::linear(), chan::linear(), chan::linear()],
        };
        let post = match nums(&spec["post"]) {
            Some(p) if p.len() == 9 => Some(std::array::from_fn(|k| p[k])),
            _ => None,
        };
        let hsl = spec.get("hsl").filter(|h| !h.is_null()).cloned();
        Ok(Fit { target: target.into(), input: Input::Display, matrix, curves, hsl, post, report: Value::Null })
    }

    pub fn renderer<'a, 'c>(&self, cam: &'a Camera<'c>) -> Result<Renderer<'a, 'c>, String> {
        let hsl = match &self.hsl {
            Some(v) => Some(multiaxial::encode(&Hsl::from_json(v), &cam.ycc)?),
            None => None,
        };
        let post = match &self.post {
            Some(p) => cam.quantise_post(p)?,
            None => IDENTITY,
        };
        let tables = std::array::from_fn(|c| cam.table_exact(&self.curves[c]));
        Ok(Renderer { cam, m: self.matrix, hsl, tables, post })
    }

    /// the target as a v2 preset holds it (source Standard); `lut` describes
    /// where it came from and goes to `edit.lut`
    pub fn spec(&self, lut: Value) -> Value {
        let sets = look_sets().find(|(n, _)| *n == self.target).map(|(_, s)| s).unwrap();
        let ident = json!([[0, 0], [1, 1]]);
        let edits: serde_json::Map<String, Value> = sets.iter().zip(&self.curves)
            .map(|(i, c)| (i.to_string(), json!({ "base": c, "points": ident })))
            .collect();
        let neutral = json!({ "hue": 0, "sat": 0 });
        let mut lut = lut;
        lut["input"] = self.input.to_json();
        lut["report"] = self.report.clone();
        let mut spec = json!({
            "source": "Standard",
            "effects_off": false,
            "matrices": Value::Object([(self.target.clone(), json!(self.matrix))].into_iter().collect()),
            "curves": self.curves,
            "edit": {
                "matrix": { "base": self.matrix, "sat": 1, "hue": 0, "calib": { "r": neutral, "g": neutral, "b": neutral } },
                "ext": null,
                "channel_edits": edits,
                "grade": null,
                "lut": lut,
            },
        });
        if let Some(h) = &self.hsl {
            spec["hsl"] = h.clone();
        }
        if let Some(p) = &self.post {
            spec["post"] = json!(p);
        }
        spec
    }
}
