//! The session against the reference answers (work/golden, examples/golden.rs).

mod common;
use common::need;

use grlook::addr::file_offset;
use grlook::grfw::{self, Container};
use grlook::session::Session;

/// the private colour set of Cross Process (looks.rs)
const CROSS_SET: u32 = 0x5300_2080;
use serde_json::{json, Value};

/// first difference between two JSON values; numbers may differ by `tol`
pub fn diff(a: &Value, b: &Value, path: &str, tol: f64) -> Option<String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            ((x - y).abs() > tol * (1.0 + y.abs())).then(|| format!("{path}: {x} vs {y}"))
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: length {} vs {}", x.len(), y.len()));
            }
            x.iter().zip(y).enumerate().find_map(|(i, (p, q))| diff(p, q, &format!("{path}[{i}]"), tol))
        }
        (Value::Object(x), Value::Object(y)) => {
            for k in y.keys() {
                if !x.contains_key(k) {
                    return Some(format!("{path}.{k}: missing"));
                }
            }
            x.iter().find_map(|(k, v)| match y.get(k) {
                Some(w) => diff(v, w, &format!("{path}.{k}"), tol),
                None => Some(format!("{path}.{k}: extra")),
            })
        }
        _ => (a != b).then(|| format!("{path}: {a} vs {b}")),
    }
}

/// newer than the reference answers: the curves of the other looks (12..)
fn cross_only(mut v: Value) -> Value {
    for k in ["channel_curves", "channel_curves_pre"] {
        v[k].as_array_mut().unwrap().truncate(12);
    }
    v
}

fn session() -> Option<Session> {
    Some(Session::open(common::update_file()?, "fwdc239b.bin").unwrap())
}

#[test]
fn firmware_tone_inherit_export() {
    let g = need!(common::golden());
    let mut s = need!(session());
    let mut fw = s.firmware();
    assert_eq!((&fw["camera"], &fw["version"], &fw["update_file"]), (&json!("GR III"), &json!("2.10"), &json!("FWDC239B.BIN")));
    // the web version names the build, not a path on disk
    fw["path"] = json!("work/fwdc239b/unpackedFW");
    // which camera the file is for: newer than the reference answers
    for k in ["camera", "version", "update_file", "multiaxial", "tone_control_base", "effects_off", "curve_looks", "look_channel_sets",
              "targets", "target_names", "sources"] {
        fw.as_object_mut().unwrap().remove(k);
    }
    if let Some(d) = diff(&fw, &g["firmware"], "firmware", 1e-12) {
        panic!("{d}");
    }
    for (n, t) in g["tone"].as_array().unwrap().iter().enumerate() {
        let got = cross_only(s.tone(&t["request"]).unwrap());
        if let Some(d) = diff(&got, &t["response"], &format!("tone[{n}]"), 1e-12) {
            panic!("{d}");
        }
    }
    let mut inh = s.set_inherit(&json!({"target": "CrossProcess", "source": "NegativeFilm"})).unwrap();
    inh["path"] = json!("work/fwdc239b/unpackedFW");
    // which camera the file is for: newer than the reference answers
    for k in ["camera", "version", "update_file", "multiaxial", "tone_control_base", "effects_off", "curve_looks", "look_channel_sets",
              "targets", "target_names", "sources"] {
        inh.as_object_mut().unwrap().remove(k);
    }
    if let Some(d) = diff(&inh, &g["inherit"]["NegativeFilm"], "inherit", 1e-12) {
        panic!("{d}");
    }
    let tone = cross_only(s.tone(&g["tone"][1]["request"]).unwrap());
    assert!(diff(&tone, &g["tone_inherited"], "tone_inherited", 1e-12).is_none());

    for (n, e) in g["export"].as_array().unwrap().iter().enumerate() {
        s.set_inherit(&json!({"target": "CrossProcess", "source": e["inherit"]})).unwrap();
        let (r, packed, preset) = s.export(&e["request"]).unwrap();
        let tone = !e["request"]["tone"].is_null();
        let retro = e["inherit"] == json!("Retro");
        if tone {
            // the tone curve for all image controls is no longer written
            assert_eq!(r["tone_ignored"], json!(true), "export {n}");
            continue;
        }
        if retro {
            // Cross Process built from Retro now reads Retro's neutral toning
            // (index 4) instead of index 0 (toning -4): the four toning
            // pointers of its private set move by 8 bytes, nothing else
            let mut img = Container::parse(packed).unwrap().decode().unwrap();
            for w in 11..15u32 {
                let at = file_offset(CROSS_SET + 4 * w);
                let v = u32::from_le_bytes(img[at..at + 4].try_into().unwrap());
                img[at..at + 4].copy_from_slice(&(v - 8).to_le_bytes());
            }
            grfw::fix_image_checksum(&mut img);
            let raw = common::update_file().unwrap();
            let mut c = Container::parse(raw).unwrap();
            let stock = c.decode().unwrap();
            let (old, _) = grfw::repack(&c, &stock, &img).unwrap();
            assert_eq!(json!(grlook::session::sha256_hex(&old)), e["update_sha256"], "export {n}: more than the toning differs");
        } else {
            assert_eq!(packed.len() as u64, e["update_size"].as_u64().unwrap(), "export {n}: size");
            assert_eq!(r["update_sha256"], e["update_sha256"], "export {n}: update file differs");
            assert_eq!(r["frames_reencoded"], e["frames_reencoded"], "export {n}");
        }
        let mut preset: Value = serde_json::from_str(&preset).unwrap();
        let mut want = e["preset"].clone();
        preset["source"] = json!(null);
        want["source"] = json!(null);
        // newer than the reference answers: HSL (none in these requests)
        // and per-look curves, effects off (none either)
        for k in ["hsl", "look_curves", "effects_off"] {
            assert_eq!(preset.as_object_mut().unwrap().remove(k), Some(Value::Null), "{k}");
        }
        if let Some(d) = diff(&preset, &want, &format!("preset[{n}]"), 1e-12) {
            panic!("{d}");
        }
    }
}

#[test]
fn refuses_unknown_and_damaged_files() {
    let raw = need!(common::update_file());
    let mut bad = raw.clone();
    bad[0x1000] ^= 1;
    assert!(Session::open(bad, "x.bin").err().unwrap().contains("unknown firmware"));
    assert!(Session::open(b"hello".to_vec(), "x.bin").is_err());
}
