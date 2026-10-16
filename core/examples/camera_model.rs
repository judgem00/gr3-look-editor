//! The camera data tools/spektrafilm-fit.py models a look with: the Standard
//! tone curve at default sliders (GammaTuning, 256 x u16) and the YCbCr matrix
//! of the MultiAxial stage.
//!
//!     cargo run --release --example camera_model --manifest-path core/Cargo.toml > work/spektrafilm-fit/camera.json
//!
//! The update file comes from GR_UPDATE or work/fwdc239b.bin.

use std::path::PathBuf;

use grlook::gamma::{Engine, ToneParams};

fn main() {
    let work = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work");
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work.join("fwdc239b.bin"));
    let raw = std::fs::read(&update).unwrap_or_else(|e| panic!("{}: {e}", update.display()));
    let img = grlook::grfw::Container::parse(raw).unwrap().decode().unwrap();
    let p = ToneParams { key: 0, contrast: 0, highlight: 0, shadow: 0, gain: 0x1800, flag: 0 };
    let curve = Engine::new(&img, &[]).curve(&img, p).unwrap();
    // Standard's parameter set
    let ycc = grlook::multiaxial::ycc(&img, 11);
    println!("{}", serde_json::json!({ "curve": curve, "ycc": ycc }));
}
