//! A look as a RAM patch, from the command line (docs/plan-ram-loader.md):
//! the same as the editor's "Test without flashing".
//!
//!     cargo run --release --example ram_patch --manifest-path core/Cargo.toml -- \
//!         PRESET.looks.json OUT/LOOK.PAT [BASE]
//!
//! PRESET is a preset of the editor (version 2: targets, video) or a bare
//! request body ({"targets": ...}). BASE is what the camera runs: an update
//! file or RTOS.BIN of a RAM dump; without it, stock. The update file comes
//! from GR_UPDATE or work/fwdc239b.bin. Prints the result as JSON.

use std::path::PathBuf;

use grlook::session::Session;
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 || args.len() > 3 {
        eprintln!("usage: ram_patch PRESET OUT [BASE]");
        std::process::exit(2);
    }
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work/fwdc239b.bin")
    });
    let s = Session::open(std::fs::read(&update).expect("update file"), "FWDC239B.BIN").unwrap();
    let preset: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&args[0]).expect("preset")).expect("preset JSON");
    let body = json!({"targets": preset["targets"], "video": preset["video"]});
    let base = args.get(2).map(|p| std::fs::read(p).expect("base"));
    let (result, pat, _) = s.export_ram(&body, base.as_deref()).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    std::fs::write(&args[1], pat).expect("write");
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}
