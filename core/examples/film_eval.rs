//! Camera test of a fitted film (docs/plan-spektrafilm-fitter.md, phase 6):
//! what each shot should look like, rendered from its own DNG, for
//! tools/eval-film.py to compare with the camera JPEG.
//!
//!     film_eval size DNG ...
//!         preview size and orientation of each DNG: "file width height flip"
//!     film_eval render OUT PRESET LUT STD.DNG=STD.rgba [SHOT.DNG=SHOT.rgba ...]
//!
//! `render` fits input scale, tone gain and lens shading on the Standard shot
//! against its camera JPEG (as the editor does; the .rgba files are the
//! embedded JPEGs at the DNG's preview size, not oriented). Then, for every
//! shot, from its own DNG with its own white balance and those three values:
//!
//!     NAME.model.f32   the shot's look as the core models it (Standard, or
//!                      a target of PRESET), sRGB-encoded, oriented
//!     NAME.lut.f32     LUT on the shot's scene colours (exposure mapped from
//!                      the shot's tone curve onto the fitter's, as the
//!                      editor's "Compare LUT")
//!     NAME.camera.u8   the camera JPEG, oriented
//!     shots.json       per shot: look, size, white balance; the Standard
//!                      shot's fit (residual, highlight chroma) for the fine match
//!
//! The update file comes from GR_UPDATE or work/fwdc239b.bin.

use std::path::{Path, PathBuf};

use grlook::{dng, fit, looks};
use grlook::gamma::Engine;
use grlook::lutfit::{self, Camera, Fit, Input};
use grlook::session::Session;
use serde_json::{json, Value};

fn write_f32(path: &Path, v: &[f32]) {
    let b: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    std::fs::write(path, b).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let work = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../work");
    match args.first().map(String::as_str) {
        Some("size") => {
            for f in &args[1..] {
                let s = dng::decode(&std::fs::read(f).unwrap()).unwrap();
                println!("{f} {} {} {}", s.width, s.height, s.flip);
            }
        }
        Some("render") if args.len() >= 5 => render(&args[1], &args[2], &args[3], &args[4..], &work),
        _ => {
            eprintln!("usage: film_eval size DNG ... | render OUT PRESET LUT STD.DNG=STD.rgba [SHOT.DNG=SHOT.rgba ...]");
            std::process::exit(2);
        }
    }
}

fn render(out: &str, preset: &str, lut_path: &str, shots: &[String], work: &Path) {
    let update = std::env::var("GR_UPDATE").map(PathBuf::from).unwrap_or_else(|_| work.join("fwdc239b.bin"));
    let s = Session::open(std::fs::read(&update).unwrap(), "fwdc239b.bin").unwrap();
    let img = s.image();
    let preset: Value = serde_json::from_str(&std::fs::read_to_string(preset).unwrap()).unwrap();
    let lut = lutfit::parse_cube(&std::fs::read_to_string(lut_path).unwrap()).unwrap();
    let input = lut.input.unwrap_or(Input::GrLog { lo: -8.0, span: 12.0 });
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap();
    let sensor = looks::sensor_ccm(img);
    let ident = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

    let load = |pair: &str| {
        let (d, r) = pair.split_once('=').expect("DNG=RGBA");
        let shot = dng::decode(&std::fs::read(d).unwrap()).unwrap();
        let rgba = std::fs::read(r).unwrap();
        assert_eq!(rgba.len(), shot.width * shot.height * 4, "{r}: not the DNG's preview size");
        let name = Path::new(d).file_stem().unwrap().to_string_lossy().to_string();
        (name, shot, rgba)
    };

    // the Standard shot: scale, gain, lens shading, fine match
    let (std_name, std_shot, std_rgba) = load(&shots[0]);
    let prep = s.prepare_shot_with(&std_shot, Some(&std_rgba), &std_name, true).unwrap();
    let meta = prep.meta;
    assert_eq!(meta["shot_look"], "Standard", "the first shot must be taken with Standard");
    let gain = meta["gain"].as_i64().unwrap() as i32;
    let scale = meta["scale"].as_f64().unwrap();
    let vig: [f64; 3] = serde_json::from_value(meta["vignette"]["coef"].clone()).unwrap();
    let base = Camera::new(img, "Standard").unwrap();
    let (fit_tone, fit_scale) = (base.tone.clone(), base.scale);
    eprintln!("Standard shot {std_name}: gain {gain}, scale {scale:.1}, lens shading {vig:?}");

    // the shot's tone index -> the fitter's, where both curves give the same value
    let tone_of = |params: &[(&str, i32)]| Engine::new(img, &[]).curve(img, fit::tone_params(params, gain)).unwrap();
    let remap = |shot_tone: &[u16]| -> Vec<f64> {
        let mut j = 0usize;
        (0..256).map(|k| {
            let y = shot_tone[k] as f64;
            while j < 254 && (fit_tone[j + 1] as f64) < y {
                j += 1;
            }
            let (a, b) = (fit_tone[j] as f64, fit_tone[j + 1] as f64);
            if y <= a { j as f64 } else if y >= b { (j + 1) as f64 } else { j as f64 + (y - a) / (b - a) }
        }).collect()
    };

    let mut list = vec![];
    for pair in shots {
        let (name, shot, rgba) = load(pair);
        let look = shot.image_tone.and_then(looks::image_tone_look).map(|t| t.1).unwrap_or("?");
        let (w, h) = (shot.width, shot.height);
        // scene colours: white balance (clipped as the camera does), sensor CCM, lens shading
        let lin = fit::render_linear(&shot.linear, shot.wb, &sensor, &ident, false);
        let lin = fit::apply_vignette(&lin, w, h, &vig);
        let px: Vec<[f64; 3]> = lin.chunks_exact(3).map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]).collect();
        let tone = tone_of(&shot.params);

        let spec = if look == "Standard" { Some(Value::Null) } else { preset["targets"].get(look).cloned() };
        let model = spec.map(|spec| {
            let mut cam = Camera::new(img, if look == "Standard" { "Standard" } else { look }).unwrap();
            cam.tone = tone.clone();
            cam.scale = scale;
            let f = Fit::from_spec(look, &spec, cam.standard).unwrap();
            f.renderer(&cam).unwrap().render(&px)
        });

        let r = remap(&tone);
        let lut_px: Vec<[f64; 3]> = px.iter().map(|l| {
            let t = (0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2]) * scale / 16.0;
            let ratio = if t > 1e-4 {
                let j = if t < 255.0 {
                    let k = t as usize;
                    r[k] + (r[k + 1] - r[k]) * (t - k as f64)
                } else {
                    t * r[255] / 255.0
                };
                j / t
            } else {
                r[1]
            };
            let lf = l.map(|v| v * ratio * scale / fit_scale);
            let u = match input {
                Input::GrLog { lo, span } => lf.map(|v| ((v.max(1e-12) / 0.18).log2() - lo) / span),
                Input::Linear => lf,
                Input::Display => panic!("display-referred LUTs are not evaluated here"),
            };
            lut.lookup(u).map(|v| v.clamp(0.0, 1.0))
        }).collect();

        let flat = |v: &[[f64; 3]]| -> Vec<f32> { v.iter().flat_map(|p| p.map(|x| x as f32)).collect() };
        let (cam_o, ow, oh) = dng::orient(&rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect::<Vec<u8>>(), w, h, 3, shot.flip);
        std::fs::write(out.join(format!("{name}.camera.u8")), cam_o).unwrap();
        write_f32(&out.join(format!("{name}.lut.f32")), &dng::orient(&flat(&lut_px), w, h, 3, shot.flip).0);
        if let Some(m) = &model {
            write_f32(&out.join(format!("{name}.model.f32")), &dng::orient(&flat(m), w, h, 3, shot.flip).0);
        }
        eprintln!("{name}: {look}{} wb {:?}", if model.is_some() { "" } else { " (not modelled)" }, shot.wb);
        list.push(json!({ "name": name, "look": look, "width": ow, "height": oh, "wb": shot.wb, "model": model.is_some() }));
    }
    let info = json!({ "standard": std_name, "gain": gain, "scale": scale, "vignette": vig,
                       "residual": meta["residual"], "highlight_chroma": meta["highlight_chroma"], "shots": list });
    std::fs::write(out.join("shots.json"), serde_json::to_string_pretty(&info).unwrap()).unwrap();
}
