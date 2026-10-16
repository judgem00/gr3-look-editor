// The editor's back end in the browser: the Rust core (pkg/grlook_bg.wasm).
// Nothing leaves this machine.
//
// Messages: {id, op, args} -> {id, ok, result | error}. Large buffers travel
// as transferables.

import init, { Editor } from "./pkg/grlook.js";

const ready = init();
let editor = null;

// the camera JPEG, decoded and resized by the browser to the preview size
async function decodeJpeg(bytes, width, height) {
  try {
    const bmp = await createImageBitmap(new Blob([bytes], { type: "image/jpeg" }),
      { resizeWidth: width, resizeHeight: height, resizeQuality: "high" });
    const c = new OffscreenCanvas(width, height);
    const ctx = c.getContext("2d");
    ctx.drawImage(bmp, 0, 0);
    bmp.close();
    return new Uint8Array(ctx.getImageData(0, 0, width, height).data.buffer);
  } catch {
    return null;    // no usable JPEG: the preview falls back to default values
  }
}

async function encodeJpeg(rgba, width, height) {
  const c = new OffscreenCanvas(width, height);
  c.getContext("2d").putImageData(new ImageData(new Uint8ClampedArray(rgba.buffer), width, height), 0, 0);
  return c.convertToBlob({ type: "image/jpeg", quality: 0.92 });
}

const ops = {
  async open({ bytes, name }) {
    editor?.free();
    editor = null;
    editor = new Editor(new Uint8Array(bytes), name);
    return JSON.parse(editor.firmware());
  },
  firmware: () => JSON.parse(editor.firmware()),
  inherit: ({ body }) => JSON.parse(editor.inherit(body)),
  tone: ({ body }) => JSON.parse(editor.tone(body)),
  hsl: ({ body }) => JSON.parse(editor.hsl(body)),
  effects: ({ body }) => JSON.parse(editor.effects(body)),
  target: ({ body }) => JSON.parse(editor.target(body)),
  space: ({ body }) => JSON.parse(editor.space(body)),
  export({ body }) {
    const e = editor.export(body);
    const update = e.take_update();
    const out = { result: JSON.parse(e.result), preset: e.preset, update };
    e.free();
    return [out, [update.buffer]];
  },
  // a .cube: what it holds
  lut: ({ text }) => JSON.parse(editor.lut_info(text)),
  // the LUT fitter: blocks this worker for some seconds, reporting each step
  lut_fit: ({ text, body }) => JSON.parse(editor.lut_fit(text, body)),
  // GR-Log (grlog.rs): the target, and a post LUT as .cube text
  grlog: ({ body }) => JSON.parse(editor.grlog(body)),
  grlog_lut: ({ body, text }) => editor.grlog_lut(body, text || ""),
  async dng({ bytes, name }) {
    const data = new Uint8Array(bytes);
    editor.open_dng(data);
    const [w, h] = [editor.dng_width(), editor.dng_height()];
    const jpeg = editor.dng_jpeg(data);
    const rgba = jpeg ? await decodeJpeg(jpeg, w, h) : null;
    const p = editor.prepare_dng(rgba ?? undefined, name);
    const meta = JSON.parse(p.meta);
    const linear = p.take_linear();
    const cam = p.take_camera();
    p.free();
    const camera = cam ? await encodeJpeg(cam, meta.width, meta.height) : null;
    return [{ meta, linear, camera }, [linear.buffer]];
  },
  // the embedded JPEGs of a DNG as [[off, len], ...] (largest first), from
  // the IFD headers only — the filmstrip's thumbnails. No raw decode, the
  // opened shot is left untouched. Returns an object: a bare array result
  // would be read as [result, transferables] above.
  dng_preview: ({ bytes }) => ({ ranges: JSON.parse(editor.dng_previews(new Uint8Array(bytes))) }),
};

// called by the core during a LUT fit (wasm.rs): progress for the running request
let running = 0;
globalThis.lutFitProgress = (step) => postMessage({ id: running, progress: step });

onmessage = async ({ data: { id, op, args } }) => {
  running = id;
  try {
    await ready;
    if (op !== "open" && !editor) throw new Error("no firmware loaded");
    let result = await ops[op](args || {});
    let transfer = [];
    if (Array.isArray(result)) [result, transfer] = result;
    postMessage({ id, ok: true, result }, transfer);
  } catch (err) {
    postMessage({ id, ok: false, error: err && err.message ? err.message : String(err) });
  }
};
