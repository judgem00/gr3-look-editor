//! The video patches (video.rs), checked against the firmware and, when the
//! camera-tested artifacts are present, byte for byte against them.

mod common;
use common::need;

use grlook::{addr, grfw, video};

/// the ten patches of the `manual mlive7 dials osd bitrate` bundle: address
/// and length in bytes
fn patches() -> Vec<(u32, usize)> {
    vec![
        (0x5301_A730, 36), // mlive7 stub over the dead AE_Mode handler
        (0x5346_C84C, 4),  // AE event loop: bl -> bl stub
        (0x5301_A760, 44), // osd overlay predicate stub
        (0x5317_BBA8, 4),  // shutter overlay item
        (0x5317_BCC4, 4),  // aperture overlay item
        (0x5327_12AC, 4),  // CreateExposureMode: movie -> P branch
        (0x5329_52CC, 4),  // dial handler 1
        (0x5329_58B4, 4),  // dial handler 2
        (0x5329_5E84, 4),  // dial handler 3 (ISO)
        (0x5366_A3E8, 4),  // bitrate <=30 fps
    ]
}

fn decode(update: Vec<u8>) -> Vec<u8> {
    grfw::Container::parse(update).unwrap().decode().unwrap()
}

/// applying the bundle to the stock image changes exactly the 98 bytes at the
/// ten patch addresses (each patch's stock bytes are checked by `apply`, so a
/// moved build is refused rather than silently patched)
#[test]
fn patches_stock_at_exactly_the_known_addresses() {
    let update = need!(common::update_file());
    let stock = decode(update);
    let mut img = stock.clone();
    let names = video::MANUAL.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let applied = video::apply(&mut img, &names).unwrap();
    assert_eq!(applied.len(), patches().len(), "one report line per patch");

    let changed: Vec<usize> = (0..img.len()).filter(|&i| img[i] != stock[i]).collect();
    assert_eq!(changed.len(), 98, "the bundle changes 98 bytes in total");
    let in_patch = |i: usize| {
        let va = i as u32 + addr::BIAS;
        patches().iter().any(|&(a, n)| (a..a + n as u32).contains(&va))
    };
    assert!(changed.iter().all(|&i| in_patch(i)), "no change outside the ten patches");
}

/// the export path: an empty-targets export with `video: true` changes the
/// stock image by exactly the video patches, and the result verifies
#[test]
fn export_applies_the_video_patches() {
    let update = need!(common::update_file());
    let stock = decode(update.clone());
    let s = grlook::session::Session::open(update, "fwdc239b.bin").unwrap();
    let (r, packed, preset) = s.export(&serde_json::json!({"targets": {}, "video": true})).unwrap();
    assert_eq!(r["video"].as_array().map(|a| a.len()), Some(patches().len()));
    // the preset records the bundle so a reload keeps the option on
    let preset: serde_json::Value = serde_json::from_str(&preset).unwrap();
    assert_eq!(preset["video"], serde_json::json!(video::MANUAL));

    let img = decode(packed);
    // everything before the last word (the image checksum, which `pack` refixes)
    let changed: Vec<usize> = (0..img.len() - 4).filter(|&i| img[i] != stock[i]).collect();
    assert_eq!(changed.len(), 98, "only the video patches change the image");
}

/// against the camera-tested firmware: `work/grlog-test/video/unpackedFW-video`
/// is `unpackedFW-looks` (the GR-Log export) plus exactly these patches and a
/// fixed checksum. Applying the patches to the looks image must reproduce the
/// flashed image byte for byte.
#[test]
fn reproduces_the_camera_tested_image() {
    let work = common::work().join("grlog-test");
    let looks = match std::fs::read(work.join("unpackedFW-looks")) {
        Ok(b) => b,
        Err(_) => {
            eprintln!("skipped: work/grlog-test/unpackedFW-looks missing");
            return;
        }
    };
    let flashed = match std::fs::read(work.join("video/unpackedFW-video")) {
        Ok(b) => b,
        Err(_) => {
            eprintln!("skipped: work/grlog-test/video/unpackedFW-video missing");
            return;
        }
    };
    assert_eq!(looks.len(), flashed.len());
    let names = video::MANUAL.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut img = looks;
    video::apply(&mut img, &names).unwrap();
    grfw::fix_image_checksum(&mut img);
    assert_eq!(img, flashed, "the editor's video patches reproduce the flashed image");
}
