//! Firmware patches that enable manual exposure (M) in movie mode,
//! developed and camera-tested on the GR III.
//!
//! The look editor itself only writes look data (matrices, curves, HSL, post
//! matrix, grain, GR-Log). These are ARM instruction patches in the AE
//! engine, found and tested on the GR III 2.10 build (project 0x24c). The
//! GR IIIx 1.60 file is the same build with another model identity, so the
//! code bytes are identical (core/tests/models.rs holds the editor to that).
//! Every patch is applied only after the bytes at its address match the
//! stock encoding it was found with, so a moved build is refused, never
//! patched blindly.

use crate::addr;

/// the dead `AE_Mode` debug-shell handler the stubs overwrite
pub const HOLD_CAVE: u32 = 0x5301_A730;

/// the tested video bundle: `manual mlive7 dials osd bitrate ndm`
pub const MANUAL: &[&str] = &["manual", "mlive7", "dials", "osd", "bitrate", "ndm"];

/// one byte patch: RTOS address, the stock encoding it replaces, and the
/// replacement (both hex, little-endian instruction words)
pub struct Patch {
    pub va: u32,
    pub stock: &'static str,
    pub new: &'static str,
    pub what: &'static str,
}

const NOP: &str = "00f020e3";

/// mlive7: in movie, copy the movie branch's mode (+0x122) to the AE field
/// the per-frame AE reads (+0x8e) when it is M, then run the unchanged AE.
const MLIVE7_STUB: &str = concat!(
    "10c090e5", // ldr   ip, [r0, #0x10]      AE info
    "3a30dce5", // ldrb  r3, [ip, #0x3a]      movie flag
    "000053e3", // cmp   r3, #0
    "0300000a", // beq   orig
    "013c8ce2", // add   r3, ip, #0x100
    "b232d3e1", // ldrh  r3, [r3, #0x22]      mode (movie branch) +0x122
    "050053e3", // cmp   r3, #5               M
    "be38cc01", // strheq r3, [ip, #0x8e]     mode field used by the AE
    "b66111ea", // b     0x53472e30           unchanged per-frame AE
);

/// osd: overlay predicate stub (dead AE_Mode code after the mlive7 stub)
const OSD_STUB: &str = concat!(
    "10402de9", // push  {r4, lr}
    "0040a0e1", // mov   r4, r0
    "86c107eb", // bl    0x5320ad88           ModelFactory
    "eac207eb", // bl    0x5320b31c           UserDataModel
    "b2a508eb", // bl    0x53243e40           GetCommonStillMovie
    "010050e3", // cmp   r0, #1
    "0100a003", // moveq r0, #1
    "1080bd08", // popeq {r4, pc}             movie: visible
    "0400a0e1", // mov   r0, r4
    "1040bde8", // pop   {r4, lr}
    "2f7a05ea", // b     0x5317904c           original predicate
);

/// ndm: in M the AE update 0x53454288 skips fetching the settings (fixed
/// exposure, [ae+0x428b] = 1) and with them the ND setting, so the ND
/// button never reaches the ND state machine 0x5344d090. The stub, called
/// from that branch, does the ND part of the other branch and nothing else
/// (docs/firmware/nd-monitor.md). In movie it takes the ND setting from
/// UserData MovieNdFilter, since in M the AE settings object never gets it
/// (camera run 6). r4 = AE context + 0x5000.
const NDM_STUB: &str = concat!(
    "04e02de5", // push  {lr}
    "2c16c4e5", // strb  r1, [r4, #0x62c]     the instruction the bl replaced
    "5b35d4e5", // ldrb  r3, [r4, #0x55b]
    "000053e3", // cmp   r3, #0
    "1c00001a", // bne   done
    "943694e5", // ldr   r3, [r4, #0x694]     ND requests
    "050013e3", // tst   r3, #5
    "1900001a", // bne   done
    "340f03e3", // movw  r0, #0x3f34          UserData model 0x55063f30, +4: its data
    "060545e3", // movt  r0, #0x5506
    "000090e5", // ldr   r0, [r0]
    "000050e3", // cmp   r0, #0
    "0600000a", // beq   settings
    "4430d0e5", // ldrb  r3, [r0, #0x44]      still/movie: 1 = movie
    "010053e3", // cmp   r3, #1
    "0300001a", // bne   settings
    "3025d0e5", // ldrb  r2, [r0, #0x530]     MovieNdFilter: 1 on, 2 off
    "010052e3", // cmp   r2, #1
    "0020a013", // movne r2, #0
    "030000ea", // b     apply
    "440594e5", // settings: ldr r0, [r4, #0x544]  AE settings object
    "000050e3", // cmp   r0, #0
    "0a00000a", // beq   done
    "3820d0e5", // ldrb  r2, [r0, #0x38]      ND setting (getter 0x5356c688)
    "5825c4e5", // apply: strb r2, [r4, #0x558]
    "9126c4e5", // strb  r2, [r4, #0x691]
    "9036d4e5", // ldrb  r3, [r4, #0x690]     ND state the AE runs with
    "030052e1", // cmp   r2, r3
    "0400000a", // beq   done
    "8436d4e5", // ldrb  r3, [r4, #0x684]     a move still pending
    "000053e3", // cmp   r3, #0
    "0130a003", // moveq r3, #1
    "5a35c405", // strbeq r3, [r4, #0x55a]    apply the ND change
    "5935c405", // strbeq r3, [r4, #0x559]
    "04f09de4", // done: pop {pc}
);

/// ndm, second stub: the other branch of 0x53454288 fetches the ND setting
/// with the getter 0x5356c688 from the AE settings object, which in movie
/// M never gets it; in movie the stub hands on MovieNdFilter instead.
const NDM_GETTER_STUB: &str = concat!(
    "02402de9", // push  {r1, lr}
    "994715eb", // bl    0x5356c688           the getter the bl replaced
    "0240bde8", // pop   {r1, lr}
    "340f03e3", // movw  r0, #0x3f34          UserData model 0x55063f30, +4: its data
    "060545e3", // movt  r0, #0x5506
    "000090e5", // ldr   r0, [r0]
    "000050e3", // cmp   r0, #0
    "1eff2f01", // bxeq  lr
    "4430d0e5", // ldrb  r3, [r0, #0x44]      still/movie: 1 = movie
    "010053e3", // cmp   r3, #1
    "1eff2f11", // bxne  lr
    "3035d0e5", // ldrb  r3, [r0, #0x530]     MovieNdFilter: 1 on, 2 off
    "010053e3", // cmp   r3, #1
    "0030a013", // movne r3, #0
    "0030c1e5", // strb  r3, [r1]             the ND setting the AE update goes on with
    "1eff2fe1", // bx    lr
);

const GROUPS: &[(&str, &[Patch])] = &[
    (
        "manual",
        &[Patch { va: 0x5327_12AC, stock: "1800000a", new: NOP, what: "CreateExposureMode: movie -> P branch" }],
    ),
    (
        "bitrate",
        &[Patch { va: 0x5366_A3E8, stock: "083205e3", new: "10340ae3", what: "bitrate <=30 fps: movw r3,#21000 -> #42000" }],
    ),
    (
        "dials",
        &[
            Patch { va: 0x5329_52CC, stock: "b0ffff1a", new: NOP, what: "dial handler 1: drop movie early return" },
            Patch { va: 0x5329_58B4, stock: "0e00000a", new: "0e0000ea", what: "dial handler 2: skip movie -> comp shortcut" },
            Patch { va: 0x5329_5E84, stock: "dcffff1a", new: NOP, what: "dial handler 3 (ISO): drop movie early return" },
        ],
    ),
    (
        "osd",
        &[
            Patch {
                va: 0x5301_A760,
                stock: "2400000a610053e33200000a740053e33600000a730053e33700000a6d0053e32f00000a650053e33600000a",
                new: OSD_STUB,
                what: "overlay predicate stub",
            },
            Patch { va: 0x5317_BBA8, stock: "27f5ffeb", new: "ec7afaeb", what: "shutter overlay item: bl predicate -> stub" },
            Patch { va: 0x5317_BCC4, stock: "e0f4ffeb", new: "a57afaeb", what: "aperture overlay item: bl predicate -> stub" },
        ],
    ),
    (
        "mlive7",
        &[
            Patch { va: HOLD_CAVE, stock: "0dc0a0e130d82de904b04ce214304be208d04de2040023e50300a0e1ff811aeb000050e3", new: MLIVE7_STUB, what: "movie: mode +0x122 -> +0x8e stub over dead shell handler" },
            Patch { va: 0x5346_C84C, stock: "771900eb", new: "b7b7eeeb", what: "AE event loop: bl 0x53472e30 -> bl stub" },
        ],
    ),
    (
        "ndm",
        &[
            Patch {
                va: 0x5301_A78C,
                stock: "f00804e3c10345e36d761aebd80401e3c20345e36a761aebe40401e3c20345e367761aebf00401e3c20345e364761aebfc0401e3c20345e361761aeb080501e3c20345e35e761aeb140501e3c20345e35b761aeb200501e3c20345e358761aeb0000a0e314d04be230a89de80040a0e30150a0e398ef11eb0c3090e55450c3e595ef11eb080090e50410a0e1",
                new: NDM_STUB,
                what: "ND in M: stub over dead shell handler (after osd)",
            },
            Patch { va: 0x5345_431C, stock: "2c16c4e5", new: "1a19efeb", what: "AE update, fixed-exposure branch: strb -> bl ND stub" },
            Patch {
                va: 0x5301_A818,
                stock: "bcda11eb91ef11eb080090e50410a0e1cfdb11eb0000a0e314d04be230a89de80240a0e30150a0e3eeffffea0540a0e30150a0e3ebffffea0140a0e30450a0e1",
                new: NDM_GETTER_STUB,
                what: "ND in M: getter stub over dead shell handler (after the ND stub)",
            },
            Patch { va: 0x5345_43D0, stock: "ac6004eb", new: "1019efeb", what: "AE update, settings branch: bl ND getter -> bl getter stub" },
        ],
    ),
];

pub fn group_names() -> impl Iterator<Item = &'static str> {
    GROUPS.iter().map(|(n, _)| *n)
}

/// the patch groups a request asks for: `true` is the full tested bundle,
/// `false`/null/absent is none, otherwise a list of patch names.
pub fn names_from(v: Option<&serde_json::Value>) -> Result<Vec<String>, String> {
    let Some(v) = v else { return Ok(vec![]) };
    if v.is_null() {
        return Ok(vec![]);
    }
    if let Some(b) = v.as_bool() {
        return Ok(if b { MANUAL.iter().map(|s| s.to_string()).collect() } else { vec![] });
    }
    let Some(a) = v.as_array() else {
        return Err("video: use true or a list of patch names".into());
    };
    let mut names = vec![];
    for x in a {
        let s = x.as_str().ok_or("video: patch names must be strings")?;
        if !GROUPS.iter().any(|(n, _)| *n == s) {
            return Err(format!("video: unknown patch {s:?}"));
        }
        if !names.iter().any(|n| n == s) {
            names.push(s.to_string());
        }
    }
    Ok(names)
}

/// apply the patches of each named group to the decoded image; returns one
/// line per applied patch for the export report. Fails (without writing) if
/// the bytes at any address differ from the stock encoding the patch was
/// found with.
pub fn apply(image: &mut [u8], names: &[String]) -> Result<Vec<String>, String> {
    let mut applied = vec![];
    for name in names {
        let (_, patches) = GROUPS.iter().find(|(n, _)| n == name).unwrap();
        for p in *patches {
            let stock = hex(p.stock)?;
            let new = hex(p.new)?;
            if stock.len() != new.len() {
                return Err(format!("video {name}: patch at {:#x} changes length", p.va));
            }
            let off = addr::file_offset(p.va);
            if off + stock.len() > image.len() {
                return Err(format!("video {name}: {:#x} outside the image", p.va));
            }
            let have = &image[off..off + stock.len()];
            if have != stock.as_slice() {
                return Err(format!(
                    "video {name}: bytes at {:#x} do not match this build (found {}, expected {})",
                    p.va,
                    hex_str(have),
                    p.stock,
                ));
            }
            image[off..off + stock.len()].copy_from_slice(&new);
            applied.push(format!("{:#x} {}", p.va, p.what));
        }
    }
    Ok(applied)
}

fn hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("odd hex length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn hex_str(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
