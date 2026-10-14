//! HSL export: each look with HSL gets a MultiAxial set of its own, the
//! others keep the stock one.

mod common;
use common::need;

use grlook::addr::{file_offset, u16_at, word};
use grlook::grfw::Container;
use grlook::multiaxial;
use grlook::session::Session;
use serde_json::json;

fn band(center: f64, sat: f64) -> serde_json::Value {
    json!({"center": center, "width": 700, "feather": 150, "hue": if sat == 0.0 { 0 } else { 10 }, "sat": sat, "lum": 0})
}

#[test]
fn export_writes_one_set_per_look() {
    let raw = need!(common::update_file());
    let s = Session::open(raw, "fwdc239b.bin").unwrap();
    let stock = Container::parse(common::update_file().unwrap()).unwrap().decode().unwrap();
    let stock_set = multiaxial::param_set(&stock, 11);
    let hsl = json!({
        "CrossProcess": {"bands": [band(7700.0, -40.0), band(5000.0, 20.0)], "protect": {"shadows": 512}},
        "PositiveFilm": {"bands": [band(2400.0, -30.0)]},
        "Vivid": {"bands": [band(2400.0, 0.0)]},   // neutral: no set
    });
    let (_, packed, preset) = s.export(&json!({"hsl": hsl})).unwrap();
    let img = Container::parse(packed).unwrap().decode().unwrap();

    for (look, table) in [("CrossProcess", 21), ("PositiveFilm", 17)] {
        let set = multiaxial::param_set(&img, table);
        assert_ne!(set, stock_set, "{look}");
        assert!((0x5300_0d50..0x5300_4000).contains(&set), "{look}: set at {set:#x}");
        let h = multiaxial::Hsl::from_json(&hsl[look]);
        let e = multiaxial::encode(&h, &multiaxial::ycc(&stock, table)).unwrap();
        let p = |k: u32| word(&img, set + 4 * k);
        let axes: Vec<u16> = (0..16).map(|k| u16_at(&img, file_offset(p(0)) + 2 * k)).collect();
        assert_eq!(axes, e.axes.iter().flatten().copied().collect::<Vec<_>>(), "{look}: axes");
        assert_eq!(&img[file_offset(p(1))..file_offset(p(1)) + 20], &e.nibbles, "{look}: nibbles");
        let ctl: Vec<u16> = (0..21).map(|k| u16_at(&img, file_offset(p(4)) + 2 * k)).collect();
        assert_eq!(ctl, e.control.to_vec(), "{look}: control");
        // matrix table behind its header, all five rows of a bank alike
        let table_va = word(&img, p(2) + 0x10);
        for (b, m) in e.banks.iter().enumerate() {
            for row in 0..5 {
                let got: Vec<i32> = (0..9).map(|k| word(&img, table_va + 36 * (5 * b as u32 + row) + 4 * k) as i32).collect();
                assert_eq!(&got, m, "{look}: bank {b} row {row}");
            }
        }
        // region values and YCbCr stay shared with the stock set
        assert_eq!(p(3), word(&stock, stock_set + 12));
        assert_eq!(p(5), word(&stock, stock_set + 20));
    }
    for table in [11, 12, 13, 18, 19, 20, 22, 23, 24, 25] {
        assert_eq!(multiaxial::param_set(&img, table), stock_set, "table {table}");
    }
    // the stock set itself is untouched
    assert_eq!(&img[file_offset(stock_set)..file_offset(stock_set) + 24], &stock[file_offset(stock_set)..file_offset(stock_set) + 24]);
    let preset: serde_json::Value = serde_json::from_str(&preset).unwrap();
    assert_eq!(preset["hsl"].as_object().unwrap().len(), 2);
    assert!(preset["hsl"].get("Vivid").is_none());
    if let Ok(path) = std::env::var("MA_DUMP") {
        std::fs::write(path, &img).unwrap();
    }
}

#[test]
fn at_most_five_looks() {
    let s = need!(Session::open(need!(common::update_file()), "fwdc239b.bin").ok());
    let looks = ["CrossProcess", "PositiveFilm", "Vivid", "Standard", "Retro", "NegativeFilm"];
    let hsl: serde_json::Map<String, serde_json::Value> =
        looks.iter().map(|l| (l.to_string(), json!({"bands": [band(2400.0, -30.0)]}))).collect();
    let err = s.export(&json!({"hsl": hsl})).err().unwrap();
    assert!(err.contains("at most 5"), "{err}");
    let five: serde_json::Map<String, serde_json::Value> = hsl.into_iter().take(5).collect();
    assert!(s.export(&json!({"hsl": five})).is_ok());
}
