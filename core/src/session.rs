//! The editor's requests, without a server: one Session holds the update
//! file the user dropped in, the decoded image and the current look data.
//! Requests and answers are JSON, routed by editor/static/bridge.js.

use crate::{chan, curvesel, gamma, grfw, looks, multiaxial, space, tone};
use crate::targets::{self, Target, CROSS};
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
    /// the preview's image: stock + linear Cross Process curves + the targets
    fw: Vec<u8>,
    /// targets of the preview, in targets::TARGETS order; Cross Process is
    /// always one (it is the slot of the earlier editor's own look)
    targets: Vec<Target>,
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
            container, fw: vec![], stock, targets: vec![Target::new(CROSS, None, false)?], build, filename,
            update_name: if name.is_empty() { format!("{}.BIN", build.name.to_uppercase()) } else { name },
            base_curves,
        };
        s.rebuild()?;
        Ok(s)
    }

    /// the stock image with `targets` at `places` (targets::item_sizes, placed
    /// by space::plan_with); Cross Process as a target starts with linear curves
    fn build(&self, targets: &[Target], places: &[u32]) -> Result<Vec<u8>, String> {
        let mut img = self.stock.clone();
        if targets.iter().any(|t| t.name == CROSS) {
            let lin: Vec<f64> = chan::linear().iter().map(|&v| v as f64).collect();
            chan::write(&mut img, &(0..chan::COUNT).map(|i| (i, lin.clone())).collect::<Vec<_>>())?;
        }
        targets::apply(&mut img, &self.stock, targets, places)?;
        Ok(img)
    }

    /// `targets` placed next to `hsl` HSL sets and `curves` channel curves
    fn build_for(&self, targets: &[Target], hsl: usize, curves: usize) -> Result<(Vec<u8>, space::Plan), String> {
        let plan = space::plan_with(&self.stock, hsl, curves, &targets::item_sizes(&self.stock, targets))?;
        Ok((self.build(targets, &plan.items)?, plan))
    }

    /// fw for the preview: where the targets' sets lie does not matter there
    fn rebuild(&mut self) -> Result<(), String> {
        self.fw = self.build_for(&self.targets.clone(), 0, 0)?.0;
        Ok(())
    }

    fn set_targets(&mut self, targets: Vec<Target>) -> Result<(), String> {
        let old = std::mem::replace(&mut self.targets, targets::sorted(targets)?);
        if let Err(e) = self.rebuild() {
            self.targets = old;
            self.rebuild()?;
            return Err(e);
        }
        Ok(())
    }

    fn target(&self, name: &str) -> Option<&Target> {
        self.targets.iter().find(|t| t.name == name)
    }

    pub fn image(&self) -> &[u8] {
        &self.fw
    }

    fn hdr_off(&self) -> bool {
        self.target(HDR).is_some_and(|t| t.effects_off)
    }

    fn effects_off(&self) -> Vec<String> {
        if self.hdr_off() { vec![HDR.to_string()] } else { vec![] }
    }

    /// {target: source} for targets built from another look
    fn inherit_json(&self) -> Value {
        Value::Object(self.targets.iter().filter(|t| t.foreign()).map(|t| (t.name.clone(), json!(t.source))).collect())
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
            "effects_off": self.effects_off(),
            "targets": self.targets.iter().map(Target::to_json).collect::<Vec<_>>(),
            "target_names": targets::TARGETS,
            "sources": targets::sources().collect::<Vec<_>>(),
            "curve_looks": CURVE_LOOKS,
            "look_channel_sets": Value::Object(look_sets().map(|(n, i)| (n.to_string(), json!(i))).collect()),
            "multiaxial": { "ycc": multiaxial::ycc(img, 11), "circle": multiaxial::CIRCLE },
            "tone_control_base": crate::tonecontrol::base(img),
        })
    }

    /// POST /api/hsl {look, hsl}: what the MultiAxial stage gets for the
    /// look's HSL settings; the preview applies exactly that
    pub fn hsl(&self, body: &Value) -> Result<Value, String> {
        let name = body.get("look").and_then(|v| v.as_str()).unwrap_or("");
        let table = looks::table_of(name).ok_or(format!("unknown look {name:?}"))?;
        let ycc = multiaxial::ycc(&self.fw, table);
        let h = multiaxial::Hsl::from_json(body.get("hsl").unwrap_or(&Value::Null));
        let mut out = multiaxial::encode(&h, &ycc)?.to_json();
        out["ycc"] = json!(ycc);
        Ok(out)
    }

    /// the post matrix a target keeps when a request changes something else
    fn post_of(&self, name: &str) -> Option<[f64; 9]> {
        self.target(name).and_then(|t| t.post)
    }

    /// POST /api/target {target, source, effects_off, post}: make `target` a
    /// look of the user's built from `source` (default: itself), with a post
    /// matrix (9 values or null; default: as it had); `remove: true` gives it
    /// back to stock (Cross Process goes back to its own data)
    pub fn set_target(&mut self, body: &Value) -> Result<Value, String> {
        let name = body.get("target").and_then(|v| v.as_str()).unwrap_or("");
        let post = match body.get("post") {
            Some(v) if v.is_null() => None,
            Some(v) => Some(crate::postmatrix::parse(v)?),
            None => self.post_of(name),
        };
        let mut list: Vec<Target> = self.targets.iter().filter(|t| t.name != name).cloned().collect();
        if body.get("remove").and_then(|v| v.as_bool()) == Some(true) {
            if name == CROSS {
                list.push(Target::new(CROSS, None, false)?);
            } else if !targets::TARGETS.contains(&name) {
                return Err(format!("{name:?} cannot be a target"));
            }
        } else {
            let source = body.get("source").and_then(|v| v.as_str());
            let off = body.get("effects_off").and_then(|v| v.as_bool()).unwrap_or(false);
            list.push(Target::new(name, source, off)?.with_post(post));
        }
        self.set_targets(list)?;
        Ok(self.firmware())
    }

    /// POST /api/inherit {target, source}; source null undoes it (the earlier
    /// editor's call, on top of set_target)
    pub fn set_inherit(&mut self, body: &Value) -> Result<Value, String> {
        let name = body.get("target").and_then(|v| v.as_str()).unwrap_or("");
        let source = body.get("source").and_then(|v| v.as_str());
        if source == Some(name) {
            return Err("unknown look".into());
        }
        let off = self.target(name).is_some_and(|t| t.effects_off);
        let post = self.post_of(name);
        let mut list: Vec<Target> = self.targets.iter().filter(|t| t.name != name).cloned().collect();
        if source.is_some() || off || post.is_some() || name == CROSS {
            list.push(Target::new(name, source, off)?.with_post(post));
        } else if !targets::TARGETS.contains(&name) {
            return Err("unknown look".into());
        }
        self.set_targets(list)?;
        Ok(self.firmware())
    }

    /// POST /api/effects {effects_off: ["HdrCuisine"] | []}
    pub fn set_effects(&mut self, body: &Value) -> Result<Value, String> {
        let names: Vec<String> = body.get("effects_off").and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        if let Some(n) = names.iter().find(|n| *n != HDR) {
            return Err(format!("effects can only be switched off for {HDR}, not {n:?}"));
        }
        let off = !names.is_empty();
        let source = self.target(HDR).map(|t| t.source.clone());
        let post = self.post_of(HDR);
        let mut list: Vec<Target> = self.targets.iter().filter(|t| t.name != HDR).cloned().collect();
        if off || post.is_some() || source.as_deref().is_some_and(|s| s != HDR) {
            list.push(Target::new(HDR, source.as_deref(), off)?.with_post(post));
        }
        self.set_targets(list)?;
        Ok(self.firmware())
    }

    /// POST /api/space {hsl, look_curves}: the shared free space for the
    /// editor's meter, counted as export would place it, with the colour
    /// sets of the session's targets
    pub fn space(&self, body: &Value) -> Result<Value, String> {
        let hsl = body.get("hsl").and_then(|v| v.as_object()).map_or(0, |h| {
            h.values().filter(|v| !multiaxial::Hsl::from_json(v).is_neutral()).count()
        });
        let mut rgb = parse_look_curves(body)?.len();
        if let Some(names) = body.get("looks").and_then(|v| v.as_array()) {
            for n in names {
                let n = n.as_str().unwrap_or("");
                if !CURVE_LOOKS.contains(&n) {
                    return Err(format!("no channel curves for {n:?}"));
                }
            }
            rgb += names.len();
        }
        Ok(space::report_with(&self.stock, hsl, rgb, &targets::item_sizes(&self.stock, &self.targets)))
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
        let mut original: Vec<Vec<i64>> = chan::read(&self.fw).into_iter().map(|c| c.into_iter().map(i64::from).collect()).collect();
        // the other looks' curves (look_sets): none in stock, they start linear
        let lin: Vec<i64> = chan::linear().into_iter().map(i64::from).collect();
        original.extend(std::iter::repeat_n(lin, 3 * CURVE_LOOKS.len()));
        let edits = chan_req.get("edits").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        let pre = effective_channels(&original, &edits)?;
        let effective = graded_channels(&pre, chan_req.get("grade").unwrap_or(&Value::Null));
        out["channel_curves"] = Value::from(effective.iter().map(|c| ints(c)).collect::<Vec<_>>());
        out["channel_curves_pre"] = Value::from(pre.iter().map(|c| ints(c)).collect::<Vec<_>>());
        out["channel_transfer"] = Value::Object(
            edits.iter().map(|(k, e)| (k.clone(), json!(tone::transfer(&points(e.get("points").unwrap_or(&Value::Null)), 129)))).collect(),
        );
        if let Some(cs) = chan_req.get("curves").filter(|v| !v.is_null()) {
            // the channel curves of any look: [R, G, B]; stock has none there
            let curves = curve_triplet(cs).map_err(|e| format!("chan.curves: {e}"))?;
            out["rgb"] = Value::from(curves.iter().map(|c| {
                let c: Vec<i64> = c.iter().map(|&v| v as i64).collect();
                json!(chan::compose(&lut, &c))
            }).collect::<Vec<_>>());
            out["rgb_ref"] = json!([reference, reference, reference]);
        } else if let Some(set) = chan_req.get("set").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
            let idx: Vec<usize> = set.iter().filter_map(|v| v.as_u64().map(|x| x as usize)).filter(|&i| i < original.len()).collect();
            out["rgb"] = Value::from(idx.iter().map(|&i| json!(chan::compose(&lut, &effective[i]))).collect::<Vec<_>>());
            // stock applies no curves to the other looks
            out["rgb_ref"] = Value::from(idx.iter().map(|&i| {
                if i < chan::COUNT { json!(chan::compose(&reference, &original[i])) } else { json!(reference) }
            }).collect::<Vec<_>>());
        }
        Ok(out)
    }

    /// POST /api/export: the patched image packed into an update file, and
    /// the preset. Returns (result JSON, update file, preset JSON text).
    /// A body with "targets" is the target-first format (export_targets);
    /// otherwise the earlier editor's, with the session's targets.
    pub fn export(&self, body: &Value) -> Result<(Value, Vec<u8>, String), String> {
        if body.get("targets").is_some() { self.export_targets(body) } else { self.export_v1(body) }
    }

    fn export_v1(&self, body: &Value) -> Result<(Value, Vec<u8>, String), String> {
        let empty = Map::new();
        let matrices = body.get("matrices").and_then(|v| v.as_object()).unwrap_or(&empty);
        // the tone curve for all image controls is gone (plan-gui-redesign.md)
        let tone_req = body.get("tone").cloned().unwrap_or(Value::Null);
        let tone_ignored = !tone::is_identity(&points(tone_req.get("points").unwrap_or(&Value::Null)));
        let mut channels: Vec<(usize, Vec<f64>)> = vec![];
        if let Some(ch) = body.get("channels").and_then(|v| v.as_object()) {
            for (k, v) in ch {
                let i: usize = k.parse().map_err(|_| format!("channel curve {k}: bad index"))?;
                let vals: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
                channels.push((i, vals));
            }
        }
        // indices 12.. are the other looks' curves (look_sets)
        let mut look_curves = parse_look_curves(body)?;
        let (own, others): (Vec<_>, Vec<_>) = channels.into_iter().partition(|(i, _)| *i < chan::COUNT);
        let channels = own;
        let mut from_channels: Vec<(u32, Vec<Vec<u16>>)> = vec![];
        for (i, vals) in others {
            let k = i - chan::COUNT;
            let Some(name) = CURVE_LOOKS.get(k / 3) else { return Err(format!("channel curve {i}: bad index")) };
            if vals.len() != 256 {
                return Err(format!("channel curve {i}: need 256 values"));
            }
            let index = look_index(name);
            if look_curves.iter().any(|(l, _)| *l == index) {
                return Err(format!("curves for {name} given twice"));
            }
            let pos = match from_channels.iter().position(|(l, _)| *l == index) {
                Some(p) => p,
                None => {
                    from_channels.push((index, vec![chan::linear(); 3]));
                    from_channels.len() - 1
                }
            };
            from_channels[pos].1[k % 3] = vals.iter().map(|&v| crate::addr::clamp_u16(v)).collect();
        }
        look_curves.extend(from_channels);
        look_curves.sort_by_key(|(l, _)| CURVE_LOOKS.iter().position(|n| look_index(n) == *l));
        if !look_curves.is_empty() && channels.iter().any(|(i, _)| *i < 3) {
            return Err("the Random curves of Cross Process cannot be combined with curves for other looks".into());
        }
        let hsl_in: Vec<(String, Value)> = body.get("hsl").and_then(|v| v.as_object())
            .map(|h| h.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
        // even without edits the export is a new look: Cross Process with
        // linear curves and its own matrix
        let (mut img, plan) = self.build_for(&self.targets, count_hsl(&hsl_in), 3 * look_curves.len())?;
        looks::patch(&mut img, matrices)?;
        let (sets, hsl_out) = encode_hsl(&img, &hsl_in)?;
        multiaxial::patch(&mut img, &sets, &plan.hsl)?;
        if !channels.is_empty() {
            chan::write(&mut img, &channels)?;
        }
        install_look_curves(&mut img, &look_curves, &plan.curves)?;
        if self.hdr_off() {
            curvesel::hdr_effects_off(&mut img)?;
        }
        let or_null = |k: &str| match body.get(k) {
            Some(v) if !v.is_null() && v != &json!({}) && v != &json!([]) => v.clone(),
            _ => Value::Null,
        };
        let channel_json = match body.get("channels") {
            Some(c) if c.as_object().is_some_and(|o| !o.is_empty()) => c.clone(),
            _ => Value::Null,
        };
        let inherit = self.inherit_json();
        let preset = json!({
            "source": format!("{}/unpackedFW", self.build.name),
            "matrices": matrices,
            "tone": Value::Null,
            "channel_curves": channel_json,
            // the points behind them, to edit the curves again
            "channel_edits": or_null("channel_edits"),
            // to edit the wheels again: the curves before grading and the wheels
            "channel_curves_pre": or_null("channels_pre"),
            "channel_grade": or_null("grade"),
            // base + sliders (incl. calibration) to edit again; the camera uses "matrices"
            "matrix_edits": or_null("matrix_edits"),
            // per-hue adjustments (MultiAxial) per look
            "hsl": if hsl_out.is_empty() { Value::Null } else { Value::Object(hsl_out) },
            "inherit": if inherit == json!({}) { Value::Null } else { inherit },
            // channel curves of looks other than Cross Process {look: [R, G, B]}
            "look_curves": if look_curves.is_empty() { Value::Null } else {
                Value::Object(look_curves.iter().map(|(l, c)| {
                    let name = CURVE_LOOKS.iter().find(|n| look_index(n) == *l).unwrap();
                    (name.to_string(), json!(c))
                }).collect())
            },
            "effects_off": if self.hdr_off() { json!(self.effects_off()) } else { Value::Null },
        });
        let space = space::report_with(&self.stock, sets.len(), look_curves.len(), &targets::item_sizes(&self.stock, &self.targets));
        let (mut result, packed) = self.pack(img, space)?;
        if tone_ignored {
            result["tone_ignored"] = json!(true);
        }
        Ok((result, packed, serde_json::to_string_pretty(&preset).unwrap()))
    }

    /// The target-first export (plan-gui-redesign.md):
    ///
    ///     {"targets": {"Vivid": {"source": "NegativeFilm", "matrices": {"Vivid": [9]},
    ///                            "hsl": {..}, "curves": [R, G, B], "edit": any},
    ///                  "CrossProcess": {"channels": {"3": [256], ..}, ..},
    ///                  "HdrCuisine": {"effects_off": true, ..},
    ///                  "NegativeFilm": {"post": [9], ..}}}
    ///
    /// Only the targets change; every other look stays byte-identical to
    /// stock. `edit` is the editor's own state, kept in the preset as it is.
    fn export_targets(&self, body: &Value) -> Result<(Value, Vec<u8>, String), String> {
        let specs = body["targets"].as_object().ok_or("targets: need an object")?;
        let mut list = vec![];
        for (name, spec) in specs {
            let source = spec.get("source").and_then(|v| v.as_str());
            let off = spec.get("effects_off").and_then(|v| v.as_bool()).unwrap_or(false);
            let post = match spec.get("post") {
                Some(v) if !v.is_null() => Some(crate::postmatrix::parse(v).map_err(|e| format!("{name}: {e}"))?),
                _ => None,
            };
            list.push(Target::new(name, source, off)?.with_post(post));
        }
        let list = targets::sorted(list)?;
        let mut matrices = Map::new();
        let mut hsl_in = vec![];
        let mut look_curves: Vec<LookCurves> = vec![];
        let mut channels: Vec<(usize, Vec<f64>)> = vec![];
        for t in &list {
            let spec = &specs[&t.name];
            if let Some(m) = spec.get("matrices").and_then(|v| v.as_object()) {
                for (look, v) in m {
                    if !t.owns_matrix(look) {
                        return Err(if t.name == HDR {
                            format!("{HDR} gets a matrix of its own only with its effects off")
                        } else {
                            format!("target {} cannot write the matrix of {look:?}", t.name)
                        });
                    }
                    matrices.insert(look.clone(), v.clone());
                }
            }
            if let Some(h) = spec.get("hsl").filter(|v| !v.is_null()) {
                hsl_in.push((t.name.clone(), h.clone()));
            }
            if let Some(c) = spec.get("curves").filter(|v| !v.is_null()) {
                if t.name == CROSS {
                    return Err("Cross Process keeps its Color Tone curves (channels)".into());
                }
                let curves = curve_triplet(c).map_err(|e| format!("curves for {}: {e}", t.name))?;
                look_curves.push((look_index(&t.name), curves));
            }
            if let Some(ch) = spec.get("channels").and_then(|v| v.as_object()) {
                if t.name != CROSS {
                    return Err(format!("{}: channels are Cross Process's Color Tone curves; use curves", t.name));
                }
                for (k, v) in ch {
                    let i: usize = k.parse().ok().filter(|&i| i < chan::COUNT).ok_or(format!("channel curve {k}: bad index"))?;
                    let vals: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
                    channels.push((i, vals));
                }
            }
        }
        if !look_curves.is_empty() && channels.iter().any(|(i, _)| *i < 3) {
            return Err("the Random curves of Cross Process cannot be combined with curves for other looks".into());
        }
        let (mut img, plan) = self.build_for(&list, count_hsl(&hsl_in), 3 * look_curves.len())?;
        looks::patch(&mut img, &matrices)?;
        let (sets, _) = encode_hsl(&img, &hsl_in)?;
        multiaxial::patch(&mut img, &sets, &plan.hsl)?;
        if !channels.is_empty() {
            chan::write(&mut img, &channels)?;
        }
        install_look_curves(&mut img, &look_curves, &plan.curves)?;
        if list.iter().any(|t| t.effects_off) {
            curvesel::hdr_effects_off(&mut img)?;
        }
        // the preset: the request as it was accepted, sources filled in
        let mut out = Map::new();
        for t in &list {
            let mut spec = specs[&t.name].clone();
            spec["source"] = json!(t.source);
            spec["effects_off"] = json!(t.effects_off);
            out.insert(t.name.clone(), spec);
        }
        let preset = json!({
            "version": 2,
            "firmware": format!("{}/unpackedFW", self.build.name),
            "targets": out,
        });
        let space = space::report_with(&self.stock, sets.len(), look_curves.len(), &targets::item_sizes(&self.stock, &list));
        let (result, packed) = self.pack(img, space)?;
        Ok((result, packed, serde_json::to_string_pretty(&preset).unwrap()))
    }

    /// checksum, repack and verify the patched image the way the camera's
    /// decoder does; the file must decode to exactly `img`
    fn pack(&self, mut img: Vec<u8>, space: Value) -> Result<(Value, Vec<u8>), String> {
        grfw::fix_image_checksum(&mut img);
        let (packed, redo) = grfw::repack(&self.container, &self.stock, &img)?;
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
            "space": space,
        });
        Ok((result, packed))
    }
}

fn count_hsl(hsl: &[(String, Value)]) -> usize {
    hsl.iter().filter(|(_, v)| !multiaxial::Hsl::from_json(v).is_neutral()).count()
}

/// (table, encoded HSL) per look, and the HSL settings that made them
type HslSets = (Vec<(u32, multiaxial::Encoded)>, Map<String, Value>);

/// a MultiAxial set of its own for each look with HSL that is not neutral
fn encode_hsl(img: &[u8], hsl: &[(String, Value)]) -> Result<HslSets, String> {
    let mut sets = vec![];
    let mut out = Map::new();
    for (name, v) in hsl {
        let table = looks::table_of(name).ok_or(format!("HSL for unknown look {name:?}"))?;
        let parsed = multiaxial::Hsl::from_json(v);
        if parsed.is_neutral() {
            continue;
        }
        let e = multiaxial::encode(&parsed, &multiaxial::ycc(img, table)).map_err(|e| format!("HSL {name}: {e}"))?;
        sets.push((table, e));
        out.insert(name.clone(), v.clone());
    }
    Ok((sets, out))
}

/// curves for looks other than Cross Process: into the pool at `places`, and
/// the selector that picks them
fn install_look_curves(img: &mut [u8], look_curves: &[LookCurves], places: &[u32]) -> Result<(), String> {
    if look_curves.is_empty() {
        return Ok(());
    }
    let mut entries = vec![];
    for ((look, curves), at) in look_curves.iter().zip(places.chunks(3)) {
        for (c, &va) in curves.iter().zip(at) {
            crate::addr::put_curve(img, va, c);
        }
        let e = curvesel::entry(*look, 0).unwrap();
        entries.push((e, [at[0], at[1], at[2]]));
    }
    curvesel::install(img, &entries)
}


use crate::targets::HDR;

/// looks that can get channel curves of their own (Cross Process has its
/// Color Tone sets; monochrome looks have no colour to curve)
pub const CURVE_LOOKS: [&str; 7] =
    ["Standard", "Vivid", "PositiveFilm", "BleechBypass_GR", "Retro", "HdrCuisine", "NegativeFilm"];

/// menu index of a look name
fn look_index(name: &str) -> u32 {
    looks::LOOKS.iter().find(|l| l.2 == name).unwrap().0
}

/// channel curve indices of the other looks in the editor: 12 + 3k.. for CURVE_LOOKS[k]
pub fn look_sets() -> impl Iterator<Item = (&'static str, [usize; 3])> {
    CURVE_LOOKS.iter().enumerate().map(|(k, &n)| (n, [12 + 3 * k, 13 + 3 * k, 14 + 3 * k]))
}

/// [R, G, B], 256 values each, 0..65535
fn curve_triplet(v: &Value) -> Result<Vec<Vec<u16>>, String> {
    let a = v.as_array().filter(|a| a.len() == 3).ok_or("need three curves (R, G, B)")?;
    a.iter().map(|c| {
        let vals: Vec<u16> = c.as_array().map(|x| {
            x.iter().filter_map(|n| n.as_f64()).filter(|n| n.is_finite())
                .map(crate::addr::clamp_u16).collect()
        }).unwrap_or_default();
        if vals.len() == 256 { Ok(vals) } else { Err("a curve needs 256 values".to_string()) }
    }).collect()
}

/// (menu index, [R, G, B])
type LookCurves = (u32, Vec<Vec<u16>>);

/// body.look_curves {look name: [R, G, B]} -> in CURVE_LOOKS order
fn parse_look_curves(body: &Value) -> Result<Vec<LookCurves>, String> {
    let Some(m) = body.get("look_curves").and_then(|v| v.as_object()) else { return Ok(vec![]) };
    for name in m.keys() {
        if !CURVE_LOOKS.contains(&name.as_str()) {
            return Err(match name.as_str() {
                "CrossProcess" => "Cross Process keeps its Color Tone curves (channels)".into(),
                _ => format!("no channel curves for {name:?}"),
            });
        }
    }
    let mut out = vec![];
    for name in CURVE_LOOKS {
        let Some(v) = m.get(name).filter(|v| !v.is_null()) else { continue };
        let curves = curve_triplet(v).map_err(|e| format!("curves for {name}: {e}"))?;
        out.push((look_index(name), curves));
    }
    Ok(out)
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
        let idx: Vec<usize> = match chan::SETS.iter().find(|(n, _)| n == name) {
            Some((_, i)) => i.to_vec(),
            None => match look_sets().find(|(n, _)| n == name) {
                Some((_, i)) => i.to_vec(),
                None => continue,
            },
        };
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
        self.prepare_shot_with(shot, camera_rgba, filename, true)
    }

    /// as prepare_shot; `vignette_fit` false leaves out what the earlier
    /// editor did not model -- lens shading, the shot's shadow correction and
    /// the highlight chroma roll-off -- so the fit matches its answers
    pub fn prepare_shot_with(&self, shot: &crate::dng::Shot, camera_rgba: Option<&[u8]>, filename: &str,
                             vignette_fit: bool) -> Result<Prepared, String> {
        use crate::{dng, fit};
        let tone_label = shot.image_tone.and_then(looks::image_tone_look);
        let shot_look = tone_label.map(|t| t.1);
        let mut camera: Option<Vec<f32>> = camera_rgba
            .filter(|c| c.len() == shot.width * shot.height * 4)
            .map(|c| c.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).map(|v| v as f32 / 255.0).collect());
        // the fits below render the look without its post matrix: take it out
        // of the camera JPEG too (the shot is taken as made with this look)
        if let (Some(cam), Some(p)) = (camera.as_mut(), shot_look.and_then(|n| self.post_of(n))) {
            crate::postmatrix::undo(&p, cam);
        }

        let (mut gain, mut scale, mut residual) = (0x1800, 4000.0, None);
        // Retro and HDR Tone change the edges themselves: their shots show the
        // lens correction only together with that, so they get the default
        let own_edges = matches!(shot_look, Some("Retro") | Some("HdrCuisine"));
        let (mut vignette, mut vignette_source) = (fit::DEFAULT_VIGNETTE, "default");
        let mut chroma = fit::DEFAULT_HIGHLIGHT_CHROMA;
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
            // the shot's own shadow correction, as the camera applied it
            let tc = shot.shadow_correction.filter(|_| vignette_fit)
                .and_then(|l| crate::tonecontrol::table(&crate::tonecontrol::base(&self.fw), crate::tonecontrol::strength(l)));
            let tc = tc.as_deref();
            let (mut g, mut s, mut res) = fit::fit_camera_tone(&rendered, cam, &curve, tc)?;
            if vignette_fit && !own_edges {
                let lut: Vec<f64> = curve(g)?.iter().map(|&v| v as f64).collect();
                if let Some(v) = fit::fit_vignette(&rendered, cam, shot.width, shot.height, &lut, s, tc) {
                    (vignette, vignette_source) = (v, "fitted");
                }
            }
            let corrected = if vignette_fit {
                // scale and fine match again, on the corrected image
                let c = fit::apply_vignette(&rendered, shot.width, shot.height, &vignette);
                (g, s, res) = fit::fit_camera_tone(&c, cam, &curve, tc)?;
                c
            } else {
                rendered
            };
            if vignette_fit {
                let lut: Vec<f64> = curve(g)?.iter().map(|&v| v as f64).collect();
                chroma = fit::fit_highlight_chroma(&corrected, cam, &lut, s, &res, tc);
            }
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
            "shadow_correction": shot.shadow_correction,
            "highlight_chroma": if vignette_fit { json!(chroma) } else { Value::Null },
            "vignette": if vignette_fit { json!({ "coef": vignette, "source": vignette_source }) } else { Value::Null },
            "filename": filename,
        });
        Ok(Prepared { meta, linear, camera })
    }
}
