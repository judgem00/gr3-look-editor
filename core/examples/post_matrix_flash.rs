//! Phase 4 of docs/plan-post-matrix.md: one test firmware for the post-gamma
//! matrix P.
//!
//!     cargo run --release --example post_matrix_flash --manifest-path core/Cargo.toml
//!
//! Writes work/post-matrix-test/ (or $PM_OUT; not in git): FWDC239B.BIN, the
//! decoded image (unpackedFW, for tools/verify-tuning.py --post) and the
//! preset. The update file comes from GR_UPDATE or work/fwdc239b.bin.
//!
//! Every test look takes Standard's colour data, so Standard is the exact
//! control and only P differs:
//!
//!     Standard          unchanged (control)
//!     Positive Film     P = R/B swap: yellow -> cyan, unmistakable
//!     Vivid             P = halfway to grey (luma 77/150/29): applied to
//!                       gamma-encoded RGB, it gives other colours than the
//!                       same mix before the tone curve, so the model
//!                       comparison shows where it sits
//!     Negative Film     P = Portra 400 against spektrafilm (spektrafilm-fit.md)

use std::path::PathBuf;

use grlook::grfw::Container;
use grlook::session::Session;
use serde_json::json;

const SWAP: [f64; 9] = [0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0];
const PORTRA: [f64; 9] = [0.9740, 0.0928, -0.0668, 0.0810, 0.8078, 0.1112, 0.0130, 0.1346, 0.8524];

/// 0.5 I + 0.5 (every row = the luma weights)
fn half_grey() -> [f64; 9] {
    let l = [77.0 / 256.0, 150.0 / 256.0, 29.0 / 256.0];
    std::array::from_fn(|n| 0.5 * l[n % 3] + if n / 3 == n % 3 { 0.5 } else { 0.0 })
}

fn main() {
    let work = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work");
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work.join("fwdc239b.bin"));
    let raw = std::fs::read(&update).unwrap_or_else(|e| panic!("{}: {e}", update.display()));
    let s = Session::open(raw, "fwdc239b.bin").unwrap();

    let body = json!({"targets": {
        "PositiveFilm": {"source": "Standard", "post": SWAP},
        "Vivid": {"source": "Standard", "post": half_grey()},
        "NegativeFilm": {"source": "Standard", "post": PORTRA},
    }});
    let (r, packed, preset) = s.export(&body).unwrap();
    let img = Container::parse(packed.clone()).unwrap().decode().unwrap();

    let out = std::env::var("PM_OUT").map(PathBuf::from).unwrap_or_else(|_| work.join("post-matrix-test"));
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("FWDC239B.BIN"), &packed).unwrap();
    std::fs::write(out.join("unpackedFW"), &img).unwrap();
    std::fs::write(out.join("post-matrix-test.looks.json"), &preset).unwrap();
    println!("{}", serde_json::to_string_pretty(&r).unwrap());
    println!("written to {}", out.display());
}
