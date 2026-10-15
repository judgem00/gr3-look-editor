//! Post-gamma matrix P per target (docs/plan-post-matrix.md, Phase 1).
//! Unicorn checks the model of the YCbCr stage in tools/verify-tuning.py q5;
//! here: what the export writes, and that nothing else changes.

mod common;
use common::need;

use grlook::addr::word;
use grlook::grfw::{self, Container};
use grlook::postmatrix::{self, SIZE, TONE_TABLE};
use grlook::session::Session;
use grlook::space;
use serde_json::{json, Value};

/// Portra 400 against spektrafilm (docs/spektrafilm-fit.md)
const PORTRA: [f64; 9] = [0.9740, 0.0928, -0.0668, 0.0810, 0.8078, 0.1112, 0.0130, 0.1346, 0.8524];
const SWAP: [f64; 9] = [0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0];
/// the shared hue table of the colour looks, and its set
const SHARED_SET: u32 = 0x53dc_555c;

fn session() -> Option<(Session, Vec<u8>)> {
    let f = common::update_file()?;
    let stock = Container::parse(f.clone()).unwrap().decode().unwrap();
    Some((Session::open(f, "fwdc239b.bin").unwrap(), stock))
}

fn export(s: &Session, targets: Value) -> (Value, Vec<u8>, Value) {
    let (r, packed, preset) = s.export(&json!({ "targets": targets })).unwrap();
    (r, Container::parse(packed).unwrap().decode().unwrap(), serde_json::from_str(&preset).unwrap())
}

/// H(k) B P from the stock hue matrices, in register units
fn ideal(stock: &[u8], p: &[f64; 9]) -> Vec<[f64; 9]> {
    let b = postmatrix::base(stock).map(|v| v as f64);
    postmatrix::hue_matrices(stock, 11).iter().map(|h| {
        let hb: [f64; 9] = std::array::from_fn(|n| (0..3).map(|k| h[3 * (n / 3) + k] as f64 / 8192.0 * b[3 * k + n % 3]).sum());
        std::array::from_fn(|n| (0..3).map(|k| hb[3 * (n / 3) + k] * p[3 * k + n % 3]).sum())
    }).collect()
}

/// the registers the camera computes for `table` in `img`, per hue step
fn registers(img: &[u8], table: u32) -> Vec<[i32; 9]> {
    let base = postmatrix::base(img);
    postmatrix::hue_matrices(img, table).iter().map(|h| postmatrix::registers(h, &base)).collect()
}

#[test]
fn stock_hue_tables_are_shared() {
    let (_, stock) = need!(session());
    for t in [11, 12, 17, 18, 19, 20, 21, 22, 23, 25] {
        assert_eq!(word(&stock, TONE_TABLE + 4 * t), SHARED_SET, "table {t}");
    }
}

#[test]
fn only_the_item_and_the_entry_change() {
    let (s, stock) = need!(session());
    let (r, img, preset) = export(&s, json!({"NegativeFilm": {"post": PORTRA}}));
    // where space places one item of SIZE bytes, and what must be there
    let at = space::plan_with(&stock, 0, 0, &[SIZE]).unwrap().items[0];
    let mut want = stock.clone();
    postmatrix::install(&mut want, &[25], at, &PORTRA).unwrap();
    grfw::fix_image_checksum(&mut want);
    assert!(img == want, "the export differs from stock + item + entry");
    // the item: set word 0 -> hue table -> nine matrices inside the item
    assert_eq!(word(&img, TONE_TABLE + 4 * 25), at);
    assert_eq!(word(&img, at), at + 4);
    for k in 0..9 {
        let m = word(&img, at + 4 + 4 * k);
        assert!(m >= at + 0x28 && m + 36 <= at + SIZE, "matrix {k} at {m:#x}");
    }
    // the camera's registers: H(k) B P within one step, grey exact
    for (k, (got, id)) in registers(&img, 25).iter().zip(ideal(&stock, &PORTRA)).enumerate() {
        for n in 0..9 {
            assert!((got[n] as f64 - id[n]).abs() <= 1.0, "hue {k}: {got:?} vs {id:?}");
        }
        assert_eq!([got[0] + got[1] + got[2], got[3] + got[4] + got[5], got[6] + got[7] + got[8]], [256, 0, 0]);
    }
    // every other look keeps the stock registers
    for t in [11, 12, 17, 18, 19, 20, 21, 22, 23] {
        assert_eq!(registers(&img, t), registers(&stock, t), "table {t}");
    }
    // the preset keeps P and gives the same file again
    assert_eq!(preset["targets"]["NegativeFilm"]["post"], json!(PORTRA));
    let (again, _, _) = export(&s, preset["targets"].clone());
    assert_eq!(again["update_sha256"], r["update_sha256"]);
    assert_eq!(r["space"]["items"], json!(1));
}

#[test]
fn several_targets_get_their_own() {
    let (s, stock) = need!(session());
    let (_, img, _) = export(&s, json!({
        "PositiveFilm": {"post": SWAP},
        "Vivid": {"source": "NegativeFilm", "post": PORTRA},
        "BleechBypass_GR": {"post": PORTRA},
        "Standard": {},
    }));
    let pf = word(&img, TONE_TABLE + 4 * 17);
    let vivid = word(&img, TONE_TABLE + 4 * 12);
    let bb = word(&img, TONE_TABLE + 4 * 18);
    assert!(pf != vivid && vivid != bb && pf != bb && ![pf, vivid, bb].contains(&SHARED_SET));
    // Bleach Bypass with its own source: P on all three variants
    assert_eq!((word(&img, TONE_TABLE + 4 * 22), word(&img, TONE_TABLE + 4 * 23)), (bb, bb));
    // the swap: Y takes B's weight for R and R's for B
    let base = postmatrix::base(&stock);
    let r = &registers(&img, 17)[4];
    assert_eq!(&r[0..3], &[base[2], base[1], base[0]]);
    // Standard (a target without P) and the looks that are no target keep theirs
    for t in [11, 19, 20, 21, 25] {
        assert_eq!(word(&img, TONE_TABLE + 4 * t), SHARED_SET, "table {t}");
    }
    // a look written with ColorMatrix data from elsewhere keeps P on its own entry
    assert_eq!(registers(&img, 12)[4], registers(&{
        let (_, i, _) = export(&s, json!({"NegativeFilm": {"post": PORTRA}}));
        i
    }, 25)[4]);
}

#[test]
fn identity_or_none_is_stock() {
    let (s, _) = need!(session());
    let (plain, _, _) = export(&s, json!({"Vivid": {}}));
    let (id, _, _) = export(&s, json!({"Vivid": {"post": [1, 0, 0, 0, 1, 0, 0, 0, 1]}}));
    let (null, _, _) = export(&s, json!({"Vivid": {"post": null}}));
    assert_eq!(plain["update_sha256"], id["update_sha256"]);
    assert_eq!(plain["update_sha256"], null["update_sha256"]);
}

#[test]
fn refuses_what_it_cannot_do() {
    let (s, _) = need!(session());
    let strong = [3.0, -1.0, -1.0, -1.0, 3.0, -1.0, -1.0, -1.0, 3.0];
    for (post, msg) in [
        (json!([1.2, 0, 0, 0, 1, 0, 0, 0, 1]), "row 1 sums"),
        (json!([1, 0, 0]), "nine numbers"),
        (json!(strong), "too strong"),
    ] {
        let err = s.export(&json!({"targets": {"Vivid": {"post": post}}})).err()
            .unwrap_or_else(|| panic!("{post}: accepted"));
        assert!(err.contains(msg), "{post}: {err}");
    }
}

#[test]
fn item_fits_next_to_everything_else() {
    let (_, stock) = need!(session());
    // complete film looks: HSL, RGB curves, a private set and P each. P does
    // not change how many fit (3; HSL and curves are the limit)
    let film = |n: usize, p: bool| {
        let mut items = vec![grlook::looks::SET_SIZE; n];
        if p {
            items.extend(vec![SIZE; n]);
        }
        space::plan_with(&stock, n, 3 * n, &items).is_ok()
    };
    for n in 1..=4 {
        assert_eq!(film(n, true), film(n, false), "{n} film looks");
    }
    assert!(film(3, true) && !film(4, false));
}
