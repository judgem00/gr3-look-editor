//! JavaScript bindings for the web worker (editor/static/worker.js). JSON in
//! and out, as session.rs takes and returns it; image data as typed arrays.

use crate::dng::{self, Shot};
use crate::session::Session;
use wasm_bindgen::prelude::*;

fn js(e: String) -> JsError {
    JsError::new(&e)
}

fn parse(body: &str) -> Result<serde_json::Value, JsError> {
    serde_json::from_str(body).map_err(|e| JsError::new(&format!("bad request: {e}")))
}

#[wasm_bindgen]
pub struct Editor {
    session: Session,
    shot: Option<Shot>,
}

#[wasm_bindgen]
pub struct Export {
    result: String,
    update: Vec<u8>,
    preset: String,
}

#[wasm_bindgen]
impl Export {
    #[wasm_bindgen(getter)]
    pub fn result(&self) -> String {
        self.result.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn preset(&self) -> String {
        self.preset.clone()
    }
    /// the update file; moves it out
    pub fn take_update(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.update)
    }
}

#[wasm_bindgen]
pub struct Prepared {
    meta: String,
    linear: Vec<f32>,
    camera: Option<Vec<u8>>,
}

#[wasm_bindgen]
impl Prepared {
    #[wasm_bindgen(getter)]
    pub fn meta(&self) -> String {
        self.meta.clone()
    }
    pub fn take_linear(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.linear)
    }
    /// RGBA, oriented, at the preview size; undefined without a camera JPEG
    pub fn take_camera(&mut self) -> Option<Vec<u8>> {
        self.camera.take()
    }
}

#[wasm_bindgen]
impl Editor {
    /// check, decode and self-test the update file the user dropped in
    #[wasm_bindgen(constructor)]
    pub fn new(update: Vec<u8>, filename: &str) -> Result<Editor, JsError> {
        Ok(Editor { session: Session::open(update, filename).map_err(js)?, shot: None })
    }

    pub fn firmware(&self) -> String {
        self.session.firmware().to_string()
    }

    pub fn inherit(&mut self, body: &str) -> Result<String, JsError> {
        Ok(self.session.set_inherit(&parse(body)?).map_err(js)?.to_string())
    }

    pub fn effects(&mut self, body: &str) -> Result<String, JsError> {
        Ok(self.session.set_effects(&parse(body)?).map_err(js)?.to_string())
    }

    pub fn space(&self, body: &str) -> Result<String, JsError> {
        Ok(self.session.space(&parse(body)?).map_err(js)?.to_string())
    }

    pub fn tone(&self, body: &str) -> Result<String, JsError> {
        Ok(self.session.tone(&parse(body)?).map_err(js)?.to_string())
    }

    pub fn hsl(&self, body: &str) -> Result<String, JsError> {
        Ok(self.session.hsl(&parse(body)?).map_err(js)?.to_string())
    }

    pub fn export(&self, body: &str) -> Result<Export, JsError> {
        let (r, update, preset) = self.session.export(&parse(body)?).map_err(js)?;
        Ok(Export { result: r.to_string(), update, preset })
    }

    /// decode a DNG; afterwards decode `dng_jpeg()` to RGBA at
    /// `dng_width()` x `dng_height()` and call `prepare_dng`
    pub fn open_dng(&mut self, data: &[u8]) -> Result<(), JsError> {
        self.shot = Some(dng::decode(data).map_err(js)?);
        Ok(())
    }

    /// the embedded camera JPEG of the opened DNG (copied out of `data`)
    pub fn dng_jpeg(&self, data: &[u8]) -> Option<Vec<u8>> {
        let (a, b) = self.shot.as_ref()?.jpeg?;
        data.get(a..b).map(|s| s.to_vec())
    }

    pub fn dng_width(&self) -> usize {
        self.shot.as_ref().map_or(0, |s| s.width)
    }

    pub fn dng_height(&self) -> usize {
        self.shot.as_ref().map_or(0, |s| s.height)
    }

    /// meta, linear image and camera image of the opened DNG
    pub fn prepare_dng(&mut self, camera_rgba: Option<Vec<u8>>, filename: &str) -> Result<Prepared, JsError> {
        let shot = self.shot.take().ok_or_else(|| JsError::new("no DNG opened"))?;
        let p = self.session.prepare_shot(&shot, camera_rgba.as_deref(), filename).map_err(js)?;
        Ok(Prepared { meta: p.meta.to_string(), linear: p.linear, camera: p.camera })
    }
}
