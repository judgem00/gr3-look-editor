//! A flash export of an editor preset, from the command line: the same as the
//! editor's Export button (looks and the video bundle of the preset).
//!
//!     cargo run --release --example video_export --manifest-path core/Cargo.toml -- \
//!         PRESET.looks.json OUT/FWDC239B.BIN
//!
//! The update file comes from GR_UPDATE or work/fwdc239b.bin. Prints the
//! export report as JSON.

use std::path::PathBuf;

use grlook::session::Session;
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: video_export PRESET OUT");
        std::process::exit(2);
    }
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work/fwdc239b.bin")
    });
    let s = Session::open(std::fs::read(&update).expect("update file"), "FWDC239B.BIN").unwrap();
    let preset: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&args[0]).expect("preset")).expect("preset JSON");
    let body = json!({"targets": preset["targets"], "video": preset["video"]});
    let (result, packed, _) = s.export(&body).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    std::fs::write(&args[1], packed).expect("write");
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}
