//! The RAM patch (rampatch.rs, docs/plan-ram-loader.md): written into the
//! view of the image the camera runs, it gives exactly the exported image.

mod common;
use common::need;

use grlook::grain;
use grlook::grfw::Container;
use grlook::rampatch::{self, Kind};
use grlook::session::Session;
use serde_json::{json, Value};

fn session() -> Option<(Session, Vec<u8>)> {
    let f = common::update_file()?;
    let stock = Container::parse(f.clone()).unwrap().decode().unwrap();
    Some((Session::open(f, "fwdc239b.bin").unwrap(), stock))
}

fn look() -> Value {
    let m = json!([1.1, -0.05, -0.05, -0.05, 1.1, -0.05, -0.05, -0.05, 1.1]);
    json!({"targets": {
        "Vivid": {"source": "NegativeFilm", "matrices": {"Vivid": m}, "grain": 2,
                  "post": [1.05, -0.05, 0.0, 0.0, 1.0, 0.0, 0.0, -0.05, 1.05]},
        "Standard": {"grain": 1},
    }})
}

/// the image the export would flash
fn exported(s: &Session, body: &Value) -> Vec<u8> {
    Container::parse(s.export(body).unwrap().1).unwrap().decode().unwrap()
}

#[test]
fn nothing_to_patch() {
    let (_, stock) = need!(session());
    let v = rampatch::view(&stock).unwrap();
    let p = rampatch::diff(v, v);
    assert!(p.runs.is_empty());
    assert_eq!(p.base_code_crc, p.patched_code_crc);
    assert_eq!(rampatch::decode(&rampatch::encode(&p)).unwrap(), p);
}

#[test]
fn patch_from_stock_gives_the_export() {
    let (s, stock) = need!(session());
    let body = look();
    let (r, pat, _) = s.export_ram(&body, None).unwrap();
    assert_eq!(r["base"], json!("stock"));
    let p = rampatch::decode(&pat).unwrap();
    let img = exported(&s, &body);
    let want = rampatch::view(&img).unwrap();
    assert_eq!(rampatch::apply(rampatch::view(&stock).unwrap(), &p).unwrap(), want);
    assert_eq!(p.patched_code_crc, rampatch::crc32(&want[..(grlook::addr::DATA_LMA - rampatch::RTOS) as usize]));

    // all three kinds, in write order; the grain sites are branches into
    // the free space, written last
    let kinds: Vec<Kind> = p.runs.iter().map(|r| r.kind).collect();
    assert!(kinds.windows(2).all(|w| w[0] <= w[1]), "{kinds:?}");
    for k in [Kind::Free, Kind::Data, Kind::Code] {
        assert!(kinds.contains(&k), "no {k:?} run: {kinds:?}");
    }
    for site in [grain::GATE, grain::FILL] {
        let run = p.runs.iter().find(|r| (r.address..r.address + r.after.len() as u32).contains(&site)).unwrap();
        assert_eq!(run.kind, Kind::Code, "{site:#x}");
    }
    // .data runs sit at 0x55000000.., not at the copy source
    for r in p.runs.iter().filter(|r| r.kind == Kind::Data) {
        assert!(r.address >= grlook::addr::DATA_VMA && r.address < grlook::addr::DATA_VMA + grlook::addr::DATA_SIZE);
    }
    assert_eq!(r["runs"], json!(p.runs.len()));
}

#[test]
fn patch_from_another_build() {
    let (s, stock) = need!(session());
    // a build with another look stands in for what the camera runs
    let other = json!({"targets": {"PositiveFilm": {"grain": 3}, "Vivid": {"source": "Retro"}}});
    let base_file = s.export(&other).unwrap().1;
    let base_img = Container::parse(base_file.clone()).unwrap().decode().unwrap();
    let body = look();
    let (r, pat, _) = s.export_ram(&body, Some(&base_file)).unwrap();
    assert_eq!(r["base"], json!("update file"));
    let p = rampatch::decode(&pat).unwrap();
    let want = exported(&s, &body);
    assert_eq!(rampatch::apply(rampatch::view(&base_img).unwrap(), &p).unwrap(), rampatch::view(&want).unwrap());
    // against stock instead, the before bytes do not match
    assert!(rampatch::apply(rampatch::view(&stock).unwrap(), &p).is_err());
}

#[test]
fn dump_as_base() {
    let (s, _) = need!(session());
    // T1 (docs/firmware/ram-loader.md): the dump and the update file it came
    // from give the same patch
    let dir = common::work().join("ramload");
    let dump = need!(std::fs::read(dir.join("dump-2026-10-21/RTOS.BIN")).ok());
    let file = need!(std::fs::read(dir.join("card-fw/FWDC239B.BIN")).ok());
    let body = look();
    let (r, from_dump, _) = s.export_ram(&body, Some(&dump)).unwrap();
    assert_eq!(r["base"], json!("RAM dump"));
    let (_, from_file, _) = s.export_ram(&body, Some(&file)).unwrap();
    assert_eq!(from_dump, from_file);
}

#[test]
fn damaged_patch() {
    let (s, _) = need!(session());
    let (_, mut pat, _) = s.export_ram(&look(), None).unwrap();
    let n = pat.len();
    pat[n / 2] ^= 1;
    assert!(rampatch::decode(&pat).unwrap_err().contains("CRC"));
    assert!(rampatch::decode(b"not a patch").is_err());
}

#[test]
fn other_build_refused() {
    let (s, mut stock) = need!(session());
    // another build moves .data: the startup literals tell
    let lit = (0x5300_0450 - grlook::addr::BIAS) as usize;
    stock[lit] ^= 0x10;
    assert!(rampatch::view(&stock).unwrap_err().contains("startup literals"));
    assert!(rampatch::view(&[0u8; 1000]).is_err());
    let _ = s;
}
