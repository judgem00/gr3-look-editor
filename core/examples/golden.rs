//! Refreshes the editor's answers in work/golden/golden.json from this core.
//!
//!     cargo run --release --example golden --manifest-path core/Cargo.toml
//!
//! Run it after an intended change to what the editor answers (tone curves,
//! export, presets, the shot fit); the tests then hold the new answers. It
//! keeps the requests and the independent references as they are: the
//! GammaTuning curves from Unicorn (gamma.u16) and the photos as rawpy decoded
//! them (*.linear.f32, *.cam.u8). Those came from the earlier Python editor
//! and are not regenerated, so the tests keep checking against them.
//!
//! The update file comes from GR_UPDATE or work/fwdc239b.bin.

use std::path::PathBuf;

use grlook::{dng, session::Session};
use serde_json::{json, Value};

fn main() {
    let work = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work");
    let path = work.join("golden/golden.json");
    let mut g: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("work/golden/golden.json"))
        .expect("golden.json");
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work.join("fwdc239b.bin"));
    let raw = std::fs::read(&update).unwrap_or_else(|e| panic!("{}: {e}", update.display()));

    // the same order as tests/session.rs: inheritance carries over between steps
    let mut s = Session::open(raw.clone(), "fwdc239b.bin").unwrap();
    let mut fw = s.firmware();
    fw["path"] = json!("work/fwdc239b/unpackedFW");
    g["firmware"] = fw;
    for t in g["tone"].as_array_mut().unwrap() {
        t["response"] = s.tone(&t["request"]).unwrap();
    }
    let mut inh = s.set_inherit(&json!({"target": "CrossProcess", "source": "NegativeFilm"})).unwrap();
    inh["path"] = json!("work/fwdc239b/unpackedFW");
    g["inherit"]["NegativeFilm"] = inh;
    g["tone_inherited"] = s.tone(&g["tone"][1]["request"]).unwrap();
    for e in g["export"].as_array_mut().unwrap() {
        s.set_inherit(&json!({"target": "CrossProcess", "source": e["inherit"]})).unwrap();
        let (r, packed, preset) = s.export(&e["request"]).unwrap();
        e["update_sha256"] = r["update_sha256"].clone();
        e["update_size"] = json!(packed.len());
        e["frames_reencoded"] = r["frames_reencoded"].clone();
        e["preset"] = serde_json::from_str(&preset).unwrap();
    }

    let s = Session::open(raw, "fwdc239b.bin").unwrap();
    for case in g["dng"].as_array_mut().unwrap() {
        let stem = case["stem"].as_str().unwrap().to_string();
        let file = case["path"].as_str().unwrap().to_string();
        let Ok(data) = std::fs::read(&file) else {
            eprintln!("{stem}: DNG not found, kept as it was");
            continue;
        };
        let cam: Vec<u8> = std::fs::read(work.join(format!("golden/{stem}.cam.u8"))).unwrap()
            .chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        let shot = dng::decode(&data).unwrap();
        let name = std::path::Path::new(&file).file_name().unwrap().to_str().unwrap();
        case["meta"] = s.prepare_shot(&shot, Some(&cam), name).unwrap().meta;
    }

    std::fs::write(&path, serde_json::to_string(&g).unwrap()).unwrap();
    println!("wrote {}", path.display());
}
