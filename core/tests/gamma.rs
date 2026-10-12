//! GammaTuning in the interpreter against Unicorn (golden/make.py gamma_grid).

mod common;
use common::need;

use grlook::gamma::{self, Engine, ToneParams};
use grlook::tone;

#[test]
fn gamma_matches_unicorn() {
    let g = need!(common::golden());
    let image = need!(common::stock_image());
    let raw = std::fs::read(common::work().join("golden/gamma.u16")).unwrap();
    let curves: Vec<u16> = raw.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    let base = gamma::base_curves(&image);
    let plain = Engine::new(&image, &[]);
    let mut bad = 0;
    for (n, c) in g["gamma_cases"].as_array().unwrap().iter().enumerate() {
        let p = ToneParams {
            key: c["key"].as_i64().unwrap() as i32,
            contrast: c["contrast"].as_i64().unwrap() as i32,
            highlight: c["highlight"].as_i64().unwrap() as i32,
            shadow: c["shadow"].as_i64().unwrap() as i32,
            gain: c["gain"].as_i64().unwrap() as i32,
            flag: c["flag"].as_u64().unwrap() as u8,
        };
        let got = if c["edit"].is_null() {
            plain.curve(&image, p).unwrap()
        } else {
            let pts: Vec<[f64; 2]> = serde_json::from_value(c["edit"]["points"].clone()).unwrap();
            let fams: Vec<usize> = serde_json::from_value(c["edit"]["families"].clone()).unwrap();
            let edit: Vec<(usize, Vec<u16>)> = tone::apply(&base, &pts, &fams)
                .into_iter()
                .map(|(i, v)| (i, v.into_iter().map(|x| x.clamp(0, 0xFFFF) as u16).collect()))
                .collect();
            Engine::new(&image, &edit).curve(&image, p).unwrap()
        };
        let want = &curves[n * 256..(n + 1) * 256];
        if got != want {
            bad += 1;
            if bad <= 5 {
                let i = (0..256).find(|&i| got[i] != want[i]).unwrap();
                eprintln!("case {n} {c}: first difference at {i}: {} vs {}", got[i], want[i]);
            }
        }
    }
    assert_eq!(bad, 0, "{bad} curves differ");
}
