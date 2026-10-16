//! The editor's film library (docs/plan-spektrafilm-fitter.md, "Film library"):
//! every LUT tools/spektrafilm-lut.py --all wrote, fitted once, in parallel.
//!
//!     cargo run --release --example film_library --manifest-path core/Cargo.toml -- \
//!         work/films editor/static/films/spektrafilm.json [--threads N]
//!
//! A fit does not depend on the target (every colour look shares the YCbCr and
//! hue data the fit uses), so the library holds one per film and print; the
//! editor puts it on any target. The update file comes from GR_UPDATE or
//! work/fwdc239b.bin.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use grlook::lutfit::{self, Camera, Options};
use grlook::session::Session;
use serde_json::{json, Map, Value};

/// numbers in the report and the matrices, rounded for a smaller file
fn round(v: &Value, digits: i32) -> Value {
    match v {
        Value::Number(n) if n.is_f64() => {
            let k = 10f64.powi(digits);
            json!((n.as_f64().unwrap() * k).round() / k)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| round(x, digits)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), round(x, digits))).collect()),
        x => x.clone(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: film_library LUT_DIR OUT.json [--threads N]");
        std::process::exit(2);
    }
    let dir = PathBuf::from(&args[0]);
    let threads = args.iter().position(|a| a == "--threads").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    let work = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work");
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work.join("fwdc239b.bin"));
    let session = Session::open(std::fs::read(&update).unwrap(), "fwdc239b.bin").unwrap();
    let img = session.image();
    let index: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("index.json")).unwrap()).unwrap();
    let looks = index["looks"].as_array().unwrap().clone();
    eprintln!("{} looks on {threads} threads", looks.len());

    let t0 = Instant::now();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Value>>> = Mutex::new(vec![None; looks.len()]);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let cam = Camera::new(img, "NegativeFilm").unwrap();
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(entry) = looks.get(k) else { break };
                    let cube = entry["cube"].as_str().unwrap();
                    let text = std::fs::read_to_string(dir.join(cube)).unwrap();
                    let lut = lutfit::parse_cube(&text).unwrap();
                    let fit = lutfit::fit(&cam, &lut, &Options::default(), &mut |_| {})
                        .unwrap_or_else(|e| panic!("{cube}: {e}"));
                    let report: Map<String, Value> = ["matrix_curves", "model", "final"].iter()
                        .map(|s| (s.to_string(), round(&fit.report[*s], 2))).collect();
                    let out = json!({
                        "film": entry["film"], "print": entry["print"], "default": entry["default"],
                        "print_exposure": entry["print_exposure"], "film_ev": entry["film_ev"],
                        "matrix": round(&json!(fit.matrix), 7),
                        "curves": fit.curves,
                        "hsl": fit.hsl,
                        "post": round(&json!(fit.post), 6),
                        "report": report,
                    });
                    eprintln!("  {:3}/{} {cube}: {:.2} ΔE ({:.0} s)", k + 1, looks.len(),
                              fit.report["final"]["all"].as_f64().unwrap(), t0.elapsed().as_secs_f64());
                    results.lock().unwrap()[k] = Some(out);
                }
            });
        }
    });
    let looks: Vec<Value> = results.into_inner().unwrap().into_iter().map(Option::unwrap).collect();
    let library = json!({
        "format": 2,
        "attribution": "Derived from spektrafilm by Andrea Volpato, https://github.com/andreavolpato/spektrafilm, \
                        licensed CC BY-SA 4.0 (LICENSE.txt). Modified: rendered as LUTs and fitted to GR III look \
                        parameters by the GR III Look Editor (CHANGELOG.txt).",
        "spektrafilm": index["spektrafilm"],
        "normalisation": index["normalisation"],
        "names": index["names"],
        "looks": looks,
    });
    let out = PathBuf::from(&args[1]);
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, serde_json::to_string(&library).unwrap()).unwrap();
    eprintln!("{}: {} looks, {} KB, {:.0} s", out.display(), looks.len(),
              std::fs::metadata(&out).unwrap().len() / 1024, t0.elapsed().as_secs_f64());
}
