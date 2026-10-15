//! The LUT fitter (docs/plan-spektrafilm-fitter.md, phase 2): .cube parsing,
//! lookup, ΔE2000, and fits against LUTs whose answer is known.

mod common;
use common::need;

use grlook::lutfit::{self, Camera, Fit, Input, Options};
use grlook::session::Session;
use serde_json::{json, Value};

/// Portra 400 against spektrafilm (docs/spektrafilm-fit.md)
const PORTRA: [f64; 9] = [0.9740, 0.0928, -0.0668, 0.0810, 0.8078, 0.1112, 0.0130, 0.1346, 0.8524];

/// a .cube of `n`^3 points from f(r, g, b) on 0..1, R fastest
fn cube(n: usize, head: &str, f: impl Fn([f64; 3]) -> [f64; 3]) -> String {
    let mut s = format!("{head}\nLUT_3D_SIZE {n}\n");
    let u = |k: usize| k as f64 / (n - 1) as f64;
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let v = f([u(r), u(g), u(b)]);
                s += &format!("{:.7} {:.7} {:.7}\n", v[0], v[1], v[2]);
            }
        }
    }
    s
}

/// small sample sets, so the fits stay quick in debug builds
fn quick() -> Options {
    Options { random: 300, sweep: 200, max_eval: 25, ..Options::default() }
}

#[test]
fn cube_parses() {
    let text = cube(3, "TITLE \"test\"\n# a comment\n# GR-INPUT: grlog -8 12\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1", |c| c);
    let lut = lutfit::parse_cube(&text).unwrap();
    assert_eq!(lut.size, 3);
    assert_eq!(lut.title, "test");
    assert_eq!(lut.input, Some(Input::GrLog { lo: -8.0, span: 12.0 }));
    assert_eq!(lut.data.len(), 27);
    // R fastest
    assert_eq!(lut.data[1], [0.5, 0.0, 0.0]);
    assert_eq!(lut.data[3], [0.0, 0.5, 0.0]);
    assert_eq!(lut.data[9], [0.0, 0.0, 0.5]);

    let r = lutfit::parse_cube(&cube(2, "LUT_3D_INPUT_RANGE 0 2", |c| c)).unwrap();
    assert_eq!((r.min, r.max), ([0.0; 3], [2.0; 3]));
    assert_eq!(r.input, None);
}

#[test]
fn cube_refusals() {
    let err = |t: &str| lutfit::parse_cube(t).unwrap_err();
    assert!(err("LUT_1D_SIZE 4\n0 0 0\n").contains("1D"));
    assert!(err("TITLE \"x\"\n0 0 0\n").contains("LUT_3D_SIZE"));
    assert!(err("LUT_3D_SIZE 66\n").contains("2 .. 65"));
    assert!(err("LUT_3D_SIZE 2\n0 0 0\n").contains("needs 8 values"));
    assert!(err("LUT_3D_SIZE 2\n0 0 x\n").contains("line 2"));
    assert!(err("LUT_3D_SIZE 2\n0 0\n").contains("need 3 numbers"));
    assert!(err(&cube(2, "DOMAIN_MIN 1 1 1\nDOMAIN_MAX 1 1 1", |c| c)).contains("DOMAIN_MAX"));
    assert!(err("# GR-INPUT: log\nLUT_3D_SIZE 2\n").contains("display, linear or grlog"));
}

#[test]
fn input_parses() {
    assert_eq!(Input::parse("display").unwrap(), Input::Display);
    assert_eq!(Input::parse("linear").unwrap(), Input::Linear);
    assert_eq!(Input::parse("grlog").unwrap(), Input::GrLog { lo: -8.0, span: 12.0 });
    assert_eq!(Input::parse("grlog -6 10").unwrap(), Input::GrLog { lo: -6.0, span: 10.0 });
    assert!(Input::parse("grlog -6 0").is_err());
    assert!(Input::parse("slog3").is_err());
}

#[test]
fn lookup_is_exact_on_linear_luts() {
    let id = lutfit::parse_cube(&cube(5, "", |c| c)).unwrap();
    let wide = lutfit::parse_cube(&cube(4, "DOMAIN_MAX 2 2 2", |c| [c[1], c[2] * c[0], 1.0 - c[0]])).unwrap();
    let mut x = 0.123_f64;
    for _ in 0..500 {
        let mut next = || {
            x = (x * 997.0 + 0.31).fract();
            x
        };
        let u = [next(), next(), next()];
        let v = id.lookup(u);
        for c in 0..3 {
            assert!((v[c] - u[c]).abs() < 1e-12, "{u:?} -> {v:?}");
        }
        // domain 0..2: the grid point at 2 holds f(1, 1, 1)
        let w = wide.lookup(u.map(|v| 2.0 * v));
        assert!((w[0] - u[1]).abs() < 1e-6 && (w[2] - (1.0 - u[0])).abs() < 1e-6, "{u:?} -> {w:?}");
    }
    // outside the domain: clamped
    assert_eq!(id.lookup([-1.0, 2.0, 0.5]), [0.0, 1.0, 0.5]);
}

#[test]
fn de2000_matches_sharma() {
    // Sharma, Wu, Dalal (2005), table 1
    let pairs = [
        ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
        ([50.0, 3.1571, -77.2803], [50.0, 0.0, -82.7485], 2.8615),
        ([50.0, 2.8361, -74.0200], [50.0, 0.0, -82.7485], 3.4412),
        ([50.0, -1.3802, -84.2814], [50.0, 0.0, -82.7485], 1.0000),
        ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
        ([60.2574, -34.0099, 36.2677], [60.4626, -34.1751, 39.4387], 1.2644),
        ([22.7233, 20.0904, -46.6940], [23.0331, 14.9730, -42.5619], 2.0373),
    ];
    for (a, b, want) in pairs {
        let d = lutfit::de2000(a, b);
        assert!((d - want).abs() < 1e-4, "{a:?} {b:?}: {d} vs {want}");
        assert!((lutfit::de2000(b, a) - want).abs() < 1e-4);
    }
    let w = lutfit::lab([1.0, 1.0, 1.0]);
    assert!((w[0] - 100.0).abs() < 1e-3 && w[1].abs() < 1e-2 && w[2].abs() < 1e-2, "{w:?}");
}

fn session() -> Option<Session> {
    Some(Session::open(common::update_file()?, "fwdc239b.bin").unwrap())
}

fn all(f: &Fit, key: &str) -> f64 {
    eprintln!("{} {key}: {}", f.target, f.report[key]);
    f.report[key]["all"].as_f64().unwrap()
}

#[test]
fn targets_are_checked() {
    let s = need!(session());
    for t in ["BleechBypass_GR", "CrossProcess", "HdrCuisine", "MonotoneGR", "nonsense"] {
        assert!(Camera::new(s.image(), t).is_err(), "{t}");
    }
    let cam = Camera::new(s.image(), "NegativeFilm").unwrap();
    // 18 % grey at sRGB 0.46: the tone index sits in the lower tenth of the curve
    let mid = 0.18 * cam.scale / 16.0;
    assert!(mid > 15.0 && mid < 35.0, "{mid}");
}

/// an identity LUT on display input is the camera's Standard: the fit must
/// reproduce it almost exactly
#[test]
fn identity_lut_gives_standard() {
    let s = need!(session());
    let cam = Camera::new(s.image(), "NegativeFilm").unwrap();
    let lut = lutfit::parse_cube(&cube(17, "# GR-INPUT: display", |c| c)).unwrap();
    let fit = lutfit::fit(&cam, &lut, &quick(), &mut |_| {}).unwrap();
    assert_eq!(fit.input, Input::Display);
    assert!(all(&fit, "final") < 0.4, "{}", fit.report);
    assert!(fit.report["final"]["grey"].as_f64().unwrap() < 0.4, "{}", fit.report);
}

/// a LUT made from a look the camera can show (Standard's matrix, an
/// S-curve, P) is fitted back closely
#[test]
fn known_look_round_trip() {
    let s = need!(session());
    let cam = Camera::new(s.image(), "PositiveFilm").unwrap();
    let curve = |k: usize, gamma: f64| -> Vec<u16> {
        (0..256).map(|i| (16383.0 * (i as f64 / 255.0).powf(gamma)).round() as u16).collect::<Vec<_>>()
            .into_iter().enumerate().map(|(i, v)| if i == 0 { 0 } else { v.max(k as u16) }).collect()
    };
    let truth = Fit {
        target: "PositiveFilm".into(),
        input: Input::GrLog { lo: -8.0, span: 12.0 },
        matrix: cam.standard,
        curves: [curve(0, 1.15), curve(0, 1.0), curve(0, 0.9)],
        hsl: None,
        post: Some(PORTRA),
        report: Value::Null,
    };
    let r = truth.renderer(&cam).unwrap();
    let text = cube(33, "# GR-INPUT: grlog -8 12", |u| {
        r.render(&[u.map(|v| 0.18 * 2f64.powf(12.0 * v - 8.0))])[0]
    });
    let lut = lutfit::parse_cube(&text).unwrap();
    let opt = Options { hsl: false, ..quick() };
    let fit = lutfit::fit(&cam, &lut, &opt, &mut |_| {}).unwrap();
    assert!(all(&fit, "final") < 0.6, "{}", fit.report);
    assert!(fit.hsl.is_none() && fit.post.is_some());
}

/// the fitted target goes through the export as it is; three films fit
#[test]
fn fitted_targets_export() {
    let s = need!(session());
    let lut = lutfit::parse_cube(&cube(9, "# GR-INPUT: grlog -8 12", |u| {
        // a warm, flat, film-like answer with crossed colours
        let g = |v: f64| 0.08 + 0.85 * v.powf(1.4);
        [g(0.9 * u[0] + 0.1 * u[1]), g(u[1]), g(0.8 * u[2] + 0.1 * u[0] + 0.1 * u[1]) * 0.95]
    }))
    .unwrap();
    let mut targets = serde_json::Map::new();
    for t in ["NegativeFilm", "PositiveFilm", "Vivid"] {
        let cam = Camera::new(s.image(), t).unwrap();
        let fit = lutfit::fit(&cam, &lut, &quick(), &mut |_| {}).unwrap();
        assert!(fit.hsl.is_some() && fit.post.is_some());
        let spec = fit.spec(json!({ "name": "test.cube" }));
        assert_eq!(spec["source"], "Standard");
        assert_eq!(spec["edit"]["lut"]["input"], "grlog -8 12");
        targets.insert(t.to_string(), spec);
        // each one alone
        let one: serde_json::Map<String, Value> = targets.iter().filter(|(k, _)| *k == t).map(|(k, v)| (k.clone(), v.clone())).collect();
        s.export(&json!({ "targets": one })).unwrap_or_else(|e| panic!("{t}: {e}"));
    }
    let (r, _, preset) = s.export(&json!({ "targets": targets })).unwrap();
    let preset: Value = serde_json::from_str(&preset).unwrap();
    for t in ["NegativeFilm", "PositiveFilm", "Vivid"] {
        assert!(preset["targets"][t]["post"].is_array(), "{t}");
        assert!(preset["targets"][t]["edit"]["lut"]["report"]["final"].is_object(), "{t}");
    }
    eprintln!("space: {}", r["space"]);
    assert_eq!(r["space"]["fits"], true);
    // a fourth complete film does not fit: refused, not a broken image
    let cam = Camera::new(s.image(), "Retro").unwrap();
    let fit = lutfit::fit(&cam, &lut, &quick(), &mut |_| {}).unwrap();
    targets.insert("Retro".into(), fit.spec(json!({ "name": "test.cube" })));
    let err = s.export(&json!({ "targets": targets })).unwrap_err();
    assert!(err.contains("not enough free space"), "{err}");
}

/// the session's requests (wasm.rs): what a .cube holds, and refusals
/// before any fitting
#[test]
fn session_lut_requests() {
    let s = need!(session());
    let text = cube(5, "TITLE \"five\"\n# GR-INPUT: grlog -8 12", |c| c);
    let info = s.lut_info(&text).unwrap();
    assert_eq!(info["size"], 5);
    assert_eq!(info["title"], "five");
    assert_eq!(info["input"], "grlog -8 12");
    assert_eq!(info["max"], json!([1.0, 1.0, 1.0]));
    assert!(s.lut_info("LUT_1D_SIZE 2\n").is_err());

    let fit = |body: Value| s.lut_fit(&text, &body, &mut |_| {});
    assert!(fit(json!({ "target": "CrossProcess" })).unwrap_err().contains("not \"CrossProcess\""));
    assert!(fit(json!({ "target": "Vivid", "input": "slog3" })).unwrap_err().contains("display, linear or grlog"));
}
