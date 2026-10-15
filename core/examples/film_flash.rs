//! Phase 6 of docs/plan-spektrafilm-fitter.md: one test firmware with a
//! complete film look, with and without the post matrix P and without HSL.
//!
//!     cargo run --release --example film_flash --manifest-path core/Cargo.toml
//!
//! Writes work/film-test/ (or $FILM_OUT; not in git): FWDC239B.BIN, the
//! decoded image (unpackedFW) and the preset, and the same preset exported
//! without any P (unpackedFW-noP, film-test-noP.looks.json) as the reference
//! for tools/verify-tuning.py --post ... --post-base. The LUT comes from
//! $FILM_LUT or work/lut-fitter/luts/kodak_portra_400.kodak_portra_endura.cube
//! (tools/spektrafilm-lut.py), the update file from GR_UPDATE or
//! work/fwdc239b.bin.
//!
//!     Standard          unchanged (control)
//!     Negative Film     matrix + curves + HSL + P: the full film look
//!     Positive Film     matrix + curves + HSL, fitted without P
//!     Vivid             matrix + curves + P, fitted without HSL

use std::path::PathBuf;
use std::time::Instant;

use grlook::grfw::Container;
use grlook::lutfit::{self, Camera, Options};
use grlook::session::{sha256_hex, Session};
use serde_json::{json, Value};

const VARIANTS: [(&str, bool, bool); 3] = [("NegativeFilm", true, true), ("PositiveFilm", false, true), ("Vivid", true, false)];

fn main() {
    let work = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work");
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work.join("fwdc239b.bin"));
    let raw = std::fs::read(&update).unwrap_or_else(|e| panic!("{}: {e}", update.display()));
    let s = Session::open(raw, "fwdc239b.bin").unwrap();
    let lut_path = std::env::var("FILM_LUT").map(PathBuf::from)
        .unwrap_or_else(|_| work.join("lut-fitter/luts/kodak_portra_400.kodak_portra_endura.cube"));
    let text = std::fs::read_to_string(&lut_path).unwrap_or_else(|e| panic!("{}: {e}", lut_path.display()));
    let lut = lutfit::parse_cube(&text).unwrap();
    let name = lut_path.file_name().unwrap().to_string_lossy().to_string();

    let mut targets = serde_json::Map::new();
    for (target, post, hsl) in VARIANTS {
        let cam = Camera::new(s.image(), target).unwrap();
        let t0 = Instant::now();
        let fit = lutfit::fit(&cam, &lut, &Options { post, hsl, ..Options::default() }, &mut |_| {}).unwrap();
        let f = &fit.report["final"];
        eprintln!("{target:13} post {post:5} hsl {hsl:5}: all {} p95 {} chart {} skin {} grey {} ({:.1} s)",
                  f["all"], f["p95"], f["chart"], f["skin"], f["grey"], t0.elapsed().as_secs_f64());
        targets.insert(target.into(), fit.spec(json!({ "name": name, "title": lut.title, "sha256": sha256_hex(text.as_bytes()) })));
    }
    let (r, packed, preset) = s.export(&json!({ "targets": targets })).unwrap();
    let img = Container::parse(packed.clone()).unwrap().decode().unwrap();

    // the reference: the same looks without P
    let mut plain = targets.clone();
    for spec in plain.values_mut() {
        spec.as_object_mut().unwrap().remove("post");
    }
    let (_, packed_ref, preset_ref) = s.export(&json!({ "targets": plain })).unwrap();
    let img_ref = Container::parse(packed_ref).unwrap().decode().unwrap();

    let out = std::env::var("FILM_OUT").map(PathBuf::from).unwrap_or_else(|_| work.join("film-test"));
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("FWDC239B.BIN"), &packed).unwrap();
    std::fs::write(out.join("unpackedFW"), &img).unwrap();
    std::fs::write(out.join("film-test.looks.json"), &preset).unwrap();
    std::fs::write(out.join("unpackedFW-noP"), &img_ref).unwrap();
    std::fs::write(out.join("film-test-noP.looks.json"), &preset_ref).unwrap();
    let summary: Value = json!({ "space": r["space"], "update_sha256": sha256_hex(&packed) });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    println!("written to {}", out.display());
}
