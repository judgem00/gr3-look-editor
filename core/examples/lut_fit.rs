//! The LUT fitter on the command line (docs/plan-spektrafilm-fitter.md):
//! fits a look to a .cube for one image control and prints a v2 preset.
//!
//!     cargo run --release --example lut_fit --manifest-path core/Cargo.toml -- \
//!         film.cube[=Target] ... [--no-post] [--no-hsl] [--input display|linear|grlog] [--out DIR] > film.looks.json
//!
//! Several cubes go into one preset, each on its own target (default
//! NegativeFilm, then PositiveFilm, Vivid). The report goes to stderr and
//! into each target's edit.lut. With --out the preset is exported too:
//! DIR/FWDC239B.BIN, unpackedFW and the preset, and the same without any P
//! (unpackedFW-noP, the reference for tools/verify-tuning.py --post-base).
//! The update file comes from GR_UPDATE or work/fwdc239b.bin.

use std::path::PathBuf;
use std::time::Instant;

use grlook::grfw::Container;
use grlook::lutfit::{self, Camera, Input, Options};
use grlook::session::Session;
use serde_json::json;
use sha2::{Digest, Sha256};

fn main() {
    let mut opt = Options::default();
    let mut cubes = vec![];
    let mut out: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-post" => opt.post = false,
            "--no-hsl" => opt.hsl = false,
            "--input" => opt.input = Some(Input::parse(&args.next().expect("--input needs a value")).unwrap()),
            "--out" => out = Some(PathBuf::from(args.next().expect("--out needs a directory"))),
            _ => cubes.push(a),
        }
    }
    if cubes.is_empty() {
        eprintln!("usage: lut_fit film.cube[=Target] ... [--no-post] [--no-hsl] [--input display|linear|grlog]");
        std::process::exit(2);
    }
    let work = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work");
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work.join("fwdc239b.bin"));
    let raw = std::fs::read(&update).unwrap_or_else(|e| panic!("{}: {e}", update.display()));
    let name = update.file_name().unwrap().to_string_lossy().to_string();
    let session = Session::open(raw, &name).unwrap();

    let defaults = ["NegativeFilm", "PositiveFilm", "Vivid"];
    let mut targets = serde_json::Map::new();
    for (k, c) in cubes.iter().enumerate() {
        let (path, target) = c.split_once('=').unwrap_or((c, defaults.get(k).copied().unwrap_or("NegativeFilm")));
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let lut = lutfit::parse_cube(&String::from_utf8_lossy(&bytes)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let cam = Camera::new(session.image(), target).unwrap();
        let t0 = Instant::now();
        let fit = lutfit::fit(&cam, &lut, &opt, &mut |s| eprintln!("{target}: {s} ({:.1} s)", t0.elapsed().as_secs_f64()))
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        eprintln!("{target}: done in {:.1} s\n{}", t0.elapsed().as_secs_f64(), serde_json::to_string_pretty(&fit.report).unwrap());
        let file = PathBuf::from(path).file_name().unwrap().to_string_lossy().to_string();
        let sha = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
        targets.insert(target.to_string(), fit.spec(json!({ "name": file, "sha256": sha })));
    }
    let preset = json!({ "version": 2, "firmware": name, "targets": targets });
    println!("{}", serde_json::to_string_pretty(&preset).unwrap());
    if let Some(dir) = out {
        std::fs::create_dir_all(&dir).unwrap();
        let (r, packed, written) = session.export(&json!({ "targets": targets })).unwrap();
        std::fs::write(dir.join("FWDC239B.BIN"), &packed).unwrap();
        std::fs::write(dir.join("unpackedFW"), Container::parse(packed.clone()).unwrap().decode().unwrap()).unwrap();
        std::fs::write(dir.join("preset.looks.json"), written).unwrap();
        let mut plain = targets.clone();
        for spec in plain.values_mut() {
            spec.as_object_mut().unwrap().remove("post");
        }
        let (_, packed_ref, _) = session.export(&json!({ "targets": plain })).unwrap();
        std::fs::write(dir.join("unpackedFW-noP"), Container::parse(packed_ref).unwrap().decode().unwrap()).unwrap();
        eprintln!("exported to {}: space {}", dir.display(), r["space"]);
    }
}
