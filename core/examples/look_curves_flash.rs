//! Phase 4 of docs/plan-look-curves.md: one test firmware for the curve
//! selector and HDR effects off.
//!
//!     cargo run --release --example look_curves_flash --manifest-path core/Cargo.toml
//!
//! Writes work/look-curves-test/ (or work/$LC_OUT/; not in git): FWDC239B.BIN, the decoded
//! image (unpackedFW, for tools/verify-tuning.py --patched) and the preset.
//! The update file comes from GR_UPDATE or work/fwdc239b.bin.
//!
//! Every test look takes Standard's colour data, so Standard is the exact
//! control and only the curves differ:
//!
//!     Standard          unchanged (control)
//!     Positive Film     A: red raised
//!     Vivid             B: blue in the shadows
//!     Negative Film     C: green raised + HSL (blue desaturated)
//!     HDR Tone          D: green lowered (magenta), effects off
//!     Cross Process     Color Tone 1 yellow, 2 cyan, 3 blue raised

use std::path::PathBuf;

use grlook::grfw::Container;
use grlook::session::Session;
use serde_json::{json, Value};

const FULL: f64 = 16383.0;

/// C[k] for input 64 k, through `f` on 0..1
fn curve(f: impl Fn(f64) -> f64) -> Vec<f64> {
    (0..256).map(|k| (f((64.0 * k as f64 / FULL).min(1.0)).clamp(0.0, 1.0) * FULL).round()).collect()
}

fn main() {
    let work = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work");
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work.join("fwdc239b.bin"));
    let raw = std::fs::read(&update).unwrap_or_else(|e| panic!("{}: {e}", update.display()));
    let mut s = Session::open(raw, "fwdc239b.bin").unwrap();

    for target in ["PositiveFilm", "Vivid", "NegativeFilm"] {
        s.set_inherit(&json!({"target": target, "source": "Standard"})).unwrap();
    }
    s.set_effects(&json!({"effects_off": ["HdrCuisine"]})).unwrap();

    let lin = curve(|x| x);
    let up = curve(|x| x + 0.18 * (std::f64::consts::PI * x).sin());          // mid-tones raised
    let down = curve(|x| x - 0.18 * (std::f64::consts::PI * x).sin());        // mid-tones lowered
    let shadow = curve(|x| x + 0.15 * (1.0 - x).powi(3));                   // lifted blacks
    let look_curves = json!({
        "PositiveFilm": [up, lin, lin],
        "Vivid": [lin, lin, shadow],
        "NegativeFilm": [lin, up, lin],
        "HdrCuisine": [lin, down, lin],
    });
    // Cross Process Color Tone n = library slots 3n..3n+2 (chan::SETS)
    let mut channels = serde_json::Map::new();
    for (i, c) in [(5, &down), (6, &down), (11, &up)] {
        channels.insert(i.to_string(), json!(c));
    }
    for i in [3, 4, 7, 8, 9, 10] {
        channels.insert(i.to_string(), json!(lin));
    }
    // HSL marker on Negative Film: blue (hue coordinate ~2500, hsl.js) desaturated
    let hsl = json!({"NegativeFilm": {"bands": [{"center": 2500, "width": 1400, "feather": 200, "sat": -80}]}});

    let body = json!({"look_curves": look_curves, "channels": channels, "hsl": hsl});
    let (r, packed, preset) = s.export(&body).unwrap();
    let img = Container::parse(packed.clone()).unwrap().decode().unwrap();

    let out = work.join(std::env::var("LC_OUT").unwrap_or_else(|_| "look-curves-test".into()));
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("FWDC239B.BIN"), &packed).unwrap();
    std::fs::write(out.join("unpackedFW"), &img).unwrap();
    std::fs::write(out.join("look-curves-test.looks.json"), &preset).unwrap();
    println!("{}", serde_json::to_string_pretty(&r).unwrap());
    let v: Value = serde_json::from_str(&preset).unwrap();
    println!("looks with curves: {:?}", v["look_curves"].as_object().unwrap().keys().collect::<Vec<_>>());
    println!("written to {}", out.display());
}
