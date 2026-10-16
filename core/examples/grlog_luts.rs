//! The post LUTs for GR-Log (docs/plan-gr-log.md): GR-Log to the camera's
//! Standard, to Rec.709 and, with --films, to every film LUT in a folder.
//!
//!     cargo run --release --example grlog_luts --manifest-path core/Cargo.toml -- \
//!         OUT_DIR [--hc] [--size N] [--films work/films]
//!
//! The update file comes from GR_UPDATE or work/fwdc239b.bin.

use std::path::PathBuf;

use grlook::grlog::{self, Encoding, To};
use grlook::lutfit;
use grlook::session::Session;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(out) = args.first().map(PathBuf::from) else {
        eprintln!("usage: grlog_luts OUT_DIR [--hc] [--size N] [--films DIR]");
        std::process::exit(2);
    };
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let hc = args.iter().any(|a| a == "--hc");
    let size: usize = opt("--size").and_then(|v| v.parse().ok()).unwrap_or(33);
    let work = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work");
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work.join("fwdc239b.bin"));
    let s = Session::open(std::fs::read(&update).unwrap(), "fwdc239b.bin").unwrap();
    let cam = grlog::camera(s.image(), hc).unwrap();
    let enc = Encoding::DEFAULT;
    std::fs::create_dir_all(&out).unwrap();
    let tag = if hc { "-hc" } else { "" };
    let write = |name: &str, text: String| {
        let p = out.join(name);
        std::fs::write(&p, text).unwrap();
        eprintln!("{}", p.display());
    };
    write(&format!("GR-Log{tag}_to_Standard.cube"), grlog::post_lut(&cam, enc, &To::Standard, size, "GR-Log to Standard").unwrap());
    write(&format!("GR-Log{tag}_to_Rec709.cube"), grlog::post_lut(&cam, enc, &To::Rec709, size, "GR-Log to Rec.709").unwrap());
    if let Some(dir) = opt("--films") {
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().and_then(|x| x.to_str()) != Some("cube") {
                continue;
            }
            let film = lutfit::parse_cube(&std::fs::read_to_string(&p).unwrap()).unwrap();
            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
            let title = format!("GR-Log to {}", if film.title.is_empty() { &stem } else { &film.title });
            write(&format!("GR-Log{tag}_to_{stem}.cube"), grlog::post_lut(&cam, enc, &To::Film(&film), size, &title).unwrap());
            n += 1;
        }
        eprintln!("{n} film LUTs");
    }
}
