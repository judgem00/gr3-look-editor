//! Grain for every colour look (docs/grain.md): what the export writes. The
//! stubs themselves are checked instruction by instruction in grain.rs; the
//! firmware around them in tools/verify-grain.py.

mod common;
use common::need;

use grlook::addr::{file_offset, word};
use grlook::grain;
use grlook::grfw::Container;
use grlook::session::Session;
use serde_json::{json, Value};

fn session() -> Option<(Session, Vec<u8>)> {
    let f = common::update_file()?;
    let stock = Container::parse(f.clone()).unwrap().decode().unwrap();
    Some((Session::open(f, "fwdc239b.bin").unwrap(), stock))
}

fn decode(packed: Vec<u8>) -> Vec<u8> {
    Container::parse(packed).unwrap().decode().unwrap()
}

/// where a branch at `site` goes
fn target_of(img: &[u8], site: u32) -> u32 {
    let w = word(img, site);
    let off = ((w & 0x00ff_ffff) << 8) as i32 >> 6;
    site.wrapping_add(8).wrapping_add(off as u32)
}

fn export(s: &Session, targets: Value) -> (Vec<u8>, Value) {
    let (_, packed, preset) = s.export(&json!({ "targets": targets })).unwrap();
    (decode(packed), serde_json::from_str(&preset).unwrap())
}

#[test]
fn grain_for_looks_without_it() {
    let (s, stock) = need!(session());
    let (img, preset) = export(&s, json!({"Standard": {"grain": 2}, "Retro": {"grain": 1}, "NegativeFilm": {"grain": 3}}));
    let gate = target_of(&img, grain::GATE);
    let fill = target_of(&img, grain::FILL);
    // one block: gate stub, fill stub, table; from the planner's pool
    let block = gate;
    assert_eq!(&grain::code(block)[..], &(0..23).map(|k| word(&img, block + 4 * k)).collect::<Vec<_>>()[..]);
    assert_eq!(fill, block + 13 * 4);
    assert_eq!(word(&img, grain::FILL) >> 24, 0xeb, "the fill site calls (bl)");
    assert_eq!(word(&img, grain::GATE) >> 24, 0xea, "the gate site branches (b)");
    let t = file_offset(block + 23 * 4);
    let mut want = [0u8; grain::LOOKS];
    want[0] = 2;   // Standard
    want[8] = 1;   // Retro
    want[11] = 3;  // Negative Film
    assert_eq!(&img[t..t + grain::LOOKS], &want);
    assert!(block >= 0x5300_0d50 && block + grain::SIZE <= 0x5300_4000, "{block:#x}");
    assert_eq!(&stock[file_offset(block)..file_offset(block + grain::SIZE)], &vec![0u8; grain::SIZE as usize][..]);
    // the preset keeps it and gives the same file again
    assert_eq!(preset["targets"]["Standard"]["grain"], json!(2));
    let (again, _) = export(&s, preset["targets"].clone());
    assert_eq!(again, img);

    // only the two sites and the block differ from the same targets without grain
    let (plain, _) = export(&s, json!({"Standard": {}, "Retro": {}, "NegativeFilm": {}}));
    let sites = [file_offset(grain::GATE), file_offset(grain::FILL)];
    let (lo, hi) = (file_offset(block), file_offset(block + grain::SIZE));
    // the image checksum: its last word (grfw::fix_image_checksum)
    let checksum = img.len() - 4..img.len();
    for (k, (a, b)) in img.iter().zip(&plain).enumerate() {
        if a != b {
            assert!(sites.iter().any(|&o| (o..o + 4).contains(&k)) || (lo..hi).contains(&k) || checksum.contains(&k),
                    "byte {k:#x} changed");
        }
    }
}

#[test]
fn no_grain_no_patch() {
    let (s, stock) = need!(session());
    for targets in [json!({"Vivid": {"source": "NegativeFilm"}}), json!({"Standard": {"grain": 0}})] {
        let (img, _) = export(&s, targets);
        assert_eq!(word(&img, grain::GATE), word(&stock, grain::GATE));
        assert_eq!(word(&img, grain::FILL), word(&stock, grain::FILL));
    }
}

#[test]
fn session_keeps_grain() {
    let (mut s, _) = need!(session());
    let fw = s.set_target(&json!({"target": "Vivid", "grain": 2})).unwrap();
    let vivid = |fw: &Value| fw["targets"].as_array().unwrap().iter().find(|t| t["name"] == "Vivid").cloned();
    assert_eq!(vivid(&fw).unwrap()["grain"], json!(2));
    // a change of source keeps it; so does undoing the source
    let fw = s.set_target(&json!({"target": "Vivid", "source": "NegativeFilm"})).unwrap();
    assert_eq!(vivid(&fw).unwrap()["grain"], json!(2));
    let fw = s.set_inherit(&json!({"target": "Vivid", "source": null})).unwrap();
    assert_eq!(vivid(&fw).unwrap()["grain"], json!(2));
    assert!(s.set_target(&json!({"target": "Vivid", "grain": 4})).is_err());
    let fw = s.set_target(&json!({"target": "Vivid", "grain": 0})).unwrap();
    assert_eq!(vivid(&fw).unwrap()["grain"], json!(0));
    let err = s.export(&json!({"targets": {"Vivid": {"grain": 7}}})).err().unwrap();
    assert!(err.contains("grain"), "{err}");
    assert!(s.export(&json!({"targets": {"MonotoneGR": {"grain": 1}}})).is_err());
}

#[test]
fn grain_with_post_matrix() {
    let (mut s, _) = need!(session());
    // a fitted film: a post matrix on the same target, and one without grain
    let p = json!([0.974, 0.0928, -0.0668, 0.081, 0.8078, 0.1112, 0.013, 0.1346, 0.8524]);
    let (img, preset) = export(&s, json!({"NegativeFilm": {"post": p, "grain": 2}, "Standard": {"post": p}}));
    assert_eq!(preset["targets"]["NegativeFilm"]["grain"], json!(2));
    assert_eq!(preset["targets"]["NegativeFilm"]["post"], p);
    let block = target_of(&img, grain::GATE);
    // only the two sites, the block and the checksum differ from the same targets without grain
    let (plain, _) = export(&s, json!({"NegativeFilm": {"post": p}, "Standard": {"post": p}}));
    let sites = [file_offset(grain::GATE), file_offset(grain::FILL)];
    let (lo, hi) = (file_offset(block), file_offset(block + grain::SIZE));
    let checksum = img.len() - 4..img.len();
    for (k, (a, b)) in img.iter().zip(&plain).enumerate() {
        if a != b {
            assert!(sites.iter().any(|&o| (o..o + 4).contains(&k)) || (lo..hi).contains(&k) || checksum.contains(&k),
                    "byte {k:#x} changed");
        }
    }
    // the session keeps each when the other changes
    let neg = |fw: &Value| fw["targets"].as_array().unwrap().iter().find(|t| t["name"] == "NegativeFilm").cloned().unwrap();
    s.set_target(&json!({"target": "NegativeFilm", "post": p})).unwrap();
    let fw = s.set_target(&json!({"target": "NegativeFilm", "grain": 1})).unwrap();
    assert_eq!(neg(&fw)["post"], p);
    let fw = s.set_target(&json!({"target": "NegativeFilm", "post": null})).unwrap();
    assert_eq!(neg(&fw)["grain"], json!(1));
}
