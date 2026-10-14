//! Channel curves for every colour look: the selector stub, its table, the
//! shared free space and HDR without its effects (docs/plan-look-curves.md).

mod common;
use common::need;

use grlook::addr::{curve_at, file_offset, word};
use grlook::arm::{Cpu, Memory};
use grlook::grfw::Container;
use grlook::session::{Session, CURVE_LOOKS};
use grlook::{chan, curvesel, looks, multiaxial, space};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;

fn ramp(scale: f64, offset: f64) -> Vec<f64> {
    (0..256).map(|k| (64.0 * k as f64 * scale + offset).clamp(0.0, 16383.0).round()).collect()
}

/// a different marker per look so mix-ups show
fn marker(n: usize) -> Value {
    let lin = ramp(1.0, 0.0);
    let mut rgb = vec![lin.clone(), lin.clone(), lin];
    rgb[n % 3] = ramp(1.1 + 0.05 * n as f64, 0.0);
    rgb[(n + 1) % 3] = ramp(1.0, 100.0 * (n as f64 + 1.0));
    json!(rgb)
}

fn all_looks() -> Value {
    Value::Object(CURVE_LOOKS.iter().enumerate().map(|(n, l)| (l.to_string(), marker(n))).collect())
}

fn decode(packed: Vec<u8>) -> Vec<u8> {
    Container::parse(packed).unwrap().decode().unwrap()
}

fn sessions() -> Vec<(Session, Vec<u8>)> {
    let mut out = vec![];
    for (file, name) in [(common::update_file(), "fwdc239b.bin"), (common::update_file_gr3x(), "fwdc243b.bin")] {
        if let Some(f) = file {
            let stock = Container::parse(f.clone()).unwrap().decode().unwrap();
            out.push((Session::open(f, name).unwrap(), stock));
        }
    }
    out
}

/// the image as the camera maps it, writes into a sparse overlay
struct Ram<'a> {
    img: &'a [u8],
    over: RefCell<BTreeMap<u32, u8>>,
}

impl Ram<'_> {
    fn byte(&self, a: u32) -> Result<u8, String> {
        if let Some(b) = self.over.borrow().get(&a) {
            return Ok(*b);
        }
        let in_image = (0x5300_0000..0x5410_0000).contains(&a) || (0x5500_0000..0x5503_a080).contains(&a);
        if in_image { Ok(self.img[file_offset(a)]) } else { Err(format!("unmapped read {a:#x}")) }
    }
    fn set(&self, a: u32, v: &[u8]) {
        let mut o = self.over.borrow_mut();
        for (k, b) in v.iter().enumerate() {
            o.insert(a + k as u32, *b);
        }
    }
}

impl Memory for Ram<'_> {
    fn read32(&self, a: u32) -> Result<u32, String> {
        Ok(u32::from_le_bytes([self.byte(a)?, self.byte(a + 1)?, self.byte(a + 2)?, self.byte(a + 3)?]))
    }
    fn read16(&self, a: u32) -> Result<u16, String> {
        Ok(u16::from_le_bytes([self.byte(a)?, self.byte(a + 1)?]))
    }
    fn read8(&self, a: u32) -> Result<u8, String> {
        self.byte(a)
    }
    fn write32(&mut self, a: u32, v: u32) -> Result<(), String> {
        self.set(a, &v.to_le_bytes());
        Ok(())
    }
    fn write16(&mut self, a: u32, v: u16) -> Result<(), String> {
        self.set(a, &v.to_le_bytes());
        Ok(())
    }
    fn write8(&mut self, a: u32, v: u8) -> Result<(), String> {
        self.set(a, &[v]);
        Ok(())
    }
}

const LIBRARY: u32 = 0x5503_7604;
const SI: u32 = 0x6100_0000;

/// run the exported image from 0x536dd444 (cmp r1, #21) to the context fill's
/// return; (variant, library pointers 9..11)
fn run_site(img: &[u8], look: u32, tone: u8) -> (u8, [u32; 3]) {
    let mut m = Ram { img, over: RefCell::new(BTreeMap::new()) };
    let (stage, info8, sp, ret) = (0x6200_0000u32, 0x6300_8000u32, 0x6400_0000u32, 0x6500_0000u32);
    m.write32(info8 + 0x2d0, SI).unwrap();
    m.write8(SI + 0x38a98, 0x77).unwrap();
    m.write8(stage + 0x797, tone).unwrap();
    for k in 0..10 {
        m.write32(sp + 4 * k, 0).unwrap();
    }
    m.write32(sp + 40, ret).unwrap();
    let mut cpu = Cpu::default();
    cpu.r[1] = look;
    cpu.r[3] = 0;
    cpu.r[5] = 0x6600_0000;
    cpu.r[6] = info8;
    cpu.r[7] = stage;
    cpu.r[13] = sp;
    cpu.r[15] = 0x536d_d444;
    for _ in 0..200 {
        if cpu.r[15] == ret {
            let lib = [9, 10, 11].map(|k| m.read32(LIBRARY + 4 * k).unwrap());
            return (m.read8(SI + 0x38a98).unwrap(), lib);
        }
        cpu.step(&mut m).unwrap();
    }
    panic!("look {look}: the context fill does not return");
}

#[test]
fn without_curves_the_code_stays_stock() {
    for (s, stock) in sessions() {
        let hsl = json!({"Vivid": {"bands": [{"center": 2400, "width": 700, "sat": -30}]}});
        for body in [json!({}), json!({"hsl": hsl}), json!({"look_curves": {}})] {
            let img = decode(s.export(&body).unwrap().1);
            assert!(!curvesel::is_installed(&img));
            assert_eq!(word(&img, curvesel::SITE), word(&stock, curvesel::SITE));
            let (lo, hi) = space::RESERVED;
            assert!(space::is_zero(&img, lo, hi), "reserved block written");
            for (site, w, _) in curvesel::HDR_OFF {
                assert_eq!(word(&img, site), w);
            }
        }
        // stock firmware itself: the stub would select exactly what stock selects
        for look in 11..=22 {
            for tone in 0..4 {
                let lib = chan::addresses(&stock);
                let want = match (look, tone) {
                    (21, 0..=2) => (tone + 2, [lib[9], lib[10], lib[11]]),
                    _ => (0, [lib[9], lib[10], lib[11]]),
                };
                assert_eq!(run_site(&stock, look, tone), want, "stock look {look} tone {tone}");
            }
        }
    }
}

#[test]
fn every_look_gets_its_own_curves() {
    for (s, stock) in sessions() {
        let lc = all_looks();
        let (r, packed, preset) = s.export(&json!({"look_curves": lc})).unwrap();
        let img = decode(packed);
        assert!(curvesel::is_installed(&img));
        let code: Vec<u32> = (0..curvesel::code().len() as u32).map(|k| word(&img, curvesel::STUB + 4 * k)).collect();
        assert_eq!(code, curvesel::code());
        let lib = chan::addresses(&stock);
        // the .data library table itself is never changed
        assert_eq!(chan::addresses(&img), lib);
        let mut seen = vec![];
        for (n, name) in CURVE_LOOKS.iter().enumerate() {
            let look = looks::LOOKS.iter().find(|l| l.2 == *name).unwrap().0;
            let (variant, ptrs) = run_site(&img, look, 0);
            assert_eq!(variant, 4, "{name}");
            let want: Vec<Vec<u16>> = lc[name].as_array().unwrap().iter()
                .map(|c| c.as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as u16).collect()).collect();
            for (c, &p) in ptrs.iter().enumerate() {
                assert_eq!(curve_at(&img, p), want[c], "{name} channel {c} (marker {n})");
                assert!(!seen.contains(&p), "{name}: curve storage shared");
                seen.push(p);
            }
        }
        // Cross Process Color Tone 1..3 through the table, at its stock storage
        for tone in 0..3u8 {
            let t = tone as usize;
            assert_eq!(run_site(&img, 21, tone), (4, [lib[3 * t + 3], lib[3 * t + 4], lib[3 * t + 5]]));
        }
        assert_eq!(run_site(&img, 21, 3).0, 0);
        // monochrome looks keep variant 0
        for look in 13..=16 {
            assert_eq!(run_site(&img, look, 0).0, 0, "look {look}");
        }
        assert_eq!(r["space"]["rgb"], json!(7));
        assert_eq!(r["space"]["fits"], json!(true));
        let preset: Value = serde_json::from_str(&preset).unwrap();
        let nums = |v: &Value| -> Vec<Vec<f64>> { serde_json::from_value(v.clone()).unwrap() };
        for name in CURVE_LOOKS {
            assert_eq!(nums(&preset["look_curves"][name]), nums(&lc[name]), "preset {name}");
        }
        if let Ok(path) = std::env::var("LC_DUMP") {
            if s.build.name == "fwdc239b" {
                std::fs::write(&path, &img).unwrap();
                std::fs::write(format!("{path}.json"), preset.to_string()).unwrap();
            }
        }
    }
}

#[test]
fn hsl_and_curves_share_the_pool() {
    let (s, stock) = need!(sessions().into_iter().next());
    // never overlapping, never outside the pool
    for hsl in 0..=5 {
        for curves in 0..=21 {
            let Ok(p) = space::plan(&stock, hsl, curves) else { continue };
            let mut runs: Vec<(u32, u32)> = p.hsl.iter().map(|&a| (a, a + space::HSL_SIZE)).collect();
            runs.extend(p.curves.iter().map(|&a| (a, a + space::CURVE_SIZE)));
            runs.sort();
            for w in runs.windows(2) {
                assert!(w[0].1 <= w[1].0, "{hsl} HSL + {curves} curves overlap: {w:x?}");
            }
            let spare = space::library_spare(&stock);
            for &(lo, hi) in &runs {
                let in_pool = space::PADDING.iter().any(|&(a, b)| lo >= a && hi <= b) || spare.contains(&lo);
                assert!(in_pool, "{lo:#x}..{hi:#x} outside the pool");
                assert!(hi <= space::RESERVED.0 || lo >= space::RESERVED.1);
            }
        }
    }
    // the numbers from the plan: all 7 looks without HSL, fewer with it
    assert!(space::plan(&stock, 0, 21).is_ok());
    assert!(space::plan(&stock, 5, 3).is_ok());
    assert!(space::plan(&stock, 5, 6).is_err());
    let rep = space::report(&stock, 2, 3);
    assert_eq!(rep["fits"], json!(true));

    // export: HSL + curves together, and refused when full
    let band = json!({"bands": [{"center": 2400, "width": 700, "sat": -30}]});
    let hsl: serde_json::Map<String, Value> = ["Standard", "Vivid", "PositiveFilm", "Retro", "NegativeFilm"]
        .iter().map(|l| (l.to_string(), band.clone())).collect();
    let lc = all_looks();
    let err = s.export(&json!({"hsl": hsl, "look_curves": lc})).err().unwrap();
    assert!(err.contains("not enough free space"), "{err}");
    let one = json!({"Vivid": lc["Vivid"]});
    let img = decode(s.export(&json!({"hsl": hsl, "look_curves": one})).unwrap().1);
    let (_, ptrs) = run_site(&img, 12, 0);
    assert_eq!(ptrs.to_vec(), space::library_spare(&stock));
    for (look, table) in [("Standard", 11), ("Retro", 19)] {
        assert_ne!(multiaxial::param_set(&img, table), multiaxial::param_set(&stock, table), "{look}");
    }
    // the meter
    let m = s.space(&json!({"hsl": hsl, "look_curves": one})).unwrap();
    assert_eq!((m["fits"].clone(), m["more_rgb"].clone(), m["more_hsl"].clone()), (json!(true), json!(0), json!(0)));
    let m = s.space(&json!({})).unwrap();
    assert_eq!((m["more_rgb"].clone(), m["more_hsl"].clone()), (json!(7), json!(5)));
}

#[test]
fn refuses_what_it_cannot_do() {
    let (s, _) = need!(sessions().into_iter().next());
    let lc = all_looks();
    for (body, msg) in [
        (json!({"look_curves": {"CrossProcess": lc["Vivid"]}}), "Color Tone"),
        (json!({"look_curves": {"MonotoneGR": lc["Vivid"]}}), "no channel curves"),
        (json!({"look_curves": {"Vivid": [[0, 1]]}}), "three curves"),
        (json!({"look_curves": {"Vivid": lc["Vivid"]}, "channels": {"1": ramp(1.0, 0.0)}}), "Random"),
    ] {
        let err = s.export(&body).err().unwrap();
        assert!(err.contains(msg), "{err}");
    }
}

#[test]
fn hdr_effects_off() {
    for (mut s, stock) in sessions() {
        assert!(s.set_effects(&json!({"effects_off": ["Retro"]})).is_err());
        let fw = s.set_effects(&json!({"effects_off": ["HdrCuisine"]})).unwrap();
        let addr = |fw: &Value, n: &str| fw["looks"].as_array().unwrap().iter().find(|l| l["name"] == n).unwrap()["address"].clone();
        // HDR has a matrix of its own now, with Standard's values
        assert_ne!(addr(&fw, "HdrCuisine"), addr(&fw, "Standard"));
        let m = |n: &str| fw["looks"].as_array().unwrap().iter().find(|l| l["name"] == n).unwrap()["matrix"].clone();
        assert_eq!(m("HdrCuisine"), m("Standard"));
        let lc = json!({"HdrCuisine": all_looks()["HdrCuisine"]});
        let (_, packed, preset) = s.export(&json!({"look_curves": lc, "matrices": {"HdrCuisine": [1, 0, 0, 0, 1, 0, 0, 0, 1]}})).unwrap();
        let img = decode(packed);
        for (site, _, w) in curvesel::HDR_OFF {
            assert_eq!(word(&img, site), w, "{site:#x}");
        }
        assert_eq!(run_site(&img, 20, 0).0, 4);
        // Standard's matrix is untouched
        let std_m = looks::look_matrix_address(&img, 11);
        assert_eq!(&img[file_offset(std_m)..file_offset(std_m) + 18], &stock[file_offset(std_m)..file_offset(std_m) + 18]);
        let preset: Value = serde_json::from_str(&preset).unwrap();
        assert_eq!(preset["effects_off"], json!(["HdrCuisine"]));
        // and back
        let fw = s.set_effects(&json!({"effects_off": []})).unwrap();
        assert_eq!(addr(&fw, "HdrCuisine"), addr(&fw, "Standard"));
        let img = decode(s.export(&json!({})).unwrap().1);
        for (site, w, _) in curvesel::HDR_OFF {
            assert_eq!(word(&img, site), w);
        }
    }
}

#[test]
fn preview_takes_any_looks_curves() {
    let (s, _) = need!(sessions().into_iter().next());
    let lc = all_looks();
    let out = s.tone(&json!({"chan": {"curves": lc["Retro"]}})).unwrap();
    let lut: Vec<u16> = serde_json::from_value(out["lut"].clone()).unwrap();
    for c in 0..3 {
        let curve: Vec<i64> = lc["Retro"][c].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as i64).collect();
        assert_eq!(out["rgb"][c], json!(chan::compose(&lut, &curve)));
        assert_eq!(out["rgb_ref"][c], out["lut_ref"]);
    }
}


#[test]
fn editor_channels_are_look_curves() {
    let (s, _) = need!(sessions().into_iter().next());
    let fw = s.firmware();
    let lc = all_looks();
    let sets = &fw["look_channel_sets"];
    assert_eq!(sets["Standard"], json!([12, 13, 14]));
    assert_eq!(sets["NegativeFilm"], json!([30, 31, 32]));
    // the editor sends changed channels by index; untouched channels of a look stay linear
    let mut channels = serde_json::Map::new();
    for (k, c) in lc["Standard"].as_array().unwrap().iter().enumerate() {
        channels.insert((12 + k).to_string(), c.clone());
    }
    channels.insert("15".into(), lc["Vivid"][0].clone());
    let lin: Vec<f64> = chan::linear().iter().map(|&v| v as f64).collect();
    let direct = json!({"Standard": lc["Standard"], "Vivid": [lc["Vivid"][0], lin, lin]});
    let (a, _, pa) = s.export(&json!({"channels": channels})).unwrap();
    let (b, _, _) = s.export(&json!({"look_curves": direct})).unwrap();
    assert_eq!(a["update_sha256"], b["update_sha256"]);
    let pa: Value = serde_json::from_value(serde_json::from_str(&pa).unwrap()).unwrap();
    assert_eq!(pa["look_curves"].as_object().unwrap().len(), 2);
    assert!(s.export(&json!({"channels": channels, "look_curves": {"Vivid": lc["Vivid"]}})).is_err());
    assert!(s.export(&json!({"channels": {"33": lin}})).is_err());

    // the preview: edits and wheels on a look's set, no curves in the reference
    let out = s.tone(&json!({"chan": {"set": [12, 13, 14],
        "edits": {"12": {"base": lc["Standard"][0], "points": [[0, 0], [1, 1]]}},
        "grade": {"Standard": {"global": {"hue": 0, "amount": 0.5, "lum": 0}}}}})).unwrap();
    let lut: Vec<u16> = serde_json::from_value(out["lut"].clone()).unwrap();
    let eff: Vec<Vec<i64>> = serde_json::from_value(out["channel_curves"].clone()).unwrap();
    assert_eq!(eff.len(), 33);
    assert_ne!(eff[13], chan::linear().iter().map(|&v| v as i64).collect::<Vec<_>>(), "wheel applied");
    for k in 0..3 {
        assert_eq!(out["rgb"][k], json!(chan::compose(&lut, &eff[12 + k])));
        assert_eq!(out["rgb_ref"][k], out["lut_ref"]);
    }
    // the meter counts looks by name
    let m = s.space(&json!({"looks": ["Standard", "Vivid"]})).unwrap();
    assert_eq!(m["rgb"], json!(2));
    assert!(s.space(&json!({"looks": ["CrossProcess"]})).is_err());
}
