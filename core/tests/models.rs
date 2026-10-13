//! GR IIIx 1.60 is supported because it is the GR III 2.10 build with another
//! model identity. These tests hold the editor to that: both files give the
//! same answers, and an export patches the IIIx image exactly where it patches
//! the GR III image, with the same bytes.

mod common;
use common::need;

use grlook::{addr, grfw, session::Session};
use serde_json::{json, Value};

fn sessions() -> Option<(Session, Session)> {
    let gr3 = Session::open(common::update_file()?, "fwdc239b.bin").unwrap();
    let gr3x = Session::open(common::update_file_gr3x()?, "fwdc243b.bin").unwrap();
    Some((gr3, gr3x))
}

/// the firmware answer without what names the file
fn look_data(s: &Session) -> Value {
    let mut fw = s.firmware();
    for k in ["path", "camera", "version", "update_file", "container"] {
        fw.as_object_mut().unwrap().remove(k);
    }
    fw
}

fn decoded(update: Vec<u8>) -> Vec<u8> {
    grfw::Container::parse(update).unwrap().decode().unwrap()
}

/// an export that touches everything the editor can patch: a look matrix, the
/// base tone curves and the channel curves
fn requests() -> Vec<(Value, Value)> {
    let ramp: Vec<f64> = (0..256).map(|i| (i * 60) as f64).collect();
    let mut out = vec![
        (Value::Null, json!({})),
        (json!("PositiveFilm"), json!({
            "matrices": {"CrossProcess": [1.1, -0.05, -0.05, -0.1, 1.2, -0.1, 0.0, -0.2, 1.2]},
            "tone": {"points": [[0.0, 0.0], [0.25, 0.2], [0.75, 0.8], [1.0, 1.0]], "families": [0, 1, 2]},
            "channels": {"3": ramp, "7": ramp, "11": ramp},
        })),
    ];
    if let Some(g) = common::golden() {
        for e in g["export"].as_array().unwrap() {
            out.push((e["inherit"].clone(), e["request"].clone()));
        }
    }
    out
}

#[test]
fn names_the_camera() {
    let (gr3, gr3x) = need!(sessions());
    for (s, camera, version, file) in [(&gr3, "GR III", "2.10", "FWDC239B.BIN"), (&gr3x, "GR IIIx", "1.60", "FWDC243B.BIN")] {
        let fw = s.firmware();
        assert_eq!(fw["camera"], camera);
        assert_eq!(fw["version"], version);
        assert_eq!(fw["update_file"], file);
    }
}

#[test]
fn images_differ_only_in_identity() {
    let (a, b) = (need!(common::update_file()), need!(common::update_file_gr3x()));
    let (a, b) = (decoded(a), decoded(b));
    assert_eq!(a.len(), b.len());
    let diff: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    // name, project number, model ID, version, dates, checksums
    assert!(diff.len() < 128, "{} bytes differ", diff.len());
    let data = addr::file_offset(addr::DATA_VMA)..addr::file_offset(addr::DATA_VMA) + addr::DATA_SIZE as usize;
    assert!(diff.iter().all(|i| !data.contains(i)), "the .data section differs");
}

#[test]
fn same_answers() {
    let (gr3, gr3x) = need!(sessions());
    assert_eq!(look_data(&gr3), look_data(&gr3x));
    let tone = json!({
        "params": {"key": 1, "contrast": -2, "highlight": 3, "shadow": -1},
        "edit": {"points": [[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]], "families": [1]},
        "chan": {"set": [3, 4, 5], "grade": {"Preset 1": {"shadows": {"amount": 0.5, "hue": 200}}}},
    });
    assert_eq!(gr3.tone(&tone).unwrap(), gr3x.tone(&tone).unwrap());
}

#[test]
fn same_patch() {
    let (mut gr3, mut gr3x) = need!(sessions());
    let stock3 = decoded(common::update_file().unwrap());
    let stock3x = decoded(common::update_file_gr3x().unwrap());
    let checksum = stock3.len() - 4;
    for (n, (inherit, req)) in requests().into_iter().enumerate() {
        for s in [&mut gr3, &mut gr3x] {
            s.set_inherit(&json!({"target": "CrossProcess", "source": inherit})).unwrap();
        }
        let (r3, p3, preset3) = gr3.export(&req).unwrap();
        let (r3x, p3x, preset3x) = gr3x.export(&req).unwrap();
        assert_eq!(r3x["update_file"], "FWDC243B.BIN");
        assert_eq!(r3["frames_reencoded"], r3x["frames_reencoded"], "export {n}");
        let mut c = grfw::Container::parse(p3x.clone()).unwrap();
        assert_eq!(c.project, 0x261, "export {n}");
        assert!(c.check(Some(0x261), Some(2)).is_empty(), "export {n}");
        let (e3, e3x) = (decoded(p3), c.decode().unwrap());
        // the GR III patch applied to the IIIx image gives the IIIx export
        for i in 0..checksum {
            let want = if e3[i] != stock3[i] { e3[i] } else { stock3x[i] };
            assert_eq!(e3x[i], want, "export {n}: byte {i:#x}");
        }
        assert_eq!(grfw::word_sum(&e3x), 0, "export {n}: image checksum");
        let strip = |p: String| {
            let mut v: Value = serde_json::from_str(&p).unwrap();
            v["source"] = Value::Null;
            v
        };
        assert_eq!(strip(preset3), strip(preset3x), "export {n}: preset");
    }
}
