//! Targets with a colour matrix of their own (docs/plan-gui-redesign.md,
//! Phase 1). Unicorn checks the exported image in tools/verify-targets.py
//! (--patched); here: what lands where, and that nothing else changes.

mod common;
use common::need;

use grlook::addr::{file_offset, word};
use grlook::grfw::Container;
use grlook::looks::{self, SET_SIZE, ZERO_TONING};
use grlook::session::Session;
use grlook::{curvesel, space, targets};
use serde_json::{json, Value};

const LOOK_TABLES: [u32; 3] = [0x5501_8d94, 0x5501_8cf0, 0x5501_8e4c];
const CM: u32 = LOOK_TABLES[0];

fn session() -> Option<(Session, Vec<u8>)> {
    let f = common::update_file()?;
    let stock = Container::parse(f.clone()).unwrap().decode().unwrap();
    Some((Session::open(f, "fwdc239b.bin").unwrap(), stock))
}

fn decode(packed: Vec<u8>) -> Vec<u8> {
    Container::parse(packed).unwrap().decode().unwrap()
}

fn matrix(img: &[u8], table: u32) -> Vec<u8> {
    let o = file_offset(looks::look_matrix_address(img, table));
    img[o..o + 18].to_vec()
}

fn set_of(img: &[u8], table: u32) -> u32 {
    word(img, CM + 4 * table)
}

/// a look the export must leave alone: its three table entries, its set and
/// matrix, and the toning tables the set points at are stock
fn assert_stock(img: &[u8], stock: &[u8], table: u32, what: &str) {
    for t in LOOK_TABLES {
        assert_eq!(word(img, t + 4 * table), word(stock, t + 4 * table), "{what}: entry in {t:#x}");
    }
    let set = set_of(stock, table);
    let (o, n) = (file_offset(set), 4 * 21);
    assert_eq!(&img[o..o + n], &stock[o..o + n], "{what}: its set");
    assert_eq!(matrix(img, table), matrix(stock, table), "{what}: its matrix");
}

/// a saturation-like edit, in floats as the editor sends them
fn edited(img: &[u8], table: u32) -> Value {
    let m: Vec<f64> = matrix(img, table).chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f64 / 8192.0).collect();
    json!((0..9).map(|k| {
        let r = k / 3;
        let s: f64 = m[3 * r..3 * r + 3].iter().sum();
        m[k] * 1.25 - s * 0.25 / 3.0
    }).collect::<Vec<_>>())
}

fn as_fixed(v: &Value) -> Vec<u8> {
    v.as_array().unwrap().iter()
        .flat_map(|x| ((x.as_f64().unwrap() * 8192.0).round_ties_even() as i16).to_le_bytes())
        .collect()
}

#[test]
fn no_targets_is_stock() {
    let (s, stock) = need!(session());
    let (_, packed, preset) = s.export(&json!({"targets": {}})).unwrap();
    assert_eq!(packed, common::update_file().unwrap(), "an export without targets is Ricoh's file");
    let p: Value = serde_json::from_str(&preset).unwrap();
    assert_eq!((p["version"].clone(), p["targets"].clone()), (json!(2), json!({})));
    drop(stock);
}

#[test]
fn every_target_is_independent() {
    let (s, stock) = need!(session());
    let t = |n: &str| looks::table_of(n).unwrap();
    let vivid_m = edited(&stock, t("NegativeFilm"));
    let std_m = edited(&stock, t("Standard"));
    let pf_m = edited(&stock, t("BleechBypass_GR · Var. 3"));
    let hdr_m = edited(&stock, t("Standard"));
    let body = json!({"targets": {
        "Vivid": {"source": "NegativeFilm", "matrices": {"Vivid": vivid_m}},
        "Standard": {"matrices": {"Standard": std_m}},
        "PositiveFilm": {"source": "BleechBypass_GR · Var. 3", "matrices": {"PositiveFilm": pf_m}},
        "BleechBypass_GR": {"source": "PositiveFilm"},
        "Retro": {"source": "BleechBypass_GR · Var. 2"},
        "HdrCuisine": {"source": "Standard", "effects_off": true, "matrices": {"HdrCuisine": hdr_m}},
        "CrossProcess": {"source": "Retro"},
    }});
    let (r, packed, preset) = s.export(&body).unwrap();
    let img = decode(packed);
    // Negative Film is a source only: untouched
    assert_stock(&img, &stock, t("NegativeFilm"), "Negative Film");
    // targets: their matrices, in sets of their own
    assert_eq!(matrix(&img, t("Vivid")), as_fixed(&vivid_m));
    assert_eq!(matrix(&img, t("Standard")), as_fixed(&std_m));
    assert_eq!(matrix(&img, t("PositiveFilm")), as_fixed(&pf_m));
    assert_eq!(matrix(&img, t("HdrCuisine")), as_fixed(&hdr_m));
    let names = &targets::TARGETS[..7];
    let sets: Vec<u32> = names.iter().map(|n| set_of(&img, t(n))).collect();
    let mut uniq = sets.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), sets.len(), "every target has a set of its own: {sets:x?}");
    for (n, &set) in names.iter().zip(&sets) {
        assert_ne!(set_of(&stock, t(n)), set, "{n}: still the stock set");
    }
    // Bleach Bypass from another source: one look over its three tables
    let bb = set_of(&img, 18);
    assert_eq!((set_of(&img, 22), set_of(&img, 23)), (bb, bb));
    for tb in [22, 23] {
        for lt in &LOOK_TABLES[1..] {
            assert_eq!(word(&img, lt + 4 * tb), word(&stock, lt + 4 * t("PositiveFilm")));
        }
    }
    // toning: index-0 targets shift by the source's index; Retro and Bleach
    // Bypass (own toning menu) repeat the source's entry or read zeros
    for w in 11..15 {
        assert_eq!(word(&img, set_of(&img, 17) + 4 * w), word(&stock, set_of(&stock, 23) + 4 * w) + 6, "PF word {w}");
        assert_eq!(word(&img, set_of(&img, 21) + 4 * w), word(&stock, set_of(&stock, 19) + 4 * w) + 8, "Cross word {w}");
        assert_eq!(word(&img, bb + 4 * w), ZERO_TONING, "BB word {w}");
    }
    let retro = set_of(&img, 19);
    for (w, want) in [(13, 40i16), (14, -60)] {
        let tb = word(&img, retro + 4 * w);
        for k in 0..9 {
            assert_eq!(grlook::addr::i16_at(&img, file_offset(tb + 2 * k)), want, "Retro toning word {w} index {k}");
        }
    }
    // HDR without its effects
    for (site, _, w) in curvesel::HDR_OFF {
        assert_eq!(word(&img, site), w);
    }
    // the preset gives the same file again
    let p: Value = serde_json::from_str(&preset).unwrap();
    assert_eq!(p["targets"]["Vivid"]["source"], json!("NegativeFilm"));
    assert_eq!(p["targets"]["Standard"]["source"], json!("Standard"));
    let (again, _, _) = s.export(&json!({"targets": p["targets"]})).unwrap();
    assert_eq!(again["update_sha256"], r["update_sha256"]);
    // the meter counts what was placed: 5 sets and Retro's toning tables
    // (Cross Process and HDR have fixed sets)
    assert_eq!(r["space"]["items"], json!(6));
    if std::env::var("TARGETS_DUMP").is_ok() {
        let dir = common::work().join("targets-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("unpackedFW"), &img).unwrap();
        std::fs::write(dir.join("preset.looks.json"), preset).unwrap();
    }
}

#[test]
fn own_source_keeps_the_stock_structure() {
    let (s, stock) = need!(session());
    let m = edited(&stock, 18);
    let body = json!({"targets": {
        "BleechBypass_GR": {"matrices": {"BleechBypass_GR": m.clone(), "BleechBypass_GR · Var. 2": m}},
        "Retro": {"hsl": {"bands": [{"center": 2400, "width": 700, "sat": -30}]}},
    }});
    let img = decode(s.export(&body).unwrap().1);
    // the variants keep their own sets, edited in place; Retro keeps its toning
    for tb in [18, 22, 23, 19] {
        assert_eq!(set_of(&img, tb), set_of(&stock, tb), "table {tb}");
    }
    assert_ne!(matrix(&img, 22), matrix(&stock, 22));
    assert_eq!(matrix(&img, 23), matrix(&stock, 23));
    // no private sets: nothing in the tail of the reserved block
    let (lo, hi) = (file_offset(space::TAIL.0), file_offset(space::TAIL.1));
    assert!(img[lo..hi].iter().all(|&b| b == 0));
}

#[test]
fn refuses_what_it_cannot_do() {
    let (s, _) = need!(session());
    let lin: Vec<Vec<u32>> = vec![(0..256).map(|k| k * 64).collect(); 3];
    for (body, msg) in [
        (json!({"MonotoneGR": {}}), "cannot be a target"),
        (json!({"Vivid": {"source": "HardMonotone"}}), "cannot be a source"),
        (json!({"Vivid": {"source": "HdrCuisine · Var."}}), "cannot be a source"),
        (json!({"HdrCuisine": {"source": "Vivid"}}), "only with its effects off"),
        (json!({"HdrCuisine": {"matrices": {"HdrCuisine": [1, 0, 0, 0, 1, 0, 0, 0, 1]}}}), "only with its effects off"),
        (json!({"Vivid": {"effects_off": true}}), "effects can only"),
        (json!({"Vivid": {"matrices": {"NegativeFilm": [1, 0, 0, 0, 1, 0, 0, 0, 1]}}}), "cannot write the matrix"),
        (json!({"CrossProcess": {"curves": lin}}), "Color Tone"),
        (json!({"Vivid": {"channels": {"3": []}}}), "use curves"),
    ] {
        let err = s.export(&json!({"targets": body})).err().unwrap_or_else(|| panic!("{body}: accepted"));
        assert!(err.contains(msg), "{body}: {err}");
    }
}

#[test]
fn sets_share_the_pool() {
    let (_, stock) = need!(session());
    // three sets fit into the tail of the reserved block, more go below the padding
    let p = space::plan_with(&stock, 0, 0, &[SET_SIZE; 4]).unwrap();
    assert_eq!(&p.items[..3], &[space::TAIL.0, space::TAIL.0 + SET_SIZE, space::TAIL.0 + 2 * SET_SIZE]);
    assert!(p.items[3] + SET_SIZE <= 0x5300_2000);
    // with every HSL slot taken: the tail (3) and the two gaps after the
    // slots (160 and 112 bytes, one set each)
    let p = space::plan_with(&stock, 5, 0, &[SET_SIZE; 5]).unwrap();
    for &a in &p.items {
        for &h in &p.hsl {
            assert!(a + SET_SIZE <= h || a >= h + space::HSL_SIZE, "{a:#x} in the HSL set at {h:#x}");
        }
    }
    let err = space::plan_with(&stock, 5, 0, &[SET_SIZE; 6]).unwrap_err();
    assert!(err.contains("private colour sets"), "{err}");
    // items and curves never overlap
    let p = space::plan_with(&stock, 3, 9, &[SET_SIZE; 6]).unwrap();
    for &a in &p.items {
        for &c in &p.curves {
            assert!(a + SET_SIZE <= c || a >= c + space::CURVE_SIZE, "{a:#x} {c:#x}");
        }
    }
}

#[test]
fn session_targets_drive_the_preview() {
    let (mut s, stock) = need!(session());
    let fw = s.set_target(&json!({"target": "Vivid", "source": "NegativeFilm"})).unwrap();
    let looks = fw["looks"].as_array().unwrap();
    let get = |n: &str| looks.iter().find(|l| l["name"] == n).unwrap().clone();
    assert_eq!(get("Vivid")["matrix"], get("NegativeFilm")["matrix"]);
    assert_ne!(get("Vivid")["address"], get("NegativeFilm")["address"]);
    assert_eq!(fw["inherit"], json!({"Vivid": "NegativeFilm"}));
    assert!(fw["targets"].as_array().unwrap().iter().any(|t| t["name"] == "Vivid"));
    // the earlier editor's calls still work on top
    assert!(s.set_inherit(&json!({"target": "HdrCuisine", "source": "Vivid"})).is_err());
    s.set_effects(&json!({"effects_off": ["HdrCuisine"]})).unwrap();
    let fw = s.set_inherit(&json!({"target": "HdrCuisine", "source": "Vivid"})).unwrap();
    assert_eq!(fw["inherit"]["HdrCuisine"], json!("Vivid"));
    assert!(s.set_effects(&json!({"effects_off": []})).is_err(), "HDR from Vivid needs its effects off");
    let fw = s.set_target(&json!({"target": "Vivid", "remove": true})).unwrap();
    assert!(fw["inherit"].get("Vivid").is_none());
    drop(stock);
}
