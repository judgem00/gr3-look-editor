//! DNG decoding against rawpy/LibRaw (frozen from the earlier Python
//! editor), and the shot fit against the reference answers.

mod common;
use common::need;

use grlook::{dng, session::Session};

fn f32s(path: &std::path::Path) -> Vec<f32> {
    std::fs::read(path).unwrap().chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect()
}

#[test]
fn dngs_match_reference() {
    let g = need!(common::golden());
    let s = need!(common::update_file().map(|u| Session::open(u, "fwdc239b.bin").unwrap()));
    let dir = common::work().join("golden");
    for case in g["dng"].as_array().unwrap() {
        let stem = case["stem"].as_str().unwrap();
        let Ok(data) = std::fs::read(case["path"].as_str().unwrap()) else {
            eprintln!("{stem}: DNG not found, skipped");
            continue;
        };
        let t = std::time::Instant::now();
        let shot = dng::decode(&data).unwrap();
        let decode_ms = t.elapsed().as_millis();
        assert!(shot.jpeg.is_some(), "{stem}: no embedded JPEG");

        // the fit from the very pixels the reference used (the camera JPEG as decoded then)
        let cam: Vec<u8> = std::fs::read(dir.join(format!("{stem}.cam.u8"))).unwrap()
            .chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        let t = std::time::Instant::now();
        let name = std::path::Path::new(case["path"].as_str().unwrap()).file_name().unwrap().to_str().unwrap();
        // the reference fit knows no lens shading correction
        let p = s.prepare_shot_with(&shot, Some(&cam), name, false).unwrap();
        let fit_ms = t.elapsed().as_millis();
        let want = &case["meta"];
        let m = &p.meta;
        for k in ["width", "height", "model", "image_tone", "shot_look", "params", "filename"] {
            assert_eq!(m[k], want[k], "{stem}: {k}");
        }
        for (a, b) in m["wb"].as_array().unwrap().iter().zip(want["wb"].as_array().unwrap()) {
            assert!((a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 1e-12, "{stem}: wb");
        }

        let lin = f32s(&dir.join(format!("{stem}.linear.f32")));
        assert_eq!(lin.len(), p.linear.len(), "{stem}: linear size");
        let worst = lin.iter().zip(&p.linear).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(worst < 1e-6, "{stem}: linear differs by {worst}");

        // compared loosely: references from the Python editor sampled pixels with numpy
        let (gw, gm) = (want["gain"].as_f64().unwrap(), m["gain"].as_f64().unwrap());
        let (sw, sm) = (want["scale"].as_f64().unwrap(), m["scale"].as_f64().unwrap());
        let rw: Vec<f64> = serde_json::from_value(want["residual"].clone()).unwrap();
        let rm: Vec<f64> = serde_json::from_value(m["residual"].clone()).unwrap();
        let res = rw.iter().zip(&rm).map(|(a, b)| (a - b).abs()).sum::<f64>() / rw.len() as f64;
        eprintln!("{stem}: decode {decode_ms} ms, fit {fit_ms} ms; gain {gm} vs {gw}, scale {sm:.0} vs {sw:.0}, residual mean diff {res:.4}");
        // effective brightness: the curve sees gain and scale together
        let ratio = (gm * sm) / (gw * sw);
        assert!((0.8..1.25).contains(&ratio), "{stem}: gain x scale off by {ratio}");
        // the Python editor itself moved by up to ~0.011 between random seeds
        assert!(res < 0.02, "{stem}: residual off by {res}");
    }
}
