//! One test firmware for grain on looks that do not offer it (docs/grain.md).
//!
//!     cargo run --release --example grain_flash --manifest-path core/Cargo.toml
//!
//! Writes work/grain-test/ (or work/$GRAIN_OUT/; not in git): FWDC239B.BIN, the
//! decoded image (unpackedFW, for tools/verify-grain.py --patched) and the preset.
//! The update file comes from GR_UPDATE or work/fwdc239b.bin.
//!
//! Every test look takes Standard's colour data, so Standard is the exact
//! control and only the grain differs:
//!
//!     Standard          unchanged (control, no grain)
//!     Vivid             grain 1
//!     Positive Film     grain 2
//!     Negative Film     grain 3 (whatever its own grain setting says)

use std::path::PathBuf;

use grlook::grfw::Container;
use grlook::session::Session;
use serde_json::json;

fn main() {
    let work = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work");
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work.join("fwdc239b.bin"));
    let raw = std::fs::read(&update).unwrap_or_else(|e| panic!("{}: {e}", update.display()));
    let s = Session::open(raw, "fwdc239b.bin").unwrap();

    let body = json!({"targets": {
        "Vivid": {"source": "Standard", "grain": 1},
        "PositiveFilm": {"source": "Standard", "grain": 2},
        "NegativeFilm": {"source": "Standard", "grain": 3},
    }});
    let (r, packed, preset) = s.export(&body).unwrap();
    let img = Container::parse(packed.clone()).unwrap().decode().unwrap();

    let out = work.join(std::env::var("GRAIN_OUT").unwrap_or_else(|_| "grain-test".into()));
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("FWDC239B.BIN"), &packed).unwrap();
    std::fs::write(out.join("unpackedFW"), &img).unwrap();
    std::fs::write(out.join("grain-test.looks.json"), &preset).unwrap();
    println!("{}", serde_json::to_string_pretty(&r).unwrap());
    println!("written to {}", out.display());
}
