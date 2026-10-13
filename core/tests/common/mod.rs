//! Reference data in work/golden (not in git): the requests and the earlier
//! Python editor's answers, refreshed by examples/golden.rs. Tests skip when
//! it is missing.

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

/// the decoded stock image
pub fn stock_image() -> Option<Vec<u8>> {
    grlook::grfw::Container::parse(update_file()?).ok()?.decode().ok()
}

/// the update file as Ricoh distributes it; set GR_UPDATE or put it in work/
pub fn update_file() -> Option<Vec<u8>> {
    let p = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work().join("fwdc239b.bin"));
    std::fs::read(p).ok()
}

/// the GR IIIx update file (firmware 1.60); set GR3X_UPDATE or put it in work/
pub fn update_file_gr3x() -> Option<Vec<u8>> {
    let p = std::env::var("GR3X_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work().join("fwdc243b.bin"));
    std::fs::read(p).ok()
}

macro_rules! need {
    ($e:expr) => {
        match $e {
            Some(v) => v,
            None => {
                eprintln!("skipped: reference data or update file missing (see README, Development)");
                return;
            }
        }
    };
}
pub(crate) use need;
