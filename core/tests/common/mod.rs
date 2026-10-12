//! Reference data written by golden/make.py (work/golden, not in git).
//! Tests skip when it is missing.

#![allow(dead_code)]

use std::path::PathBuf;

pub fn work() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work")
}

pub fn golden() -> Option<serde_json::Value> {
    let p = work().join("golden/golden.json");
    let s = std::fs::read_to_string(&p).ok()?;
    Some(serde_json::from_str(&s).unwrap())
}

/// the decoded stock image, as the Python editor caches it
pub fn stock_image() -> Option<Vec<u8>> {
    std::fs::read(work().join("fwdc239b/unpackedFW")).ok()
}

/// the update file as Ricoh distributes it; set GR_UPDATE or put it in work/
pub fn update_file() -> Option<Vec<u8>> {
    let p = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work().join("fwdc239b.bin"));
    std::fs::read(p).ok()
}

macro_rules! need {
    ($e:expr) => {
        match $e {
            Some(v) => v,
            None => {
                eprintln!("skipped: reference data missing (run golden/make.py)");
                return;
            }
        }
    };
}
pub(crate) use need;
