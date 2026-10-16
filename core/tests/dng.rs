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

/// highlight correction from the makernote (0x0069): the screen-chart test
/// shots of docs/plan-highlight-correction.md, off and on
#[test]
fn highlight_correction_flag() {
    let dir = common::work().join("highlight-test");
    for (stem, on) in [("MR029426", false), ("MR029428", true), ("MR029430", false)] {
        let Ok(data) = std::fs::read(dir.join(format!("{stem}.DNG"))) else {
            eprintln!("{stem}: DNG not found, skipped");
            continue;
        };
        assert_eq!(dng::decode(&data).unwrap().highlight_correction, Some(on), "{stem}");
    }
}

// ---- the filmstrip's preview walk (dng::preview_ranges) --------------------
// Synthetic TIFFs: IFD0 carries a non-JPEG RGB thumbnail (as in GR III DNGs)
// plus SubIFDs carrying the embedded JPEGs; the walk must pick only the JPEG
// IFDs, largest first, and work on a header-only prefix.

fn p16(le: bool, v: u16) -> Vec<u8> {
    if le { v.to_le_bytes().to_vec() } else { v.to_be_bytes().to_vec() }
}
fn p32(le: bool, v: u32) -> Vec<u8> {
    if le { v.to_le_bytes().to_vec() } else { v.to_be_bytes().to_vec() }
}
fn ent(le: bool, tag: u16, typ: u16, count: u32, value: u32) -> Vec<u8> {
    let mut e = p16(le, tag);
    e.extend(p16(le, typ));
    e.extend(p32(le, count));
    // the 4-byte value field: BYTE/SHORT types use the first bytes (as real
    // TIFF writers do), LONG the whole field
    match typ {
        1 | 6 | 7 => {
            e.push(value as u8);
            e.extend([0, 0, 0]);
        }
        3 | 8 => {
            e.extend(p16(le, value as u16));
            e.extend([0, 0]);
        }
        _ => e.extend(p32(le, value)),
    }
    e
}
fn ifd(le: bool, entries: &[Vec<u8>]) -> Vec<u8> {
    let mut b = p16(le, entries.len() as u16);
    for e in entries {
        b.extend(e);
    }
    b.extend(p32(le, 0)); // next IFD: none
    b
}

/// one embedded JPEG: which entry gates it, which offset/length tags, its length
struct SynthJpeg {
    gate: (u16, u32),
    off_tag: u16,
    len_tag: u16,
    len: u32,
}

/// a minimal TIFF: IFD0 (SubIFDs, an RGB thumbnail that must not win) + one
/// SubIFD per JPEG + the data; returns the file and the declared ranges
fn synth(le: bool, jpegs: &[SynthJpeg]) -> (Vec<u8>, Vec<(u32, u32)>) {
    let n = jpegs.len() as u32;
    let ifd0_end = 8 + 2 + 5 * 12 + 4; // after the header, IFD0's five entries
    // the SubIFDs entry is inline for one IFD, an out-of-line offset array
    // for several (as in real DNGs); its value is ifd0_end either way
    let sub_ifds = ifd0_end + if n > 1 { 4 * n } else { 0 };
    let mut at = sub_ifds + (2 + 3 * 12 + 4) * n; // the JPEGs' data
    let thumb_len = 1000u32; // larger than any JPEG below
    let thumb_at = at + jpegs.iter().map(|j| j.len).sum::<u32>();

    let mut b = Vec::new();
    b.extend(if le { &b"II*\0"[..] } else { &b"MM\0*"[..] });
    b.extend(p32(le, 8)); // IFD0 at 8
    b.extend(ifd(le, &[
        ent(le, 0x14A, 4, n, ifd0_end),
        ent(le, 0x106, 3, 1, 2), // IFD0's own RGB thumbnail: not a JPEG
        ent(le, 0x103, 3, 1, 1),
        ent(le, 0x111, 4, 1, thumb_at),
        ent(le, 0x117, 4, 1, thumb_len),
    ]));
    if n > 1 {
        for i in 0..n {
            b.extend(p32(le, sub_ifds + 42 * i));
        }
    }
    assert_eq!(b.len() as u32, sub_ifds);
    let mut ranges = Vec::new();
    for j in jpegs {
        b.extend(ifd(le, &[
            ent(le, j.gate.0, 3, 1, j.gate.1),
            ent(le, j.off_tag, 4, 1, at),
            ent(le, j.len_tag, 4, 1, j.len),
        ]));
        ranges.push((at, j.len));
        at += j.len;
    }
    for (_, l) in &ranges {
        b.extend([0xFF, 0xD8]); // the magic only; content irrelevant here
        b.extend(std::iter::repeat(0u8).take(*l as usize - 2));
    }
    b.extend(std::iter::repeat(0u8).take(thumb_len as usize));
    (b, ranges)
}

#[test]
fn preview_ranges_picks_the_embedded_jpeg() {
    for le in [true, false] {
        // IFD0's larger non-JPEG thumbnail must not win over the JPEG
        let (f, r) = synth(le, &[SynthJpeg { gate: (0x106, 6), off_tag: 0x111, len_tag: 0x117, len: 800 }]);
        let want: Vec<(usize, usize)> = r.iter().map(|&(o, l)| (o as usize, l as usize)).collect();
        assert_eq!(dng::preview_ranges(&f), want, "le = {le}");
        // the filmstrip sends only a header prefix, before any image data
        assert_eq!(dng::preview_ranges(&f[..r[0].0 as usize]), want, "prefix, le = {le}");
        // not a TIFF, or too short: empty
        assert!(dng::preview_ranges(&f[..10]).is_empty());
    }
}

#[test]
fn preview_ranges_gates_and_sorts() {
    // Compression 6 gates too, 0x201/0x202 stand in for the strip tags,
    // and the largest JPEG comes first even when it sits in the later IFD
    let (f, r) = synth(true, &[
        SynthJpeg { gate: (0x103, 6), off_tag: 0x201, len_tag: 0x202, len: 700 },
        SynthJpeg { gate: (0x106, 6), off_tag: 0x111, len_tag: 0x117, len: 900 },
    ]);
    let mut want: Vec<(usize, usize)> = r.iter().map(|&(o, l)| (o as usize, l as usize)).collect();
    want.sort_by(|a, b| b.1.cmp(&a.1));
    assert_eq!(dng::preview_ranges(&f), want);
}

/// the editor's local test photos (editor/static/_local, not in git): the
/// walk on a 1 MiB prefix must agree with decode() on the full file
#[test]
fn preview_ranges_match_decode() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/static/_local");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut seen = 0;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().map_or(true, |x| !x.eq_ignore_ascii_case("dng")) {
            continue;
        }
        let data = std::fs::read(&p).unwrap();
        let Ok(shot) = dng::decode(&data) else { continue };
        let ranges = dng::preview_ranges(&data);
        if let Some((o, l)) = shot.jpeg {
            assert_eq!(ranges.first(), Some(&(o, l - o)), "{}: largest preview", p.display());
            assert_eq!(&data[o..o + 2], &[0xFF, 0xD8], "{}: JPEG magic", p.display());
        } else {
            assert!(ranges.is_empty(), "{}: decode found no JPEG, the walk did", p.display());
        }
        // the filmstrip sends a file prefix (1 MiB, retried at 8 MiB, then
        // the whole file); the prefix walk must agree wherever the header
        // points IFD0 into the prefix (some files put IFD0 at the very end)
        let ifd0 = if data.starts_with(b"II*\0") {
            u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize
        } else {
            u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize
        };
        let cap = data.len().min(1 << 20);
        if ifd0 < cap {
            assert_eq!(dng::preview_ranges(&data[..cap]), ranges, "{}: prefix walk differs", p.display());
        }
        seen += 1;
    }
    assert!(seen > 0, "no DNGs in {}", dir.display());
}
