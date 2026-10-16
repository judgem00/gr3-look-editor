//! GR-Log (docs/plan-gr-log.md): a look whose tone curve stores the scene
//! logarithmically, for video in manual exposure, and the LUTs that take it
//! back to a finished image in post.
//!
//! A look's RGB curve C sits on Standard's tone curve b (`o = C(b >> 6)`).
//! The same curve on all three channels, C = E ∘ b⁻¹, gives
//!
//! ```text
//! o = E(x),   E(x) = (log2(x / 0.18) − lo) / span,  clamped to 0..1
//! ```
//!
//! with x the look's linear RGB (after its matrix), 18 % grey at 0.18, as
//! the fitter's camera model places it (grey at sRGB 0.46 on Standard). The
//! matrix is Standard's, with no HSL and no post matrix, so the chain after
//! the curve is per channel and the post LUTs are exact inverses.

use serde_json::{json, Value};

use crate::lutfit::{self, Camera, Fit, Input, Lut, M3, IDENTITY};
use crate::session::look_sets;

/// the looks a GR-Log target can be: the colour looks the camera offers in
/// movie mode that have curves of their own (Negative Film and HDR Tone
/// cannot be picked in video; Cross Process curves through its Color Tone sets)
pub const TARGETS: [&str; 5] = ["Standard", "Vivid", "PositiveFilm", "BleechBypass_GR", "Retro"];

/// the log encoding: −6 … +4 EV over grey by default
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Encoding {
    pub lo: f64,
    pub span: f64,
}

impl Encoding {
    pub const DEFAULT: Encoding = Encoding { lo: -6.0, span: 10.0 };

    pub fn new(lo: f64, span: f64) -> Result<Encoding, String> {
        if !(lo.is_finite() && span.is_finite() && (-12.0..=0.0).contains(&lo) && (4.0..=20.0).contains(&span)) {
            return Err(format!("a log encoding needs lo in -12..0 and span in 4..20 EV, not {lo} {span}"));
        }
        Ok(Encoding { lo, span })
    }

    /// linear (18 % grey = 0.18) -> code 0..1
    pub fn encode(&self, x: f64) -> f64 {
        (((x.max(1e-12) / 0.18).log2() - self.lo) / self.span).clamp(0.0, 1.0)
    }

    /// code 0..1 -> linear; code 0 is `lo` EV under grey, not black
    pub fn decode(&self, u: f64) -> f64 {
        0.18 * 2f64.powf(self.lo + self.span * u.clamp(0.0, 1.0))
    }

    /// the EV over grey the camera's input reaches with this tone curve
    pub fn top_ev(cam: &Camera) -> f64 {
        (4095.0 / (0.18 * cam.scale)).log2()
    }

    pub fn to_json(&self) -> Value {
        json!({ "lo": self.lo, "span": self.span })
    }
}

/// the camera model a GR-Log target computes with: Standard's tone curve,
/// with or without highlight correction (lutfit::Camera::with_hc)
pub fn camera(img: &[u8], hc: bool) -> Result<Camera<'_>, String> {
    Camera::with_hc(img, "Standard", hc)
}

/// the curve C = E ∘ b⁻¹, 256 × u16 over b >> 6, the same for R, G and B
pub fn curve(cam: &Camera, enc: Encoding) -> Vec<u16> {
    let tone = &cam.tone;
    (0..256).map(|k| {
        let b = (64 * k) as f64;
        // the tone index (fractional) where Standard's curve reaches b;
        // the curve rises until it hits white, then stays there
        let t = match tone.windows(2).position(|w| w[1] as f64 >= b) {
            Some(i) => {
                let (a, c) = (tone[i] as f64, tone[i + 1] as f64);
                i as f64 + if c > a { ((b - a) / (c - a)).clamp(0.0, 1.0) } else { 0.0 }
            }
            None => (tone.len() - 1) as f64,
        };
        let x = t * 16.0 / cam.scale;
        (16383.0 * enc.encode(x)).round() as u16
    }).collect()
}

/// the GR-Log look on `target`, as the fitter's model renders it
pub fn look(cam: &Camera, target: &str, enc: Encoding) -> Result<Fit, String> {
    if !TARGETS.contains(&target) {
        return Err(format!("GR-Log goes on {}, not {target:?}", TARGETS.join(", ")));
    }
    let c = curve(cam, enc);
    Ok(Fit {
        target: target.to_string(),
        input: Input::GrLog { lo: enc.lo, span: enc.span },
        matrix: cam.standard,
        curves: [c.clone(), c.clone(), c],
        hsl: None,
        post: None,
        report: Value::Null,
    })
}

/// the target as a v2 preset holds it: source Standard, Standard's matrix,
/// the three curves; `edit.lut` says where it came from like a film of the
/// library does (`library: "grlog"`, the encoding, `hc`), so the editor
/// treats it as a fitted target (resets go back to it, another source drops it)
pub fn spec(cam: &Camera, target: &str, enc: Encoding) -> Result<Value, String> {
    let fit = look(cam, target, enc)?;
    let mut lut = json!({ "library": "grlog", "title": "GR-Log", "lo": enc.lo, "span": enc.span });
    if cam.hc {
        lut["hc"] = json!(true);
    }
    let mut spec = fit.spec(lut);
    // a log curve is not a fit: no error report
    if let Some(l) = spec["edit"]["lut"].as_object_mut() {
        l.remove("report");
    }
    debug_assert!(look_sets().any(|(n, _)| n == target));
    Ok(spec)
}

/// what a post LUT turns GR-Log into
pub enum To<'a> {
    /// the camera's Standard look, as it would have recorded the frame
    Standard,
    /// Rec.709: the linear values through the BT.709 curve, with a soft
    /// shoulder up to the camera's input end
    Rec709,
    /// a film LUT (`.cube`) on its own input (grlog, linear or display)
    Film(&'a Lut),
}

/// the post LUT from GR-Log codes (0..1, as the camera's RGB) to `to`
pub fn post_lut(cam: &Camera, enc: Encoding, to: &To, size: usize, title: &str) -> Result<String, String> {
    if !(2..=lutfit::MAX_SIZE).contains(&size) {
        return Err(format!("a LUT size of 2 .. {}, not {size}", lutfit::MAX_SIZE));
    }
    // Standard's rendering of the look's linear values: its matrix is
    // already applied, so identity here, and Standard's own (linear) curves
    let lin = crate::chan::linear();
    let standard = Fit {
        target: "Standard".into(), input: Input::Display, matrix: IDENTITY,
        curves: [lin.clone(), lin.clone(), lin], hsl: None, post: None, report: Value::Null,
    };
    let std_r = standard.renderer(cam)?;
    let to_scene: M3 = lutfit::inverse(&cam.standard);
    let top = 0.18 * 2f64.powf(Encoding::top_ev(cam));
    let decode = decoder(cam, enc);
    let f = |u: [f64; 3]| -> [f64; 3] {
        let x = u.map(&decode);
        match to {
            To::Standard => std_r.render(&[x])[0],
            To::Rec709 => x.map(|v| rec709(shoulder(v, top))),
            To::Film(lut) => {
                let l = lutfit::apply(&to_scene, x).map(|v| v.max(0.0));
                let input = lut.input.unwrap_or(Input::Display);
                let q = match input {
                    Input::GrLog { lo, span } => l.map(|v| (((v.max(1e-12) / 0.18).log2() - lo) / span).clamp(0.0, 1.0)),
                    Input::Linear => l,
                    Input::Display => std_r.render(&[x])[0],
                };
                lut.lookup(q).map(|v| v.clamp(0.0, 1.0))
            }
        }
    };
    let what = match to {
        To::Standard => "the camera's Standard look (sRGB-encoded)".to_string(),
        To::Rec709 => "Rec.709 (BT.709 curve, soft shoulder)".to_string(),
        To::Film(l) => format!("film LUT {}", if l.title.is_empty() { "(untitled)" } else { &l.title }),
    };
    let comments = vec![
        format!("input: GR-Log, grlog {} {} on Standard's colours (camera RGB codes 0..1, full range)", enc.lo, enc.span),
        format!("output: {what}"),
        format!("camera model: Standard's tone curve, highlight correction {}", if cam.hc { "on" } else { "off" }),
        "made by the GR III Look Editor (core/src/grlog.rs)".to_string(),
    ];
    Ok(cube(title, &comments, size, f))
}

/// code 0..1 -> the look's linear value, through the curve the camera
/// actually applies: Standard's tone and C composed at the 256 tone steps,
/// linear in between (so the straight toe under the first step and the
/// chords of the log in the deep shadows invert exactly, and code 0 is black)
pub fn decoder<'a>(cam: &'a Camera<'_>, enc: Encoding) -> impl Fn(f64) -> f64 + 'a {
    let c: Vec<i64> = curve(cam, enc).into_iter().map(i64::from).collect();
    let o: Vec<f64> = crate::chan::compose(&cam.tone, &c).into_iter().map(|v| v as f64 / 16384.0).collect();
    move |u: f64| {
        let k = match o.windows(2).position(|w| w[1] >= u) {
            Some(k) => k,
            None => return (o.len() - 1) as f64 * 16.0 / cam.scale,
        };
        let (a, b) = (o[k], o[k + 1]);
        let t = k as f64 + if b > a { ((u - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        t * 16.0 / cam.scale
    }
}

/// soft shoulder: linear up to 0.6, then an exponential roll-off that lands
/// at 1.0 where the camera's input ends (`top`, linear)
fn shoulder(v: f64, top: f64) -> f64 {
    let k = 0.6;
    if v <= k || top <= k {
        return v.clamp(0.0, 1.0);
    }
    let g = |y: f64| 1.0 - (-(y - k) / (1.0 - k)).exp();
    k + (1.0 - k) * g(v.min(top)) / g(top)
}

/// the BT.709 transfer curve
fn rec709(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    if v < 0.018 { 4.5 * v } else { 1.099 * v.powf(0.45) - 0.099 }
}

/// a 3D `.cube` of `size`^3 points, R fastest, domain 0..1
pub fn cube(title: &str, comments: &[String], size: usize, f: impl Fn([f64; 3]) -> [f64; 3]) -> String {
    let mut s = format!("TITLE \"{}\"\n", title.replace('"', "'"));
    for c in comments {
        s += &format!("# {c}\n");
    }
    s += &format!("LUT_3D_SIZE {size}\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n");
    let u = |k: usize| k as f64 / (size - 1) as f64;
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                let v = f([u(r), u(g), u(b)]);
                s += &format!("{:.6} {:.6} {:.6}\n", v[0], v[1], v[2]);
            }
        }
    }
    s
}
