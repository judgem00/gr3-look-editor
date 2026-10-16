//! Phase 3 of docs/plan-gr-log.md: the editor's export with GR-Log on
//! Bleach Bypass (all other looks stock), for the video patches to ride on
//! (`tools/video-test.py build … --image`).
//!
//!     cargo run --release --example grlog_flash --manifest-path core/Cargo.toml [-- --hc]
//!
//! Writes work/grlog-test/ (or $GRLOG_OUT; not in git): FWDC239B-looks.BIN,
//! the decoded image (unpackedFW-looks), the preset and the post LUTs
//! GR-Log → Standard and → Rec.709 (33³). The update file comes from
//! GR_UPDATE or work/fwdc239b.bin.

use std::path::PathBuf;

use grlook::grfw::Container;
use grlook::grlog::{self, Encoding, To};
use grlook::session::{sha256_hex, Session};
use serde_json::{json, Value};

const TARGET: &str = "BleechBypass_GR";

fn main() {
    let hc = std::env::args().any(|a| a == "--hc");
    let work = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work");
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work.join("fwdc239b.bin"));
    let s = Session::open(std::fs::read(&update).unwrap(), "fwdc239b.bin").unwrap();
    let cam = grlog::camera(s.image(), hc).unwrap();
    let enc = Encoding::DEFAULT;
    let spec = grlog::spec(&cam, TARGET, enc).unwrap();
    let (r, packed, preset) = s.export(&json!({ "targets": { TARGET: spec } })).unwrap();
    let img = Container::parse(packed.clone()).unwrap().decode().unwrap();

    let out = std::env::var("GRLOG_OUT").map(PathBuf::from).unwrap_or_else(|_| work.join("grlog-test"));
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("FWDC239B-looks.BIN"), &packed).unwrap();
    std::fs::write(out.join("unpackedFW-looks"), &img).unwrap();
    std::fs::write(out.join("grlog-test.looks.json"), &preset).unwrap();
    let tag = if hc { "-HC" } else { "" };
    for (to, name) in [(To::Standard, "Standard"), (To::Rec709, "Rec709")] {
        let cube = grlog::post_lut(&cam, enc, &to, 33, &format!("GR-Log to {name}")).unwrap();
        std::fs::write(out.join(format!("GR-Log{tag}_to_{name}.cube")), cube).unwrap();
    }
    let first = (16.0 / (0.18 * cam.scale)).log2();
    let summary: Value = json!({
        "target": TARGET, "encoding": enc.to_json(), "hc": hc,
        "log_range_ev": [first, Encoding::top_ev(&cam)],
        "grey_code_full_range": (255.0 * enc.encode(0.18)).round(),
        "space": r["space"], "looks_update_sha256": sha256_hex(&packed),
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    println!("written to {}", out.display());
}
