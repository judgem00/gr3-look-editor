//! The editor's requests, without a server: one Session holds the update
//! file the user dropped in, the decoded image and the current look data.
//! Requests and answers are JSON, routed by editor/static/bridge.js.

use crate::{chan, gamma, grfw, looks, tone};
use crate::gamma::{Engine, ToneParams};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// Every address the editor reads and patches was found in one build: GR III
/// 2.10. GR IIIx 1.60 is the same build, compiled two minutes later; its image
/// differs only in the model's identity (name, project number, model ID,
/// version, checksums), and the editor reads and patches none of those bytes
/// except the image checksum, which it recomputes (core/tests/models.rs).
/// Other versions move code and data, so they are refused, not patched blindly.
pub struct Build {
    /// sha256 of the update file as Ricoh distributes it
    pub update_sha256: &'static str,
    pub name: &'static str,
    /// sha256 of the decoded image
    pub image_sha256: &'static str,
    pub camera: &'static str,
    pub version: &'static str,
}

pub const SUPPORTED: [Build; 2] = [
    Build {
        update_sha256: "d6a3c9c288d784080e7b283222ecdd8752adfeed6faa36dab44f73115354bfd4",
        name: "fwdc239b",
        image_sha256: "53da690797f35ad6dc2c429d8ca1d0de8e333c211ed959816813982cd093b703",
        camera: "GR III",
        version: "2.10",
    },
    Build {
        update_sha256: "708e79f3b7b96a7e1581b4acf4c412233050a1832718ef7f06c8397db1b431c7",
        name: "fwdc243b",
        image_sha256: "c4bfc09d1148deec2155b21bd3d63be8085cf835cbe5e5cde4832747f519e630",
        camera: "GR IIIx",
        version: "1.60",
    },
];

pub fn sha256_hex(d: &[u8]) -> String {
    Sha256::digest(d).iter().map(|b| format!("{b:02x}")).collect()
}

pub struct Session {
    container: grfw::Container,
    /// the decoded stock image, needed to repack
    stock: Vec<u8>,
    /// stock + linear channel curves + inheritance + private Cross Process set
    fw: Vec<u8>,
    inherit: Vec<(String, String)>,
    pub build: &'static Build,
    /// the dropped file's name, and the name the camera expects (FWDC239B.BIN)
    pub filename: String,
    pub update_name: String,
    base_curves: Vec<Vec<u16>>,
}

fn points(v: &Value) -> Vec<[f64; 2]> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?]))
                .collect()
        })
        .filter(|p: &Vec<[f64; 2]>| !p.is_empty())
        .unwrap_or_else(|| vec![[0.0, 0.0], [1.0, 1.0]])
}

fn families(v: &Value) -> Vec<usize> {
    v.as_array()
        .map(|a| a.iter().filter_map(|f| f.as_u64().map(|x| x as usize)).collect::<Vec<_>>())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| vec![0, 1, 2])
}

fn ints(v: &[i64]) -> Value {
    Value::from(v.to_vec())
}

impl Session {
    /// check and decode the update file; refuses unknown builds
    pub fn open(update: Vec<u8>, filename: &str) -> Result<Session, String> {
        let hash = sha256_hex(&update);
        let Some(build) = SUPPORTED.iter().find(|b| b.update_sha256 == hash) else {
            return Err(format!(
                "unknown firmware file. Only these builds are supported: {}",
                SUPPORTED.iter().map(|b| format!("{} {} ({})", b.camera, b.version, b.name)).collect::<Vec<_>>().join(", ")
            ));
        };
        let mut container = grfw::Container::parse(update)?;
        let problems = container.check(None, None);
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        let stock = container.decode()?;
        if sha256_hex(&stock) != build.image_sha256 {
            return Err("decoded image does not match the known build".into());
        }
        // self-test: the untouched image must pack back to the very same file
        let (again, redo) = grfw::repack(&container, &stock, &stock)?;
        if redo != 0 || again != container.data {
            return Err("self-test failed: repacking the original does not give the original".into());
        }
        let filename = filename.rsplit(['/', '\\']).next().unwrap_or(filename).to_string();
        let name = filename.to_uppercase();
        let base_curves = gamma::base_curves(&stock);
        let mut s = Session {
            container, fw: vec![], stock, inherit: vec![], build, filename,
            update_name: if name.is_empty() { format!("{}.BIN", build.name.to_uppercase()) } else { name },
            base_curves,
        };
        s.rebuild()?;
        Ok(s)
    }

    /// fw from the stock image: Cross Process is the user's look -- its channel
    /// curves start linear and it gets a colour matrix of its own
    fn rebuild(&mut self) -> Result<(), String> {
        let mut img = self.stock.clone();
        let lin: Vec<f64> = chan::linear().iter().map(|&v| v as f64).collect();
        chan::write(&mut img, &(0..chan::COUNT).map(|i| (i, lin.clone())).collect::<Vec<_>>())?;
        for (target, source) in &self.inherit {
            looks::inherit(&mut img, looks::table_of(target).unwrap(), looks::table_of(source).unwrap());
        }
        looks::private_cross(&mut img)?;
        self.fw = img;
        Ok(())
    }

    pub fn image(&self) -> &[u8] {
        &self.fw
    }

    fn inherit_json(&self) -> Value {
        Value::Object(self.inherit.iter().map(|(t, s)| (t.clone(), json!(s))).collect())
    }

    /// GET /api/firmware
    pub fn firmware(&self) -> Value {
        let img = &self.fw;
        let sets: Map<String, Value> = chan::SETS.iter().map(|(n, i)| (n.to_string(), json!(i))).collect();
        json!({
            "path": format!("{}/unpackedFW", self.build.name),
            "camera": self.build.camera,
            "version": self.build.version,
            "update_file": self.update_name,
            "looks": looks::looks(img),
            "sensor_ccm": looks::sensor_ccm(img),
            "luma": looks::luma_weights(img),
            "ycc": looks::ycc_base(img),
            "container": self.filename,
            "channel_curves": chan::read(img),
            "channel_sets": sets,
            "channel_linear": chan::linear(),
            "inherit": self.inherit_json(),
        })
    }

    /// POST /api/inherit {target, source}; source null undoes it
    pub fn set_inherit(&mut self, body: &Value) -> Result<Value, String> {
        let target = body.get("target").and_then(|v| v.as_str()).unwrap_or("");
        let source = body.get("source").and_then(|v| v.as_str());
        let known = |n: &str| looks::table_of(n).is_some();
        if !known(target) || source.is_some_and(|s| !known(s) || s == target) {
            return Err("unknown look".into());
        }
        let mut inherit = self.inherit.clone();
        match source {
            Some(s) => match inherit.iter_mut().find(|(t, _)| t == target) {
                Some(e) => e.1 = s.to_string(),
                None => inherit.push((target.to_string(), s.to_string())),
            },
            None => inherit.retain(|(t, _)| t != target),
        }
        self.inherit = inherit;
        self.rebuild()?;
        Ok(self.firmware())
    }

    fn engine_for(&self, pts: &[[f64; 2]], fams: &[usize]) -> Engine {
        let edit: Vec<(usize, Vec<u16>)> = tone::apply(&self.base_curves, pts, fams)
            .into_iter()
            .map(|(i, v)| (i, v.into_iter().map(|x| x.clamp(0, 0xFFFF) as u16).collect()))
            .collect();
        Engine::new(&self.fw, &edit)
    }

    /// the tone curve for settings and gain (shot fitting uses it too)
    pub fn tone_curve(&self, p: ToneParams) -> Result<Vec<u16>, String> {
        Engine::new(&self.fw, &[]).curve(&self.fw, p)
    }

    /// POST /api/tone
    pub fn tone(&self, body: &Value) -> Result<Value, String> {
        let par = body.get("params").cloned().unwrap_or(Value::Null);
        let get = |k: &str| par.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(-4.0, 4.0).trunc() as i32;
        let gain = body.get("gain").and_then(|v| v.as_f64()).filter(|g| *g != 0.0).unwrap_or(6144.0).trunc() as i32;
        let p = ToneParams { key: get("key"), contrast: get("contrast"), highlight: get("highlight"),
                             shadow: get("shadow"), gain, flag: 0 };
        let edit = body.get("edit").cloned().unwrap_or(Value::Null);
        let pts = points(edit.get("points").unwrap_or(&Value::Null));
        let fams = families(edit.get("families").unwrap_or(&Value::Null));
        let reference = self.tone_curve(p)?;
        let lut = if tone::is_identity(&pts) { reference.clone() } else { self.engine_for(&pts, &fams).curve(&self.fw, p)? };
        let mut out = json!({ "lut": lut, "lut_ref": reference, "transfer": tone::transfer(&pts, 129) });

        let chan_req = body.get("chan").cloned().unwrap_or(Value::Null);
        let original: Vec<Vec<i64>> = chan::read(&self.fw).into_iter().map(|c| c.into_iter().map(i64::from).collect()).collect();
        let edits = chan_req.get("edits").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        let pre = effective_channels(&original, &edits)?;
        let effective = graded_channels(&pre, chan_req.get("grade").unwrap_or(&Value::Null));
        out["channel_curves"] = Value::from(effective.iter().map(|c| ints(c)).collect::<Vec<_>>());
        out["channel_curves_pre"] = Value::from(pre.iter().map(|c| ints(c)).collect::<Vec<_>>());
        out["channel_transfer"] = Value::Object(
            edits.iter().map(|(k, e)| (k.clone(), json!(tone::transfer(&points(e.get("points").unwrap_or(&Value::Null)), 129)))).collect(),
        );
        if let Some(set) = chan_req.get("set").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
            let idx: Vec<usize> = set.iter().filter_map(|v| v.as_u64().map(|x| x as usize)).filter(|&i| i < chan::COUNT).collect();
            out["rgb"] = Value::from(idx.iter().map(|&i| json!(chan::compose(&lut, &effective[i]))).collect::<Vec<_>>());
            out["rgb_ref"] = Value::from(idx.iter().map(|&i| json!(chan::compose(&reference, &original[i]))).collect::<Vec<_>>());
        }
        Ok(out)
    }

    /// POST /api/export: the patched image packed into an update file, and
    /// the preset. Returns (result JSON, update file, preset JSON text).
    pub fn export(&self, body: &Value) -> Result<(Value, Vec<u8>, String), String> {
        let empty = Map::new();
        let matrices = body.get("matrices").and_then(|v| v.as_object()).unwrap_or(&empty);
        let tone_req = body.get("tone").cloned().unwrap_or(Value::Null);
        let pts = points(tone_req.get("points").unwrap_or(&Value::Null));
        let tone_changed = !tone::is_identity(&pts);
        let mut channels: Vec<(usize, Vec<f64>)> = vec![];
        if let Some(ch) = body.get("channels").and_then(|v| v.as_object()) {
            for (k, v) in ch {
                let i: usize = k.parse().map_err(|_| format!("channel curve {k}: bad index"))?;
                let vals: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
                channels.push((i, vals));
            }
        }
        // even without edits the export is a new look: Cross Process with
        // linear curves and its own matrix
        let mut img = self.fw.clone();
        looks::patch(&mut img, matrices)?;
        if tone_changed {
            let fams = families(tone_req.get("families").unwrap_or(&Value::Null));
            let edit: Vec<(usize, Vec<u16>)> = tone::apply(&gamma::base_curves(&img), &pts, &fams)
                .into_iter()
                .map(|(i, v)| (i, v.into_iter().map(|x| x.clamp(0, 0xFFFF) as u16).collect()))
                .collect();
            gamma::with_curves(&mut img, &edit);
        }
        if !channels.is_empty() {
            chan::write(&mut img, &channels)?;
        }
        let or_null = |k: &str| match body.get(k) {
            Some(v) if !v.is_null() && v != &json!({}) && v != &json!([]) => v.clone(),
            _ => Value::Null,
        };
        let channel_json = if channels.is_empty() { Value::Null } else { body["channels"].clone() };
        let preset = json!({
            "source": format!("{}/unpackedFW", self.build.name),
            "matrices": matrices,
            "tone": if tone_changed { tone_req.clone() } else { Value::Null },
            "channel_curves": channel_json,
            // the points behind them, to edit the curves again
            "channel_edits": or_null("channel_edits"),
            // to edit the wheels again: the curves before grading and the wheels
            "channel_curves_pre": or_null("channels_pre"),
            "channel_grade": or_null("grade"),
            // base + sliders (incl. calibration) to edit again; the camera uses "matrices"
            "matrix_edits": or_null("matrix_edits"),
            "inherit": if self.inherit.is_empty() { Value::Null } else { self.inherit_json() },
        });
        let preset_text = serde_json::to_string_pretty(&preset).unwrap();

        grfw::fix_image_checksum(&mut img);
        let (packed, redo) = grfw::repack(&self.container, &self.stock, &img)?;
        // verify before handing it out: every check of the camera's decoder,
        // and the file must decode to exactly the patched image
        let mut rebuilt = grfw::Container::parse(packed)?;
        let problems = rebuilt.check(Some(self.container.project), Some(self.container.fw_type()));
        if !problems.is_empty() || rebuilt.decode()? != img {
            return Err("the rebuilt update file does not verify; not written".into());
        }
        let packed = rebuilt.data;
        let result = json!({
            "written": format!("{}.looks.json", self.build.name),
            "preset": format!("{}.looks.json", self.build.name),
            "update_file": self.update_name,
            "frames_reencoded": redo,
            "update_sha256": sha256_hex(&packed),
        });
        Ok((result, packed, preset_text))
    }
}

/// the 12 channel curves after edits {index: {base: [256] | None, points}}
fn effective_channels(original: &[Vec<i64>], edits: &Map<String, Value>) -> Result<Vec<Vec<i64>>, String> {
    let mut eff = original.to_vec();
    for (key, e) in edits {
        let i: usize = key.parse().map_err(|_| format!("bad channel index {key}"))?;
        if i >= eff.len() {
            return Err(format!("bad channel index {key}"));
        }
        let base: Vec<f64> = match e.get("base").and_then(|b| b.as_array()) {
            Some(b) if !b.is_empty() => b.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect(),
            _ => original[i].iter().map(|&v| v as f64).collect(),
        };
        let pts = points(e.get("points").unwrap_or(&Value::Null));
        eff[i] = if tone::is_identity(&pts) {
            base.iter().map(|v| v.trunc() as i64).collect()
        } else {
            let t = tone::transfer(&pts, 1025);
            let grid = tone::linspace(1025);
            base.iter()
                .map(|&x| crate::addr::round_even(tone::interp((x / tone::FULL).clamp(0.0, 1.0), &grid, &t) * tone::FULL) as i64)
                .collect()
        };
    }
    Ok(eff)
}

/// the 12 curves with colour grading wheels applied per set {set name: wheels}
fn graded_channels(curves: &[Vec<i64>], grades: &Value) -> Vec<Vec<i64>> {
    let mut out = curves.to_vec();
    let Some(g) = grades.as_object() else { return out };
    for (name, wheels) in g {
        let Some((_, idx)) = chan::SETS.iter().find(|(n, _)| n == name) else { continue };
        if chan::grade_is_neutral(wheels) {
            continue;
        }
        let rgb: Vec<Vec<i64>> = idx.iter().map(|&i| curves[i].clone()).collect();
        for (&i, c) in idx.iter().zip(chan::grade(&rgb, wheels)) {
            out[i] = c;
        }
    }
    out
}

/// what the preview needs from one DNG
pub struct Prepared {
    pub meta: Value,
    /// oriented linear RGB, f32
    pub linear: Vec<f32>,
    /// the camera JPEG, resized to the preview and oriented, RGBA
    pub camera: Option<Vec<u8>>,
}

impl Session {
    /// POST /api/dng, after the browser decoded the embedded JPEG to RGBA at
    /// the size of the (unoriented) preview: meta, linear image, camera image
    pub fn prepare_shot(&self, shot: &crate::dng::Shot, camera_rgba: Option<&[u8]>, filename: &str) -> Result<Prepared, String> {
        use crate::{dng, fit};
        let tone_label = shot.image_tone.and_then(looks::image_tone_look);
        let shot_look = tone_label.map(|t| t.1);
        let camera: Option<Vec<f32>> = camera_rgba
            .filter(|c| c.len() == shot.width * shot.height * 4)
            .map(|c| c.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).map(|v| v as f32 / 255.0).collect());

        let (mut gain, mut scale, mut residual) = (0x1800, 4000.0, None);
        if let Some(cam) = &camera {
            let all = looks::looks(&self.fw);
            let name = shot_look.unwrap_or("Standard");
            let r = all.iter().find(|l| l["name"] == name).unwrap();
            let sat_setting = if name == "CrossProcess" { 0 } else {
                shot.params.iter().find(|p| p.0 == "saturation").map(|p| p.1).unwrap_or(0)
            };
            let sat: Vec<f64> = serde_json::from_value(r["sat_matrices"][(sat_setting + 4).clamp(0, 8) as usize].clone()).unwrap();
            let m: Vec<f64> = serde_json::from_value(r["matrix"].clone()).unwrap();
            let look: Vec<f64> = (0..9).map(|k| (0..3).map(|j| sat[3 * (k / 3) + j] * m[3 * j + k % 3]).sum()).collect();
            let rendered = fit::render_linear(&shot.linear, shot.wb, &looks::sensor_ccm(&self.fw), &look,
                                              r["mono_mix"].as_bool().unwrap_or(false));
            let engine = Engine::new(&self.fw, &[]);
            let curve = |g: i32| engine.curve(&self.fw, fit::tone_params(&shot.params, g));
            let (g, s, res) = fit::fit_camera_tone(&rendered, cam, &curve)?;
            (gain, scale, residual) = (g, s, Some(res));
        }
        let (linear, w, h) = dng::orient(&shot.linear, shot.width, shot.height, 3, shot.flip);
        let camera = camera_rgba
            .filter(|c| c.len() == shot.width * shot.height * 4)
            .map(|c| dng::orient(c, shot.width, shot.height, 4, shot.flip).0);
        let params: Map<String, Value> = shot.params.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
        let meta = json!({
            "width": w, "height": h, "wb": shot.wb, "model": shot.model,
            "image_tone": tone_label.map(|t| t.0), "shot_look": shot_look,
            "params": params, "gain": gain, "scale": scale, "residual": residual,
            "filename": filename,
        });
        Ok(Prepared { meta, linear, camera })
    }
}
