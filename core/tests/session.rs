//! The session against the reference answers (work/golden, examples/golden.rs).

mod common;
use common::need;

use grlook::session::Session;
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
    for k in ["camera", "version", "update_file", "multiaxial", "tone_control_base"] {
        fw.as_object_mut().unwrap().remove(k);
    }
    if let Some(d) = diff(&fw, &g["firmware"], "firmware", 1e-12) {
        panic!("{d}");
    }
    for (n, t) in g["tone"].as_array().unwrap().iter().enumerate() {
        let got = s.tone(&t["request"]).unwrap();
        if let Some(d) = diff(&got, &t["response"], &format!("tone[{n}]"), 1e-12) {
            panic!("{d}");
        }
    }
    let mut inh = s.set_inherit(&json!({"target": "CrossProcess", "source": "NegativeFilm"})).unwrap();
    inh["path"] = json!("work/fwdc239b/unpackedFW");
    // which camera the file is for: newer than the reference answers
    for k in ["camera", "version", "update_file", "multiaxial", "tone_control_base"] {
        inh.as_object_mut().unwrap().remove(k);
    }
    if let Some(d) = diff(&inh, &g["inherit"]["NegativeFilm"], "inherit", 1e-12) {
        panic!("{d}");
    }
    let tone = s.tone(&g["tone"][1]["request"]).unwrap();
    assert!(diff(&tone, &g["tone_inherited"], "tone_inherited", 1e-12).is_none());

    for (n, e) in g["export"].as_array().unwrap().iter().enumerate() {
        s.set_inherit(&json!({"target": "CrossProcess", "source": e["inherit"]})).unwrap();
        let (r, packed, preset) = s.export(&e["request"]).unwrap();
        assert_eq!(packed.len() as u64, e["update_size"].as_u64().unwrap(), "export {n}: size");
        assert_eq!(r["update_sha256"], e["update_sha256"], "export {n}: update file differs");
        assert_eq!(r["frames_reencoded"], e["frames_reencoded"], "export {n}");
        let mut preset: Value = serde_json::from_str(&preset).unwrap();
        let mut want = e["preset"].clone();
        preset["source"] = json!(null);
        want["source"] = json!(null);
        // newer than the reference answers: HSL (none in these requests)
        assert_eq!(preset.as_object_mut().unwrap().remove("hsl"), Some(Value::Null));
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
