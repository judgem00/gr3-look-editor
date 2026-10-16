//! GR-Log (docs/plan-gr-log.md, phase 1): the log curve on the camera
//! model, the target as a preset, and the post LUTs back to Standard,
//! Rec.709 and a film.

mod common;
use common::need;

use grlook::grlog::{self, Encoding, To};
use grlook::lutfit::{self, Fit, Input};
use grlook::session::Session;
use serde_json::{json, Value};

fn session() -> Option<Session> {
    Some(Session::open(common::update_file()?, "fwdc239b.bin").unwrap())
}

/// scene colours: a grey ramp and random colours, from `lo` to `hi` EV over grey
fn samples(lo: f64, hi: f64) -> Vec<[f64; 3]> {
    let mut v: Vec<[f64; 3]> = (0..60).map(|k| [0.18 * 2f64.powf(lo + (hi - lo) * k as f64 / 59.0); 3]).collect();
    let mut s: u64 = 7;
    let mut next = || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    for _ in 0..400 {
        let c: [f64; 3] = std::array::from_fn(|_| 0.05 + 0.95 * next());
        let mx = c.iter().cloned().fold(0.0, f64::max);
        let g = 0.18 * 2f64.powf(lo + (hi - lo) * next()) / mx;
        v.push(c.map(|x| x * g));
    }
    v
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

fn de(a: &[[f64; 3]], b: &[[f64; 3]]) -> Vec<f64> {
    a.iter().zip(b).map(|(x, y)| lutfit::de2000(lutfit::lab(*x), lutfit::lab(*y))).collect()
}

#[test]
fn encoding() {
    let e = Encoding::DEFAULT;
    assert!((e.encode(0.18) - 0.6).abs() < 1e-12);
    assert!((e.decode(e.encode(0.5)) - 0.5).abs() < 1e-9);
    assert_eq!(e.encode(0.0), 0.0);
    assert_eq!(e.encode(1e6), 1.0);
    assert!(Encoding::new(-6.0, 10.0).is_ok());
    assert!(Encoding::new(1.0, 10.0).is_err() && Encoding::new(-6.0, 2.0).is_err() && Encoding::new(f64::NAN, 10.0).is_err());
}

/// the curve rises, starts at 0 and stores the scene as E(x) on the model
#[test]
fn curve_is_log_on_the_model() {
    let s = need!(session());
    for hc in [false, true] {
        let cam = grlog::camera(s.image(), hc).unwrap();
        let enc = Encoding::DEFAULT;
        let c = grlog::curve(&cam, enc);
        assert_eq!(c.len(), 256);
        assert_eq!(c[0], 0);
        assert!(c.windows(2).all(|w| w[1] >= w[0]), "rising");
        let top = Encoding::top_ev(&cam);
        assert!((top - if hc { 4.45 } else { 3.46 }).abs() < 0.05, "{top}");
        // a grey ramp from the first tone index to just under the input end:
        // the camera interpolates its curve linearly between 256 knots on the
        // 12-bit input, so below index 1 (−4.5 EV, −3.5 with HC) it can only
        // draw a straight toe, not a log
        let first = (16.0 / (0.18 * cam.scale)).log2() + 0.05;
        let look = grlog::look(&cam, "PositiveFilm", enc).unwrap();
        let r = look.renderer(&cam).unwrap();
        // worst error in 8-bit codes: in the deep shadows (the first stop and a
        // half above `first`) the knots are a whole tone step apart and the
        // camera draws chords of the log; above, they are dense
        let (mut deep, mut rest): (f64, f64) = (0.0, 0.0);
        for k in 0..200 {
            let ev = first + (top - 0.1 - first) * k as f64 / 199.0;
            let l = [0.18 * 2f64.powf(ev); 3];
            let m = cam.standard;
            let x: [f64; 3] = std::array::from_fn(|r| m[3 * r] * l[0] + m[3 * r + 1] * l[1] + m[3 * r + 2] * l[2]);
            let o = r.render(&[l])[0];
            for ch in 0..3 {
                let e = (o[ch] - enc.encode(x[ch])).abs() * 255.0;
                if ev < first + 1.5 { deep = deep.max(e) } else { rest = rest.max(e) }
            }
        }
        assert!(deep < 2.5 && rest < 1.0, "hc {hc}: deep shadows {deep:.2}, rest {rest:.2} codes");
        let worst = rest / 255.0;
        eprintln!("hc {hc}: log from {first:+.2} to +{top:.2} EV, worst {deep:.2} codes in the deep shadows, {:.2} above", worst * 255.0);
    }
}

#[test]
fn targets_are_checked() {
    let s = need!(session());
    let cam = grlog::camera(s.image(), false).unwrap();
    for t in ["NegativeFilm", "HdrCuisine", "CrossProcess", "MonotoneGR", "x"] {
        assert!(grlog::look(&cam, t, Encoding::DEFAULT).is_err(), "{t}");
    }
}

/// every GR-Log target goes through the export; the preset marks it as log
#[test]
fn targets_export() {
    let s = need!(session());
    for hc in [false, true] {
        let cam = grlog::camera(s.image(), hc).unwrap();
        for t in grlog::TARGETS {
            let spec = grlog::spec(&cam, t, Encoding::DEFAULT).unwrap();
            assert_eq!(spec["source"], "Standard");
            let l = &spec["edit"]["lut"];
            assert_eq!(l["library"], "grlog");
            assert_eq!(l["lo"], -6.0);
            assert_eq!(l.get("hc").is_some(), hc);
            assert!(l.get("report").is_none(), "not a fit");
            let (_, _, preset) = s.export(&json!({ "targets": { t: spec } })).unwrap_or_else(|e| panic!("{t}: {e}"));
            let preset: Value = serde_json::from_str(&preset).unwrap();
            assert_eq!(preset["targets"][t]["edit"]["lut"]["span"], 10.0, "{t}");
        }
    }
}

/// GR-Log through `GR-Log → Standard` is the camera's Standard
#[test]
fn post_lut_to_standard() {
    let s = need!(session());
    let cam = grlog::camera(s.image(), false).unwrap();
    let enc = Encoding::DEFAULT;
    let look = grlog::look(&cam, "Vivid", enc).unwrap();
    let lin = grlook::chan::linear();
    let standard = Fit {
        target: "Standard".into(), input: Input::Display, matrix: cam.standard,
        curves: [lin.clone(), lin.clone(), lin], hsl: None, post: None, report: Value::Null,
    };
    let text = grlog::post_lut(&cam, enc, &To::Standard, 33, "GR-Log to Standard").unwrap();
    let lut = lutfit::parse_cube(&text).unwrap();
    assert_eq!(lut.size, 33);
    assert!(text.contains("# input: GR-Log, grlog -6 10"));
    let l = samples(-5.0, 3.2);
    let log = look.renderer(&cam).unwrap().render(&l);
    let want = standard.renderer(&cam).unwrap().render(&l);
    let got: Vec<[f64; 3]> = log.iter().map(|u| lut.lookup(*u)).collect();
    let d = de(&got, &want);
    let p95 = { let mut v = d.clone(); v.sort_by(f64::total_cmp); v[v.len() * 95 / 100] };
    eprintln!("GR-Log -> Standard: ΔE2000 mean {:.2}, p95 {:.2}", mean(&d), p95);
    // the LUT inverts the curve the camera applies (grlog::decoder), so only
    // the 33^3 lookup is left
    assert!(mean(&d) < 0.15 && p95 < 0.4, "mean {} p95 {}", mean(&d), p95);
    assert_eq!(lut.lookup([0.0; 3]), [0.0; 3], "code 0 is black");
}

/// Rec.709: grey through BT.709, rising, white where the input ends
#[test]
fn post_lut_to_rec709() {
    let s = need!(session());
    let cam = grlog::camera(s.image(), false).unwrap();
    let enc = Encoding::DEFAULT;
    let lut = lutfit::parse_cube(&grlog::post_lut(&cam, enc, &To::Rec709, 33, "GR-Log to Rec.709").unwrap()).unwrap();
    let grey = lut.lookup([enc.encode(0.18); 3]);
    let bt709 = 1.099 * 0.18f64.powf(0.45) - 0.099;
    assert!((grey[1] - bt709).abs() < 0.01, "{grey:?} vs {bt709}");
    let ramp: Vec<f64> = (0..50).map(|k| lut.lookup([k as f64 / 49.0; 3])[1]).collect();
    assert!(ramp.windows(2).all(|w| w[1] >= w[0] - 1e-9), "rising");
    let end = lut.lookup([enc.encode(0.18 * 2f64.powf(Encoding::top_ev(&cam))); 3])[1];
    assert!(end > 0.98, "{end}");
}

/// GR-Log through `GR-Log → film` is the film LUT on the scene
#[test]
fn post_lut_to_film() {
    let s = need!(session());
    let path = common::work().join("films/kodak_portra_400.kodak_portra_endura.cube");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("skipped: {} missing", path.display());
        return;
    };
    let film = lutfit::parse_cube(&text).unwrap();
    let cam = grlog::camera(s.image(), false).unwrap();
    let enc = Encoding::DEFAULT;
    let look = grlog::look(&cam, "PositiveFilm", enc).unwrap();
    let post = lutfit::parse_cube(&grlog::post_lut(&cam, enc, &To::Film(&film), 33, "GR-Log to Portra").unwrap()).unwrap();
    // the film LUT's own input: grlog -8 12 on scene-linear sRGB
    let Some(Input::GrLog { lo, span }) = film.input else { panic!("{:?}", film.input) };
    let l = samples(-4.0, 3.0);
    let want: Vec<[f64; 3]> = l.iter().map(|c| film.lookup(c.map(|v| (((v / 0.18).log2() - lo) / span).clamp(0.0, 1.0)))).collect();
    let log = look.renderer(&cam).unwrap().render(&l);
    let got: Vec<[f64; 3]> = log.iter().map(|u| post.lookup(*u)).collect();
    let d = de(&got, &want);
    eprintln!("GR-Log -> Portra: ΔE2000 mean {:.2}", mean(&d));
    // two tetrahedral lookups on 33^3 and the curve's knots
    assert!(mean(&d) < 0.3, "mean {}", mean(&d));
}

/// the session's requests (wasm.rs): the target and the post LUTs
#[test]
fn session_requests() {
    let s = need!(session());
    let r = s.grlog(&json!({ "target": "Retro", "hc": true })).unwrap();
    assert_eq!(r["spec"]["edit"]["lut"]["hc"], true);
    assert_eq!(r["top_ev"].as_f64().map(|v| (v - 4.45).abs() < 0.05), Some(true));
    assert!(s.grlog(&json!({ "target": "NegativeFilm" })).is_err());
    assert!(s.grlog(&json!({ "target": "Retro", "lo": 2 })).is_err());
    let cube = s.grlog_lut(&json!({ "to": "standard", "size": 9 }), None).unwrap();
    assert_eq!(lutfit::parse_cube(&cube).unwrap().size, 9);
    let own = "TITLE \"own\"\n# GR-INPUT: grlog -8 12\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
    let cube = s.grlog_lut(&json!({ "to": "lut", "size": 5 }), Some(own)).unwrap();
    assert!(cube.contains("film LUT own"));
    assert!(s.grlog_lut(&json!({ "to": "lut" }), None).is_err());
    assert!(s.grlog_lut(&json!({ "to": "x" }), None).is_err());
}
