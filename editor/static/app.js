// GR III / GR IIIx look editor — preview, matrix and tone curve editing.
//
// The preview runs the colour part of ColorMatrixTuning on the GPU:
//   camera RGB -> white balance -> sensor CCM -> look matrix -> (mono mix)
//   -> MultiAxial (HSL, hsl.js)
// followed by the camera's tone curve. That curve comes from the core (worker), which
// runs the camera's own GammaTuning code on the (edited) base curves; a small
// residual fitted from the camera JPEG stands in for its image-dependent shadow
// correction.

const $ = (id) => document.getElementById(id);
const SCALE = 8192;

const state = {
  fw: null,            // /api/firmware
  looks: {},           // name -> look from the firmware
  edits: {},           // matrix address -> { base, sat, hue }
  current: null,
  mode: "edit",
  split: 0.5,
  ev: 0,
  wb: { kelvin: false, k: 5200, ab: 0, gm: 0 },   // preview-only white balance (false: as shot)
  wbCal: null,         // [R, B] calibration of the photo's camera, null: typical
  wbShot: null,        // { k, gm } closest to the shot's white balance
  shot: null,          // /api/dng meta
  tone: { points: [[0, 0], [1, 1]], families: [0, 1, 2] },
  params: { key: 0, contrast: 0, highlight: 0, shadow: 0, saturation: 0, hue: 0, shadowcorr: 0 },
  residual: true,
  vignette: true,      // the camera's lens shading correction in the preview
  curves: null,        // last /api/tone answer
  chan: { edits: {}, set: "Preset 1", channel: 0, all: true },   // the look's channel curves (Farbton 1-3)
  extBases: [],        // extra bases from _local/bases.json (not in the repo)
  extBase: null,       // id of the active extra base
  grade: {},           // set name -> colour wheels, applied by the core after the edits
};
const TONE_KEYS = ["key", "contrast", "highlight", "shadow"];
// camera settings that act through matrices, not the tone curve: saturation
// selects A[sat] in ColorMatrixTuning, hue a Cb/Cr rotation after the curve
const COLOR_KEYS = ["saturation", "hue"];
const INHERIT_TARGET = "CrossProcess";
const INHERIT_SOURCES = ["Standard", "Vivid", "PositiveFilm", "NegativeFilm", "Retro",
  "BleechBypass_GR", "BleechBypass_GR · Var. 2", "BleechBypass_GR · Var. 3"];
// what the last export contained; a later edit makes its downloads stale
const exportKey = () => JSON.stringify([
  Object.fromEntries((state.fw ? state.fw.looks : []).filter((l) => isChanged(l.name))
    .map((l) => [l.name, effective(l.name).map((v) => Math.round(v * SCALE))])),
  toneChanged() ? state.tone : null,
  state.fw ? changedChannels().map((i) => [i, chanValues(i)]) : [],
  state.fw ? state.fw.inherit : {},
  hslForExport(),
]);
const toneChanged = () => !state.tone.points.every(([x, y]) => Math.abs(x - y) < 1e-4);

// firmware names -> the names in the camera menu
const LOOK_LABELS = {
  Standard: "Standard", Vivid: "Vivid", MonotoneGR: "Monotone", SoftMonotone: "Soft Monotone",
  HardMonotone: "Hard Monotone", HighContrastBW: "Hi-Contrast B&W", PositiveFilm: "Positive Film",
  BleechBypass_GR: "Bleach Bypass", "BleechBypass_GR · Var. 2": "Bleach Bypass (2)",
  "BleechBypass_GR · Var. 3": "Bleach Bypass (3)", Retro: "Retro", HdrCuisine: "HDR Tone",
  "HdrCuisine · Var.": "HDR Tone (2)", CrossProcess: "Cross Process", NegativeFilm: "Negative Film",
};
const label = (name) => LOOK_LABELS[name] || name;
const esc = (v) => String(v).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const store = {   // per-viewer conveniences only; the page works without storage
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* private mode */ } },
};

// ---- small 3x3 helpers, row-major arrays of 9 --------------------------------

const mul = (a, b) => {
  const r = new Array(9).fill(0);
  for (let i = 0; i < 3; i++)
    for (let j = 0; j < 3; j++)
      for (let k = 0; k < 3; k++) r[i * 3 + j] += a[i * 3 + k] * b[k * 3 + j];
  return r;
};
const ident = () => [1, 0, 0, 0, 1, 0, 0, 0, 1];
const transpose = (m) => [m[0], m[3], m[6], m[1], m[4], m[7], m[2], m[5], m[8]];
function inverse(m) {
  const [a, b, c, d, e, f, g, h, i] = m;
  const A = e * i - f * h, B = f * g - d * i, C = d * h - e * g;
  const det = a * A + b * B + c * C;
  return [A, c * h - b * i, b * f - c * e, B, a * i - c * g, c * d - a * f, C, b * g - a * h, a * e - b * d]
    .map((v) => v / det);
}
const step = (k) => Math.max(0, Math.min(8, Math.round(state.params[k] || 0) + 4));
// the camera's saturation matrix for the current look and setting
const satMatrix = (look) => (look.sat_matrices ? look.sat_matrices[step("saturation")] : ident());
// hue: YCbCr = H x YCC on the tone-mapped RGB; as an RGB matrix YCC^-1 H YCC
function hueMatrix(look) {
  if (!look.hue_matrices || step("hue") === 4) return ident();
  return mul(mul(inverse(state.fw.ycc), look.hue_matrices[step("hue")]), state.fw.ycc);
}
const same = (a, b) => a.every((v, i) => Math.round(v * SCALE) === Math.round(b[i] * SCALE));

function saturation(s) {
  // mix towards luma with the firmware's own weights; rows keep summing to 1
  const w = state.fw.luma;
  const t = w[0] + w[1] + w[2];
  const l = w.map((v) => v / t);
  const m = [];
  for (let i = 0; i < 3; i++)
    for (let j = 0; j < 3; j++) m.push((1 - s) * l[j] + (i === j ? s : 0));
  return m;
}

function hueRotation(deg) {
  // rotation about the grey axis (1,1,1): grey stays grey
  const a = (deg * Math.PI) / 180, c = Math.cos(a), s = Math.sin(a) / Math.sqrt(3);
  const k = (1 - c) / 3;
  return [c + k, k - s, k + s, k + s, c + k, k - s, k - s, k + s, c + k];
}

// ---- edit state -------------------------------------------------------------

// Lightroom-style calibration: hue (-100..100 -> +-30 deg) and saturation
// (-100..100 -> x0..x2) of the R, G, B primaries
const PRIMS = ["r", "g", "b"];
const CAL_DEG = 0.3;
const neutralCalib = () => ({ r: { hue: 0, sat: 0 }, g: { hue: 0, sat: 0 }, b: { hue: 0, sat: 0 } });
const calibNeutral = (c) => !c || PRIMS.every((p) => !c[p].hue && !c[p].sat);

// The primaries are the columns of the matrix. Each splits into a grey part
// (its luma share) and a chroma part; the chroma part is rotated about the
// grey axis and scaled. Then, as when building an RGB matrix from primaries,
// each column is scaled so that white stays white (P k = 1): every row sums
// to 1, which the camera forces anyway (ColorMatrixTuning::Calc sets the
// diagonal from the row sum), and scaling a column leaves the hue of the
// other primaries untouched -- they only change a little in brightness.
function calibMatrix(c) {
  if (calibNeutral(c)) return ident();
  const w = state.fw.luma, t = w[0] + w[1] + w[2];
  const cols = PRIMS.map((p, k) => {
    const g = w[k] / t;
    const d = [0, 1, 2].map((i) => (i === k ? 1 : 0) - g);
    const R = hueRotation(c[p].hue * CAL_DEG), f = 1 + c[p].sat / 100;
    return [0, 1, 2].map((i) => g + f * (R[i * 3] * d[0] + R[i * 3 + 1] * d[1] + R[i * 3 + 2] * d[2]));
  });
  const P = [];
  for (let i = 0; i < 3; i++) for (let k = 0; k < 3; k++) P.push(cols[k][i]);
  const Pi = inverse(P);
  const kk = [0, 1, 2].map((i) => Pi[i * 3] + Pi[i * 3 + 1] + Pi[i * 3 + 2]);
  if (kk.some((v) => !(v > 0.05))) return ident();     // degenerate: keep the look as it is
  return P.map((v, n) => v * kk[n % 3]);
}

function editOf(name) {
  const look = state.looks[name];
  if (!state.edits[look.address])
    state.edits[look.address] = { base: look.matrix.slice(), sat: 1, hue: 0, calib: neutralCalib() };
  const e = state.edits[look.address];
  if (!e.calib) e.calib = neutralCalib();
  return e;
}

function effective(name) {
  const e = editOf(name);
  return mul(mul(mul(saturation(e.sat), hueRotation(e.hue)), e.base), calibMatrix(e.calib));
}

const isChanged = (name) => !same(effective(name), state.looks[name].matrix);

// ---- WebGL ------------------------------------------------------------------

const canvas = $("view");
const gl = canvas.getContext("webgl2", { preserveDrawingBuffer: true });
let prog, tex = {};

const VS = `#version 300 es
in vec2 p; out vec2 uv;
void main() { uv = vec2(p.x * .5 + .5, .5 - p.y * .5); gl_Position = vec4(p, 0, 1); }`;

const FS = `#version 300 es
precision highp float;
in vec2 uv; out vec4 o;
uniform sampler2D uImg, uLut, uLutRef, uRes, uCam;
uniform mat3 uM, uMRef, uHue;
uniform bool uMono, uResidual, uVigOn, uChromaOn, uTcOn;
uniform sampler2D uTc;       // the camera's shadow correction: gain per 12-bit value / 4 (1024)
uniform float uChroma[16];   // the camera's chroma roll-off: factor per 16 levels of Y
uniform vec3 uVig;     // lens shading correction: 1 + a r2 + b r4 + c r6
uniform vec2 uSize;
uniform vec3 uWb;
uniform float uExp, uScale, uSplit;
uniform int uMode;   // 0 edit, 1 original|edit, 2 camera|edit, 3 camera

${HSL_GLSL}
float lut(sampler2D t, float f) {   // single-channel table (residual), 256 texels
  f = clamp(f, 0., 255.);
  int i = int(min(floor(f), 254.));
  return mix(texelFetch(t, ivec2(i, 0), 0).r, texelFetch(t, ivec2(i + 1, 0), 0).r, f - float(i));
}

// the camera's R, G, B tables (identical unless channel curves apply):
// 12-bit linear input, 256 entries, step 16, out 0..0x4000
vec3 lut3(sampler2D t, vec3 f) {
  f = clamp(f, 0., 255.);
  ivec3 i = ivec3(min(floor(f), vec3(254.)));
  vec3 u = f - vec3(i);
  return vec3(
    mix(texelFetch(t, ivec2(i.r, 0), 0).r, texelFetch(t, ivec2(i.r + 1, 0), 0).r, u.r),
    mix(texelFetch(t, ivec2(i.g, 0), 0).g, texelFetch(t, ivec2(i.g + 1, 0), 0).g, u.g),
    mix(texelFetch(t, ivec2(i.b, 0), 0).b, texelFetch(t, ivec2(i.b + 1, 0), 0).b, u.b));
}

vec3 render(mat3 m, sampler2D curve, bool ma) {
  // where the sensor clipped, white balance would invent a colour: cut all
  // channels at the level of the smallest gain, as the camera does (fit.rs)
  vec3 c = min(texture(uImg, uv).rgb * uWb, vec3(min(uWb.r, min(uWb.g, uWb.b))));
  if (uVigOn) {
    vec2 d = (uv - .5) * uSize;
    float r2 = dot(d, d) / (.25 * dot(uSize, uSize));
    c *= 1. + r2 * (uVig.x + r2 * (uVig.y + r2 * uVig.z));
  }
  c = max(m * c, 0.);
  if (uMono) c = vec3((c.r + c.g + c.b) / 3.);
  c *= uExp;
  if (ma && uMa) c = multiaxial(c);
  if (uTcOn) {   // indexed by the largest channel, see tonecontrol.rs
    vec3 v = c * uScale;
    int i = int(clamp(max(v.r, max(v.g, v.b)) / 4., 0., 1023.));
    c *= texelFetch(uTc, ivec2(i, 0), 0).r;
  }
  vec3 g = lut3(curve, c * uScale / 16.) / 16384.;
  g = uHue * g;
  if (uResidual) g = vec3(lut(uRes, clamp(g.r, 0., 1.) * 255.),
                          lut(uRes, clamp(g.g, 0., 1.) * 255.),
                          lut(uRes, clamp(g.b, 0., 1.) * 255.));
  if (uChromaOn) {
    float y = dot(clamp(g, 0., 1.), vec3(.299, .587, .114));
    float f = clamp(y * 16. - .5, 0., 15.);
    int i = int(min(floor(f), 14.));
    g = y + (g - y) * mix(uChroma[i], uChroma[i + 1], f - float(i));
  }
  return g;
}

void main() {
  vec3 cam = texture(uCam, uv).rgb;
  bool left = uv.x < uSplit;
  vec3 c;
  if (uMode == 3) c = cam;
  else if (uMode == 1 && left) c = render(uMRef, uLutRef, false);
  else if (uMode == 2 && left) c = cam;
  else c = render(uM, uLut, true);
  o = vec4(c, 1);
}`;

function compile(type, src) {
  const s = gl.createShader(type);
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
  return s;
}

function initGL() {
  if (!gl) throw new Error(t("no_webgl"));
  gl.getExtension("EXT_color_buffer_float");
  prog = gl.createProgram();
  gl.attachShader(prog, compile(gl.VERTEX_SHADER, VS));
  gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, FS));
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog));
  gl.useProgram(prog);
  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog, "p");
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
}

function texture(unit, setup) {
  const t = gl.createTexture();
  gl.activeTexture(gl.TEXTURE0 + unit);
  gl.bindTexture(gl.TEXTURE_2D, t);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
  setup();
  return t;
}

const uni = (n) => gl.getUniformLocation(prog, n);

function draw() {
  if (!state.shot || !tex.lut) return;
  const look = state.looks[state.current];
  const sensor = state.fw.sensor_ccm;
  const A = satMatrix(look);
  const m = mul(mul(A, effective(state.current)), sensor);
  const mRef = mul(mul(A, look.matrix), sensor);
  gl.viewport(0, 0, canvas.width, canvas.height);
  gl.uniform1i(uni("uImg"), 0);
  gl.uniform1i(uni("uLut"), 1);
  gl.uniform1i(uni("uCam"), 2);
  gl.uniform1i(uni("uLutRef"), 3);
  gl.uniform1i(uni("uRes"), 4);
  gl.uniformMatrix3fv(uni("uM"), false, transpose(m));
  gl.uniformMatrix3fv(uni("uMRef"), false, transpose(mRef));
  gl.uniformMatrix3fv(uni("uHue"), false, transpose(hueMatrix(look)));
  gl.uniform1i(uni("uMono"), look.mono_mix ? 1 : 0);
  gl.uniform1i(uni("uResidual"), state.residual && state.shot.residual ? 1 : 0);
  // part of the fine match: fitted on the camera JPEG like the residual
  gl.uniform1i(uni("uChromaOn"), state.residual && state.shot.highlight_chroma ? 1 : 0);
  if (state.shot.highlight_chroma) gl.uniform1fv(uni("uChroma"), state.shot.highlight_chroma);
  const vig = state.shot.vignette;
  gl.uniform1i(uni("uVigOn"), state.vignette && vig ? 1 : 0);
  gl.uniform3fv(uni("uVig"), vig ? vig.coef : [0, 0, 0]);
  gl.uniform2fv(uni("uSize"), [state.shot.width, state.shot.height]);
  gl.uniform3fv(uni("uWb"), wbGains());
  gl.uniform1f(uni("uExp"), Math.pow(2, state.ev));
  gl.uniform1f(uni("uScale"), state.shot.scale);
  gl.uniform1f(uni("uSplit"), state.mode === "edit" || state.mode === "jpeg" ? -1 : state.split);
  gl.uniform1i(uni("uMode"), { edit: 0, ab: 1, camera: 2, jpeg: 3 }[state.mode]);
  hslUniforms();
  gl.uniform1i(uni("uTc"), 5);
  gl.uniform1i(uni("uTcOn"), tex.tc && state.params.shadowcorr > 0 ? 1 : 0);
  gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  placeSplit();
}

// ---- split handle -----------------------------------------------------------

function imageRect() {
  const r = canvas.getBoundingClientRect(), w = $("wrap").getBoundingClientRect();
  const s = Math.min(r.width / canvas.width, r.height / canvas.height);
  const dw = canvas.width * s, dh = canvas.height * s;
  return { x: r.left - w.left + (r.width - dw) / 2, y: r.top - w.top + (r.height - dh) / 2, w: dw, h: dh };
}

function placeSplit() {
  const show = state.shot && (state.mode === "ab" || state.mode === "camera");
  $("split").hidden = !show;
  $("labels").hidden = !show;
  if (!show) return;
  const r = imageRect();
  $("split").style.left = `${r.x + state.split * r.w}px`;
  $("split").style.top = `${r.y}px`;
  $("split").style.bottom = "auto";
  $("split").style.height = `${r.h}px`;
  $("labels").style.left = `${r.x + 8}px`;
  $("labels").style.right = "auto";
  $("labels").style.width = `${r.w - 16}px`;
  $("labels").style.top = `${r.y + 8}px`;
  $("label-l").textContent = t(state.mode !== "ab" ? "label_camera" : state.current === INHERIT_TARGET ? "label_base" : "label_original");
  $("label-r").textContent = t(state.current === INHERIT_TARGET ? "label_look" : "label_edited");
}

let dragging = false;
$("split").addEventListener("pointerdown", (e) => { dragging = true; e.target.setPointerCapture(e.pointerId); });
window.addEventListener("pointerup", () => { dragging = false; });
window.addEventListener("pointermove", (e) => {
  if (!dragging) return;
  const r = imageRect(), w = $("wrap").getBoundingClientRect();
  state.split = Math.min(1, Math.max(0, (e.clientX - w.left - r.x) / r.w));
  draw();
});
window.addEventListener("resize", placeSplit);

// ---- UI ---------------------------------------------------------------------

function renderLooks() {
  const sel = $("look-select");
  sel.innerHTML = "";
  for (const look of state.fw.looks) {
    const o = document.createElement("option");
    o.value = look.name;
    o.textContent = label(look.name) +
      (isChanged(look.name) ? t("changed_mark") : "") +
      (state.shot && state.shot.shot_look === look.name ? t("photo_look") : "");
    sel.appendChild(o);
  }
  sel.value = state.current;
}
$("look-select").onchange = (e) => select(e.target.value);

function renderMatrix() {
  const look = state.looks[state.current];
  const m = effective(state.current);
  const box = $("matrix");
  box.innerHTML = "";
  m.forEach((v, i) => {
    const inp = document.createElement("input");
    inp.type = "number";
    inp.step = "0.01";
    inp.value = v.toFixed(4);
    if (Math.round(v * SCALE) !== Math.round(look.matrix[i] * SCALE)) inp.classList.add("diff");
    inp.onchange = () => {
      const val = parseFloat(inp.value);
      if (Number.isNaN(val)) return renderMatrix();
      const cur = effective(state.current);
      cur[i] = val;
      Object.assign(editOf(state.current), { base: cur, sat: 1, hue: 0, calib: neutralCalib() });
      update();
    };
    box.appendChild(inp);
    if (i % 3 === 2) box.appendChild(document.createElement("span"));
  });
  const sums = [0, 1, 2].map((r) => m[r * 3] + m[r * 3 + 1] + m[r * 3 + 2]);
  $("sums").innerHTML = t("row_sums") + sums
    .map((s) => `<span class="${Math.abs(s - 1) > 0.001 ? "bad" : ""}">${s.toFixed(4)}</span>`)
    .join(" · ") + (sums.some((s) => Math.abs(s - 1) > 0.001) ? ` <span class="bad">${t("cast")}</span>` : "");
  $("addr").textContent = `0x${look.address.toString(16)}`;
  const note = $("shared-note");
  note.hidden = !look.shared_with.length;
  note.className = "note warn";
  note.textContent = t("shared", { look: label(look.name), others: look.shared_with.map(label).join(t("and")) });
  const rows = [0, 1, 2].map((r) => m.slice(r * 3, r * 3 + 3).map((v) => v.toFixed(4)).join());
  if (!look.mono_mix && rows[0] === rows[1] && rows[1] === rows[2]) {
    note.hidden = false;
    note.className = "note";
    note.textContent = t("mono_filter");
  }
  if (look.mono_mix) {
    note.hidden = false;
    note.className = "note";
    note.textContent = t("mono_mix");
  }
  renderInherit();
  const e = editOf(state.current);
  $("sat").value = e.sat; $("sat-out").textContent = e.sat.toFixed(2);
  $("hue").value = e.hue; $("hue-out").textContent = `${e.hue.toFixed(1)}°`;
  for (const p of PRIMS) for (const k of ["hue", "sat"]) {
    const el = calibUI[p + k];
    if (!el) continue;
    el.input.value = e.calib[p][k];
    el.out.textContent = e.calib[p][k] > 0 ? `+${e.calib[p][k]}` : `${e.calib[p][k]}`;
  }
}

// ---- 1 Grundlage: what the user's look (in the Cross Process slot) starts from

const CROSS = INHERIT_TARGET;
const BASES = [null, "Vivid", "PositiveFilm", "NegativeFilm", "Retro",
  "BleechBypass_GR", "BleechBypass_GR · Var. 2", "BleechBypass_GR · Var. 3"];
const BASE_HINTS = {
  null: "hint_standard", Vivid: "hint_vivid", PositiveFilm: "hint_positive", NegativeFilm: "hint_negative",
  Retro: "hint_retro", BleechBypass_GR: "hint_bleach",
  "BleechBypass_GR · Var. 2": "hint_bleach_var", "BleechBypass_GR · Var. 3": "hint_bleach_var",
};
const baseOf = () => (state.fw.inherit || {})[CROSS] || null;

function renderInherit() {
  const ul = $("base-list");
  const cur = baseOf();
  const ext = extBaseOf();
  ul.innerHTML = "";
  for (const b of BASES) {
    const li = document.createElement("li");
    li.className = b === cur && !ext ? "on" : "";
    li.innerHTML = `<b>${b ? label(b) : t("base_standard")}</b><span>${t(BASE_HINTS[b])}</span>`;
    li.onclick = () => setBase(b);
    ul.appendChild(li);
  }
  for (const x of state.extBases) {
    const li = document.createElement("li");
    li.className = ext === x ? "on" : "";
    li.innerHTML = `<b>${esc(extText(x.label))}</b><span>${esc(extText(x.hint))}</span>`;
    li.onclick = () => setExtBase(x);
    ul.appendChild(li);
  }
  const src = cur;
  const p = src && state.shot && state.shot.shot_look === src && state.shot.params;
  $("inherit-note").textContent = ext ? extText(ext.tip)
    : src
    ? t("base_tip", { look: label(src) }) +
      (p ? t("base_tip_photo", { contrast: signed(p.contrast || 0), highlight: signed(p.highlight || 0),
                                 shadow: signed(p.shadow || 0), saturation: signed(p.saturation || 0) }) : "") + "."
    : "";
}

// switch the look data of the Cross Process slot to `source` (null: Standard)
async function applyInherit(source) {
  if (source === baseOf()) return true;
  const res = await api("/api/inherit", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ target: CROSS, source }),
  });
  if (!res.ok) return false;
  const fw = await res.json();
  Object.assign(state.fw, { looks: fw.looks, inherit: fw.inherit });
  for (const l of fw.looks) state.looks[l.name] = l;
  return true;
}

async function setBase(source) {
  const hadExt = !!extBaseOf();
  if (source === baseOf() && !hadExt) return;
  if (hadExt) clearExtBase();
  if (!(await applyInherit(source))) { renderInherit(); return; }
  // the look keeps its own matrix slot; start it from the new base, keep the sliders
  const e = state.edits[state.looks[CROSS].address];
  if (e) e.base = state.looks[CROSS].matrix.slice();
  renderInherit();
  update();
  if (hadExt) renderChannelPanel();
  requestTone();
}

// ---- extra bases ----------------------------------------------------------------
// Bases that are not image controls of the GR III / GR IIIx firmware.
// Each: { id, label, hint, tip: {en, de}, matrix_from: look name, matrix_mul: [9],
//         channel_curve: [256] }. The matrix is matrix_mul x the look's matrix of the
// loaded firmware; the curve goes on R, G and B of Color Tone 1-3. An optional
// _local/bases.json (not in the repo) adds bases or replaces one with the same id.

// Extra bases built into the editor: a colour matrix relative to Standard and a
// tone curve for Color Tone 1-3.
const BUILTIN_BASES = [{
  id: "positive-film-classic",
  label: {"en": "Positive Film (GR 1)", "de": "Positive Film (GR 1)"},
  hint: {"en": "rich red, muted green and yellow, deep blacks", "de": "kräftiges Rot, gedämpftes Grün und Gelb, tiefe Schwärzen"},
  tip: {"en": "Colour as a matrix, tone as curves on Color Tone 1-3. In the camera: Cross Process, controls at 0.", "de": "Farbe als Matrix, Ton als Kurven auf Farbton 1–3. In der Kamera: Cross Process, Regler auf 0."},
  matrix_from: "Standard",
  matrix_mul: [1.040811742340139, 0.20596874929707515, -0.19377299757717903, 0.04734633241530474, 0.9336765395681852, -0.01931811901750858, -0.06289532853274803, 0.3269147342454318, 0.7756132715716554],
  channel_curve: [
    0, 0, 0, 0, 21, 42, 64, 85, 107, 128, 159, 191, 223, 255, 297, 340,
    383, 430, 478, 526, 574, 608, 640, 672, 704, 750, 798, 846, 894, 956, 1020, 1084,
    1134, 1182, 1230, 1279, 1340, 1404, 1468, 1532, 1596, 1660, 1724, 1788, 1852, 1916, 1980, 2044,
    2134, 2231, 2301, 2365, 2429, 2518, 2614, 2685, 2749, 2813, 2902, 2998, 3094, 3190, 3310, 3438,
    3518, 3582, 3646, 3733, 3829, 3925, 4021, 4117, 4213, 4309, 4405, 4501, 4597, 4693, 4789, 4885,
    4981, 5056, 5120, 5206, 5302, 5398, 5494, 5653, 5782, 5878, 5974, 6070, 6145, 6209, 6355, 6465,
    6529, 6594, 6658, 6801, 6914, 6978, 7042, 7106, 7170, 7234, 7298, 7400, 7490, 7554, 7655, 7728,
    7760, 7846, 7938, 8002, 8066, 8130, 8194, 8258, 8322, 8386, 8484, 8562, 8594, 8643, 8706, 8803,
    8882, 8915, 8963, 9058, 9154, 9218, 9267, 9299, 9347, 9411, 9475, 9568, 9666, 9731, 9766, 9795,
    9859, 9986, 10023, 10050, 10114, 10178, 10242, 10280, 10370, 10434, 10498, 10537, 10562, 10626, 10690, 10794,
    10818, 10882, 10946, 10986, 11051, 11074, 11138, 11202, 11266, 11330, 11372, 11436, 11458, 11522, 11586, 11650,
    11693, 11757, 11822, 11842, 11905, 11950, 12014, 12078, 12143, 12161, 12225, 12289, 12336, 12353, 12417, 12464,
    12528, 12592, 12657, 12673, 12721, 12785, 12849, 12865, 12928, 12992, 13042, 13106, 13171, 13184, 13235, 13299,
    13363, 13428, 13492, 13504, 13556, 13620, 13685, 13749, 13813, 13877, 13942, 13952, 14016, 14070, 14134, 14199,
    14263, 14327, 14391, 14456, 14520, 14528, 14592, 14656, 14713, 14777, 14841, 14905, 14970, 15034, 15098, 15162,
    15167, 15231, 15295, 15359, 15423, 15487, 15551, 15615, 15679, 15743, 15807, 15933, 16000, 16064, 16190, 16383,
  ],
}];

const extText = (v) => (v && typeof v === "object" ? v[LANG] || v.en || "" : v || "");
const extBaseOf = () => state.extBases.find((x) => x.id === state.extBase) || null;
const extMatrix = (x) => mul(x.matrix_mul, state.looks[x.matrix_from].matrix);
const sameCurve = (a, b) => a && b && a.length === b.length && a.every((v, k) => v === b[k]);

async function loadExtBases() {
  let local = [];
  try {
    const res = await fetch("_local/bases.json", { cache: "no-store" });
    if (res.ok) local = await res.json();
  } catch { /* no local bases */ }
  const curve = (v) => Array.isArray(v) && v.length === 256 && v.every(Number.isFinite);
  const valid = (x) => x && typeof x.id === "string" &&
    state.looks[x.matrix_from] && Array.isArray(x.matrix_mul) && x.matrix_mul.length === 9 &&
    x.matrix_mul.every(Number.isFinite) && (x.channel_curve == null || curve(x.channel_curve));
  const byId = new Map(BUILTIN_BASES.filter(valid).map((x) => [x.id, x]));
  for (const x of (Array.isArray(local) ? local : []).filter(valid)) byId.set(x.id, x);
  return [...byId.values()];
}

async function setExtBase(x) {
  if (extBaseOf() === x) return;
  if (extBaseOf()) clearExtBase();
  // Cross Process keeps the Standard look data; the base only sets matrix and curves
  if (!(await applyInherit(null))) { renderInherit(); return; }
  editOf(CROSS).base = extMatrix(x);
  if (x.channel_curve) {
    for (const n of SET_ORDER) for (const i of chanSet(n))
      state.chan.edits[i] = { base: x.channel_curve.map((v) => Math.round(v)), points: IDENT.map((p) => p.slice()) };
    relink();
  }
  state.extBase = x.id;
  renderInherit();
  update();
  renderChannelPanel();
  requestTone();
}

// leave an extra base: its matrix goes, its curves go where they are still untouched
function clearExtBase() {
  const x = extBaseOf();
  state.extBase = null;
  if (!x) return;
  const e = state.edits[state.looks[CROSS].address];
  if (e) e.base = state.looks[CROSS].matrix.slice();
  if (!x.channel_curve) return;
  for (const n of SET_ORDER) for (const i of chanSet(n)) {
    const c = state.chan.edits[i];
    if (c && sameCurve(c.base, x.channel_curve.map((v) => Math.round(v))) && isIdent(c.points)) delete state.chan.edits[i];
  }
  relink();
}

// after loading a preset: was it made from an extra base?
function detectExtBase() {
  state.extBase = null;
  if (baseOf() !== null) return;
  const e = state.edits[state.looks[CROSS].address];
  if (!e) return;
  for (const x of state.extBases) {
    const m = extMatrix(x);
    if (m.every((v, k) => Math.abs(v - e.base[k]) < 1e-6)) { state.extBase = x.id; return; }
  }
}

function renderChanges() {
  if (!$("downloads").hidden && state.lastExport !== exportKey()) {
    $("downloads").hidden = true;
    $("export").classList.add("primary");
    $("export-msg").textContent = "";
  }
  const others = state.fw.looks.map((l) => l.name).filter((n) => n !== CROSS && isChanged(n));
  const sets = new Set(changedChannels().map(chanSetOf));
  const ext = extBaseOf();
  const names = [t("ch_base", { look: ext ? extText(ext.label) : baseOf() ? label(baseOf()) : t("base_standard") })];
  if (isChanged(CROSS)) names.push(t("ch_colors"));
  for (const n of SET_ORDER) names.push(`${setLabel(n)}: ` + (sets.has(n)
    ? t(gradedSets().includes(n) ? "ch_curves_wheels" : "ch_curves")
    : t("ch_neutral")));
  for (const n of others) names.push(`<span class="warn">${t("ch_other", { look: label(n) })}</span>`);
  for (const n of hslChangedLooks())
    names.push(n === CROSS ? t("ch_hsl") : `<span class="warn">${t("ch_hsl_other", { look: label(n) })}</span>`);
  if (toneChanged()) names.push(`<span class="warn">${t("ch_tone")}</span>`);
  $("changes").innerHTML = names.map((n) => `<li>${n}</li>`).join("");
  const badge = { base: !!baseOf() || !!ext, color: isChanged(CROSS) || others.length > 0 || hslChangedLooks().length > 0,
    curves: sets.size > 0 || toneChanged() };
  for (const b of document.querySelectorAll("#tabs button")) b.classList.toggle("dirty", !!badge[b.dataset.tab]);
}
const chanSetOf = (idx) => Object.keys(state.fw.channel_sets).find((n) => state.fw.channel_sets[n].includes(idx));

// ---- tabs -------------------------------------------------------------------

function showTab(tab) {
  for (const b of document.querySelectorAll("#tabs button")) b.classList.toggle("on", b.dataset.tab === tab);
  for (const s of document.querySelectorAll("section.tab")) s.hidden = s.dataset.tab !== tab;
  store.set("tab", tab);
  document.querySelector("aside").dataset.tab = tab;
  // base and curves belong to the user's look: always preview it there
  if ((tab === "base" || tab === "curves") && state.current !== CROSS) select(CROSS);
  if (tab === "curves") { chanEditor.draw(); drawChannels(); renderWheels(); toneEditor.draw(); drawEffective(); }
}
for (const b of document.querySelectorAll("#tabs button")) b.onclick = () => showTab(b.dataset.tab);

function renderOtherLook() {
  const el = $("other-look");
  el.hidden = state.current === CROSS;
  if (el.hidden) return;
  el.innerHTML = t("other_look", { look: label(state.current) }) +
    `<a href="#" id="back-to-look">${t("back_to_look")}</a>`;
  $("back-to-look").onclick = (e) => { e.preventDefault(); select(CROSS); };
}

function update() {
  renderMatrix();
  renderLooks();
  renderOtherLook();
  renderChanges();
  draw();
}

function select(name) {
  const wasCross = state.current === CROSS;
  state.current = name;
  renderHsl();
  requestHsl();
  update();
  if (wasCross !== (name === CROSS)) requestTone();
  else renderChannelPanel();
}

for (const b of document.querySelectorAll("#modes button")) {
  b.onclick = () => {
    if (b.disabled) return;
    state.mode = b.dataset.mode;
    document.querySelectorAll("#modes button").forEach((x) => x.classList.toggle("on", x === b));
    draw();
  };
}

const calibUI = {};
(function buildCalib() {
  const box = $("calib");
  const names = { r: ["red", "#e0645a"], g: ["green", "#6fbf73"], b: ["blue", "#5b8fe0"] };
  PRIMS.forEach((p, n) => {
    if (n) box.insertAdjacentHTML("beforeend", '<span class="gap"></span>');
    for (const [k, key] of [["hue", "hue"], ["sat", "saturation"]]) {
      const head = document.createElement("span");
      head.innerHTML = k === "hue"
        ? `<span class="prim" style="color:${names[p][1]}" data-i18n="${names[p][0]}"></span> <span class="lbl" data-i18n="${key}"></span>`
        : `<span class="lbl" data-i18n="${key}"></span>`;
      const input = Object.assign(document.createElement("input"), { type: "range", min: -100, max: 100, step: 1, value: 0 });
      const out = document.createElement("output");
      out.textContent = "0";
      input.oninput = () => { editOf(state.current).calib[p][k] = +input.value; update(); };
      input.ondblclick = () => { editOf(state.current).calib[p][k] = 0; update(); };
      box.append(head, input, out);
      calibUI[p + k] = { input, out };
    }
  });
})();
$("sat").oninput = (e) => { editOf(state.current).sat = parseFloat(e.target.value); update(); };
$("hue").oninput = (e) => { editOf(state.current).hue = parseFloat(e.target.value); update(); };
for (const k of TONE_KEYS) {
  $(`p-${k}`).oninput = (e) => {
    state.params[k] = parseInt(e.target.value, 10);
    $(`p-${k}-out`).textContent = signed(state.params[k]);
    requestTone();
  };
}
const signed = (v) => (v > 0 ? `+${v}` : `${v}`);
for (const k of COLOR_KEYS) {
  $(`p-${k}`).oninput = (e) => {
    state.params[k] = parseInt(e.target.value, 10);
    $(`p-${k}-out`).textContent = signed(state.params[k]);
    draw();
  };
}
$("residual").onchange = (e) => { state.residual = e.target.checked; draw(); };
// the camera's shadow correction table (tonecontrol.rs): 1 + S (base - 1) on
// the first 256 of 1024 entries, S = 0.25 / 0.5 / 1 for Low / Medium / Strong
function setShadowCorr(level) {
  state.params.shadowcorr = level;
  $("p-shadowcorr").value = level;
  const base = state.fw && state.fw.tone_control_base;
  if (!base || !gl) return;
  const s = [0, 256, 512, 1024][level] || 0;
  const t = new Float32Array(1024).fill(1);
  base.forEach((b, i) => { t[i] = (4096 + Math.trunc(s * (b - 4096) / 1024)) / 4096; });
  if (tex.tc) gl.deleteTexture(tex.tc);
  tex.tc = texture(5, () => gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, 1024, 1, 0, gl.RED, gl.FLOAT, t));
}
$("p-shadowcorr").onchange = (e) => { setShadowCorr(+e.target.value); draw(); };
$("vignette").onchange = (e) => { state.vignette = e.target.checked; draw(); };
for (const cb of document.querySelectorAll(".fam")) {
  cb.onchange = () => {
    state.tone.families = [...document.querySelectorAll(".fam:checked")].map((x) => +x.value);
    requestTone();
  };
}
$("tone-reset").onclick = () => {
  state.tone.points = [[0, 0], [1, 1]];
  toneEditor.deselect();
  requestTone();
  renderChanges();
};
$("ev").oninput = (e) => {
  state.ev = parseFloat(e.target.value);
  $("ev-out").textContent = `${state.ev >= 0 ? "+" : ""}${state.ev.toFixed(1)} EV`;
  draw();
};
// preview-only white balance. "As shot" uses the photo's gains; otherwise they
// come from colour temperature and fine adjustment, computed like the camera
// (wb.js). They multiply the camera RGB before the colour matrices, which is
// where the camera applies white balance too.
const wbCal = () => state.wbCal || WB_DEFAULT_CAL;
function wbGains() {
  if (!state.wb.kelvin) return state.shot.wb;
  return wbGainsKelvin(state.wb.k, state.wb.ab, state.wb.gm, wbCal());
}
const fineLabel = (v, neg, pos) => (v > 0 ? `${pos}${v}` : v < 0 ? `${neg}${-v}` : "0");
function renderWb() {
  const w = state.wb;
  $("wb-k").value = w.k; $("wb-ab").value = w.ab; $("wb-gm").value = w.gm;
  $("wb-k-out").textContent = w.kelvin ? `${w.k} K` : t("wb_as_shot");
  $("wb-ab-out").textContent = fineLabel(w.ab, "B", "A");
  $("wb-gm-out").textContent = fineLabel(w.gm, "G", "M");
  const s = state.wbShot;
  $("wb-note").textContent = !state.shot ? ""
    : (s ? t("wb_note", { k: s.k, gm: fineLabel(s.gm, "G", "M") }) : "") +
      t(state.wbCal ? "wb_cal_photo" : "wb_cal_default");
}
function setWb(key, value) {
  if (!state.wb.kelvin && state.wbShot) Object.assign(state.wb, { k: state.wbShot.k, ab: 0, gm: state.wbShot.gm });
  state.wb.kelvin = true;
  state.wb[key] = value;
  renderWb();
  draw();
}
$("wb-k").oninput = (e) => setWb("k", parseInt(e.target.value, 10));
$("wb-ab").oninput = (e) => setWb("ab", parseInt(e.target.value, 10));
$("wb-gm").oninput = (e) => setWb("gm", parseInt(e.target.value, 10));
$("wb-reset").onclick = () => { wbAsShot(); draw(); };
function wbAsShot() {
  const s = state.wbShot || { k: 5200, gm: 0 };
  state.wb = { kelvin: false, k: s.k, ab: 0, gm: s.gm };
  renderWb();
}
$("reset").onclick = () => {
  const look = state.looks[state.current];
  state.edits[look.address] = { base: look.matrix.slice(), sat: 1, hue: 0, calib: neutralCalib() };
  update();
};

$("risk-ok").onchange = e => { $("export").disabled = !e.target.checked; };

$("export").onclick = async () => {
  const matrices = {};
  for (const l of state.fw.looks) if (isChanged(l.name)) matrices[l.name] = effective(l.name);
  $("export-msg").className = "note";
  $("export-msg").textContent = t("creating");
  const res = await api("/api/export", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      matrices,
      tone: toneChanged() ? state.tone : null,
      channels: Object.fromEntries(changedChannels().map((i) => [i, chanValues(i)])),
      // the points behind those curves, so a loaded preset can be edited again
      channel_edits: Object.fromEntries(changedChannels().filter((i) => state.chan.edits[i])
        .map((i) => [i, { base: state.chan.edits[i].base, points: state.chan.edits[i].points }])),
      // slider positions; the base only when typed in by hand, since otherwise it is
      // the firmware's own matrix and comes from there again on load
      matrix_edits: Object.fromEntries(state.fw.looks.filter((l) => isChanged(l.name))
        .map((l) => {
          const e = editOf(l.name);
          const own = e.base.every((v, i) => Math.abs(v - l.matrix[i]) < 1e-9);
          return [l.name, { ...e, base: own ? null : e.base }];
        })),
      channels_pre: gradedSets().length ? Object.fromEntries(changedChannels().map((i) => [i, chanPre(i)])) : null,
      grade: gradedSets().length ? Object.fromEntries(gradedSets().map((n) => [n, state.grade[n]])) : null,
      hsl: hslForExport(),
    }),
  });
  if (res.ok) {
    const r = await res.json();
    $("export-msg").className = "note ok";
    $("export-msg").textContent = t("done_web");
    state.lastExport = exportKey();
    offerDownloads(r.downloads || {}, r.update_file);
  } else {
    $("export-msg").className = "note warn";
    $("export-msg").textContent = await errorText(res);
  }
};

function offerDownloads(urls, updateFile) {
  const set = (id, url, name) => {
    const a = $(id);
    a.hidden = !url;
    if (url) { a.href = url; a.download = name || ""; }
  };
  set("dl-update", urls.update, updateFile && updateFile.split("/").pop());
  set("dl-preset", urls.preset, t("preset_name"));
  $("downloads").hidden = !urls.update && !urls.preset;
  $("export").classList.toggle("primary", $("downloads").hidden);
  if (urls.update) $("dl-update").focus();
}

async function errorText(res) {
  const body = await res.text();
  const m = body.match(/<p>(.*?)<\/p>/s);
  return m ? m[1] : t("error", { status: res.status });
}

// ---- loading a DNG ----------------------------------------------------------

async function loadDng(file) {
  $("busy").hidden = false;
  try {
    // the camera's colour temperature calibration from the makernote (near the start)
    let kwb = parseKelvinWB(await file.slice(0, 4 << 20).arrayBuffer());
    if (!kwb && file.size > 4 << 20) kwb = parseKelvinWB(await file.arrayBuffer());
    state.wbCal = kwb ? wbCalibration(kwb) : null;
    const fd = new FormData();
    fd.append("file", file);
    const res = await api("/api/dng", { method: "POST", body: fd });
    if (!res.ok) throw new Error(await errorText(res));
    await show(await res.json());
  } catch (err) {
    alert(t("load_failed", { msg: err.message }));
  } finally {
    $("busy").hidden = true;
  }
}

// preview settings: defaults (false) or the shot's values from the DNG (true)
function applyShotParams(fromDng) {
  const meta = state.shot;
  const p = (fromDng && meta && meta.params) || {};
  for (const k of [...TONE_KEYS, ...COLOR_KEYS]) {
    // Cross Process: the DNG's saturation value does not act like A[sat]
    // in measurements (MR024511/12 fit better without), so it stays at 0
    state.params[k] = k === "saturation" && meta && meta.shot_look === "CrossProcess" ? 0 : p[k] || 0;
    $(`p-${k}`).value = state.params[k];
    $(`p-${k}-out`).textContent = signed(state.params[k]);
  }
  // the shadow correction level (makernote 1..3; 4 = Auto in the menu, off in the firmware)
  const sc = fromDng && meta ? meta.shadow_correction : 0;
  setShadowCorr([1, 2, 3].includes(sc) ? sc : 0);
  state.paramsFromDng = fromDng;
  if (fromDng && meta.shot_look === "CrossProcess" && p.cross_variant) {
    state.chan.set = `Preset ${p.cross_variant}`;
    renderWheels();
    renderChannelPanel();
  }
  $("load-shot").disabled = !meta || !(Object.keys(meta.params || {}).length || meta.shot_look === "CrossProcess");
  renderToneNote();
}
$("load-shot").onclick = async () => {
  if (!state.shot) return;
  applyShotParams(true);
  await fetchTone();
  update();
};

async function show(meta) {
  const lin = new Float32Array(await (await api("/api/dng/linear")).arrayBuffer());

  canvas.width = meta.width;
  canvas.height = meta.height;
  tex.img = texture(0, () =>
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGB32F, meta.width, meta.height, 0, gl.RGB, gl.FLOAT, lin));
  const res = new Float32Array(meta.residual || Array.from({ length: 256 }, (_, i) => i / 255));
  tex.res = texture(4, () =>
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, 256, 1, 0, gl.RED, gl.FLOAT, res));

  let cam = null;
  const camRes = await api("/api/dng/camera.jpg");
  if (camRes.ok) cam = await createImageBitmap(await camRes.blob());
  tex.cam = texture(2, () => {
    if (cam) gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, cam);
    else gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(4));
  });
  for (const b of document.querySelectorAll('#modes [data-mode="camera"], #modes [data-mode="jpeg"]'))
    b.disabled = !cam;
  if (!cam && (state.mode === "camera" || state.mode === "jpeg")) $("modes").querySelector("button").click();

  state.shot = meta;
  state.wbShot = wbEstimate(meta.wb, wbCal());   // a new photo starts from its own white balance
  wbAsShot();
  $("empty").hidden = true;
  $("shot-info").textContent = [meta.filename, meta.shot_look ? label(meta.shot_look) : meta.image_tone]
    .filter(Boolean).join(" · ");
  // a new photo starts with the preview settings at their defaults; only the
  // white balance comes from the DNG ("Load preview settings from DNG" takes the rest)
  applyShotParams(false);
  $("residual").disabled = !meta.residual;
  renderVignetteNote();
  await fetchTone();
  update();
}

// ---- tone curve and channel curves --------------------------------------------

const IDENT = [[0, 0], [1, 1]];
const isIdent = (pts) => pts.every(([x, y]) => Math.abs(x - y) < 1e-4);
const CH_NAMES = ["R", "G", "B"];
const CH_COLORS = ["#e0645a", "#6fbf73", "#5b8fe0"];

const chanSet = (name) => state.fw.channel_sets[name || state.chan.set];
// internal set names stay (presets store them); the camera menu calls them Farbton 1-3
const SET_LABEL = { "Preset 1": "set_1", "Preset 2": "set_2", "Preset 3": "set_3" };
const setLabel = (name) => (SET_LABEL[name] ? t(SET_LABEL[name]) : name);
// "Zufall" cannot be chosen in the camera menu, so the editor leaves it out
const SET_ORDER = ["Preset 1", "Preset 2", "Preset 3"];
// state.chan.all: one curve on R, G and B together (a tone curve for the look);
// the three channel edits then share one points array
const chanActive = () => chanSet()[state.chan.all ? 0 : state.chan.channel];
const samePoints = (a, b) => a.length === b.length && a.every((p, i) => p[0] === b[i][0] && p[1] === b[i][1]);
function linkSet(name, force) {
  const set = chanSet(name);
  const pts = set.map((i) => chanEdit(i).points);
  if (!force && !pts.every((q) => samePoints(q, pts[0]))) return false;
  for (const i of set) chanEdit(i).points = pts[0];
  return true;
}
function unlinkSet(name) {
  for (const i of chanSet(name)) chanEdit(i).points = chanEdit(i).points.map((p) => p.slice());
}
const relink = () => { if (state.chan.all) for (const n of SET_ORDER) linkSet(n); };
function chanEdit(idx) {
  if (!state.chan.edits[idx]) state.chan.edits[idx] = { base: null, points: IDENT.map((p) => p.slice()) };
  return state.chan.edits[idx];
}
const chanValues = (idx) => (state.curves && state.curves.channel_curves
  ? state.curves.channel_curves[idx] : state.fw.channel_curves[idx]);
const WHEELS = [["shadows", "shadows"], ["midtones", "midtones"], ["highlights", "highlights"], ["global", "global"]];
const wheelNeutral = (w) => !w || (Math.abs(w.amount || 0) < 1e-4 && Math.abs(w.lum || 0) < 1e-4);
const gradeNeutral = (g) => !g || WHEELS.every(([k]) => wheelNeutral(g[k]));
const gradedSets = () => Object.keys(state.grade).filter((n) => !gradeNeutral(state.grade[n]));
const chanGraded = (idx) => gradedSets().some((n) => chanSet(n).includes(idx));
const chanPre = (idx) => (state.curves && state.curves.channel_curves_pre
  ? state.curves.channel_curves_pre[idx] : chanValues(idx));
const chanChanged = (idx) => {
  if (chanGraded(idx)) return true;
  const e = state.chan.edits[idx];
  if (!e) return false;
  if (e.base && e.base.some((v, k) => v !== state.fw.channel_curves[idx][k])) return true;
  return !isIdent(e.points);
};
const changedChannels = () => [...Array(12).keys()].filter(chanChanged);
const chanLabel = (idx) => {
  for (const [name, set] of Object.entries(state.fw.channel_sets)) {
    const k = set.indexOf(idx);
    if (k >= 0) return `${setLabel(name)} · ${CH_NAMES[k]}`;
  }
  return t("curve_n", { n: idx });
};
// the set the preview applies: only Cross Process uses channel curves
const previewSet = () => (state.current === "CrossProcess" ? chanSet() : null);

let toneBusy = false, tonePending = false;
function requestTone() {
  toneEditor.draw();
  chanEditor.draw();
  if (toneBusy) { tonePending = true; return; }
  toneBusy = true;
  fetchTone().finally(() => {
    toneBusy = false;
    if (tonePending) { tonePending = false; requestTone(); }
  });
}

async function fetchTone() {
  const edits = {};
  for (const [idx, e] of Object.entries(state.chan.edits)) edits[idx] = { base: e.base, points: e.points };
  const res = await api("/api/tone", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      params: state.params, gain: state.shot ? state.shot.gain : 0x1800,
      edit: { points: state.tone.points, families: state.tone.families },
      chan: { edits, set: previewSet(), grade: state.grade },
    }),
  });
  if (!res.ok) return;
  state.curves = await res.json();
  // one RGBA texture per side: R, G, B tables in the first three components
  const up = (unit, rgb) => texture(unit, () => {
    const d = new Float32Array(256 * 4);
    for (let i = 0; i < 256; i++) {
      d[i * 4] = rgb[0][i]; d[i * 4 + 1] = rgb[1][i]; d[i * 4 + 2] = rgb[2][i]; d[i * 4 + 3] = 1;
    }
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA32F, 256, 1, 0, gl.RGBA, gl.FLOAT, d);
  });
  const c = state.curves;
  if (tex.lut) gl.deleteTexture(tex.lut);
  if (tex.lutRef) gl.deleteTexture(tex.lutRef);
  tex.lut = up(1, c.rgb || [c.lut, c.lut, c.lut]);
  tex.lutRef = up(3, c.rgb_ref || [c.lut_ref, c.lut_ref, c.lut_ref]);
  toneEditor.draw();
  chanEditor.draw();
  drawEffective();
  drawChannels();
  renderChannelPanel();
  renderChanges();
  drawHslCurve();
  draw();
}

const css = (v) => getComputedStyle(document.documentElement).getPropertyValue(v).trim();

function drawGrid(ctx, w, h) {
  ctx.clearRect(0, 0, w, h);
  ctx.strokeStyle = css("--line");
  ctx.lineWidth = 1;
  for (let i = 1; i < 4; i++) {
    ctx.beginPath(); ctx.moveTo((w * i) / 4, 0); ctx.lineTo((w * i) / 4, h); ctx.stroke();
    ctx.beginPath(); ctx.moveTo(0, (h * i) / 4); ctx.lineTo(w, (h * i) / 4); ctx.stroke();
  }
}

function polyline(ctx, pts, color, width, dash) {
  ctx.strokeStyle = color;
  ctx.lineWidth = width;
  ctx.setLineDash(dash || []);
  ctx.beginPath();
  pts.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
  ctx.stroke();
  ctx.setLineDash([]);
}

// A transfer-curve editor on a canvas. Click on empty space adds a point,
// click on a point selects it, drag moves it. A point goes away by
// double-click, right-click, the Delete key, or by dragging it out of the
// field. The two ends stay at x = 0 and x = 1 and cannot be removed.
function pointEditor(cv, { points, transfer, color }) {
  const HIT_PX = 14, REMOVE_PX = 40;
  const ctx = cv.getContext("2d");
  let dragIdx = -1, selected = -1, dragOut = false;

  const pos = (e) => {
    const r = cv.getBoundingClientRect();
    return [Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)),
            Math.min(1, Math.max(0, 1 - (e.clientY - r.top) / r.height))];
  };
  const outside = (e) => {
    const r = cv.getBoundingClientRect();
    return e.clientX < r.left - REMOVE_PX || e.clientX > r.right + REMOVE_PX ||
           e.clientY < r.top - REMOVE_PX || e.clientY > r.bottom + REMOVE_PX;
  };
  const hit = (e) => {
    const r = cv.getBoundingClientRect();
    let best = -1, bd = HIT_PX;
    points().forEach(([px, py], i) => {
      const d = Math.hypot(r.left + px * r.width - e.clientX, r.top + (1 - py) * r.height - e.clientY);
      if (d < bd) { bd = d; best = i; }
    });
    return best;
  };
  const removable = (i) => i > 0 && i < points().length - 1;
  const remove = (i) => {
    if (!removable(i)) return false;
    points().splice(i, 1);
    selected = -1;
    requestTone();
    return true;
  };

  function draw() {
    const w = cv.width, h = cv.height;
    drawGrid(ctx, w, h);
    polyline(ctx, [[0, h], [w, 0]], css("--muted"), 1);
    const T = transfer();
    if (T) polyline(ctx, T.map((y, i) => [(i / (T.length - 1)) * w, h - y * h]), color(), 3);
    points().forEach(([x, y], i) => {
      ctx.fillStyle = dragOut && i === dragIdx ? css("--warn") : i === selected ? color() : css("--text");
      ctx.beginPath(); ctx.arc(x * w, h - y * h, i === selected ? 12 : 9, 0, 7); ctx.fill();
    });
  }

  cv.tabIndex = 0;
  cv.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    cv.focus();
    const pts = points();
    dragIdx = hit(e);
    if (dragIdx < 0 && e.detail < 2) {     // a double-click must not add points
      const p = pos(e);
      pts.push(p);
      pts.sort((a, b) => a[0] - b[0]);
      dragIdx = pts.indexOf(p);
    }
    selected = dragIdx;
    dragOut = false;
    if (dragIdx >= 0) cv.setPointerCapture(e.pointerId);
    requestTone();
  });
  cv.addEventListener("pointermove", (e) => {
    cv.style.cursor = hit(e) >= 0 ? "grab" : "crosshair";
    if (dragIdx < 0) return;
    dragOut = removable(dragIdx) && outside(e);
    if (dragOut) { cv.style.cursor = "no-drop"; draw(); return; }
    const pts = points(), [x, y] = pos(e), last = pts.length - 1;
    const lo = dragIdx === 0 ? 0 : pts[dragIdx - 1][0] + 0.01;
    const hi = dragIdx === last ? 1 : pts[dragIdx + 1][0] - 0.01;
    pts[dragIdx] = [dragIdx === 0 ? 0 : dragIdx === last ? 1 : Math.min(hi, Math.max(lo, x)), y];
    requestTone();
  });
  cv.addEventListener("pointerup", () => {
    if (dragOut) remove(dragIdx);
    dragIdx = -1;
    dragOut = false;
  });
  cv.addEventListener("dblclick", (e) => { remove(hit(e)); });
  cv.addEventListener("contextmenu", (e) => {
    const i = hit(e);
    if (i >= 0) { e.preventDefault(); remove(i); }
  });
  cv.addEventListener("keydown", (e) => {
    if ((e.key === "Delete" || e.key === "Backspace") && remove(selected)) e.preventDefault();
  });
  return { draw, deselect: () => { selected = -1; } };
}

const toneEditor = pointEditor($("curve"), {
  points: () => state.tone.points,
  transfer: () => state.curves && state.curves.transfer,
  color: () => css("--accent"),
});

const chanEditor = pointEditor($("chan-curve"), {
  points: () => (state.fw ? chanEdit(chanActive()).points : IDENT),
  transfer: () => state.curves && state.curves.channel_transfer &&
    state.curves.channel_transfer[chanActive()],
  color: () => (state.chan.all ? css("--text") : CH_COLORS[state.chan.channel]),
});

function drawEffective() {
  const cv = $("effective"), ctx = cv.getContext("2d"), w = cv.width, h = cv.height;
  drawGrid(ctx, w, h);
  if (!state.curves) return;
  // x: input on a square-root scale so the shadows get room
  const pts = (lut) => lut.map((v, i) => [Math.sqrt(i / 255) * w, h - (v / 16384) * h]);
  polyline(ctx, pts(state.curves.lut_ref), css("--muted"), 2);
  polyline(ctx, pts(state.curves.lut), css("--accent"), 3);
}

function drawChannels() {
  const cv = $("chan-effective"), ctx = cv.getContext("2d"), w = cv.width, h = cv.height;
  drawGrid(ctx, w, h);
  if (!state.fw) return;
  // the three channel curves of the set: input = computed tone value, 0..0x4000
  const pts = (c) => c.map((v, i) => [(i / 255) * w, h - (v / 16384) * h]);
  chanSet().forEach((idx, k) => {
    polyline(ctx, pts(state.fw.channel_curves[idx]), CH_COLORS[k], 1, [6, 6]);
    polyline(ctx, pts(chanValues(idx)), CH_COLORS[k], state.chan.all || k === state.chan.channel ? 3 : 1.5);
  });
}

function renderChannelPanel() {
  if (!state.fw) return;
  for (const b of document.querySelectorAll("#chan-sets button"))
    b.classList.toggle("on", b.dataset.set === state.chan.set);
  document.querySelectorAll("#chan-channels button").forEach((b, k) => {
    b.classList.toggle("on", k === 0 ? state.chan.all : !state.chan.all && k - 1 === state.chan.channel);
    b.textContent = k === 0 ? "RGB" : CH_NAMES[k - 1] + (chanChanged(chanSet()[k - 1]) ? " •" : "");
  });
  $("chan-title").textContent = state.chan.all
    ? t("chan_all")
    : t("chan_one", { ch: CH_NAMES[state.chan.channel], up: t(["red", "green", "blue"][state.chan.channel]),
                       down: ["Cyan", "Magenta", LANG === "de" ? "Gelb" : "Yellow"][state.chan.channel] });
  $("chan-panel").classList.toggle("inactive", state.current !== "CrossProcess");
  renderWheels();
  $("chan-note").textContent = "";
}

function buildChannelPanel() {
  const sets = $("chan-sets");
  sets.innerHTML = "";
  for (const name of SET_ORDER.filter((n) => state.fw.channel_sets[n])) {
    const b = document.createElement("button");
    b.dataset.set = name;
    b.textContent = setLabel(name);
    b.onclick = () => { state.chan.set = name; chanEditor.deselect(); renderWheels(); requestTone(); };
    sets.appendChild(b);
  }
  document.querySelectorAll("#chan-channels button").forEach((b, k) => {
    if (k) b.style.color = CH_COLORS[k - 1];
    b.onclick = () => {
      if (k === 0 && !state.chan.all) {
        if (!linkSet(state.chan.set) &&
            !confirm(t("link_confirm"))) return;
        linkSet(state.chan.set, true);
        state.chan.all = true;
      } else if (k > 0) {
        if (state.chan.all) for (const n of SET_ORDER) unlinkSet(n);
        state.chan.all = false;
        state.chan.channel = k - 1;
      }
      chanEditor.deselect(); renderChannelPanel(); requestTone(); drawChannels();
    };
  });
  const src = $("chan-source");
  src.innerHTML = "";
  for (const name of SET_ORDER.filter((n) => state.fw.channel_sets[n])) {
    const set = state.fw.channel_sets[name];
    const og = document.createElement("optgroup");
    og.label = setLabel(name);
    set.forEach((idx, k) => {
      const o = document.createElement("option");
      o.value = idx;
      o.textContent = `${setLabel(name)} · ${CH_NAMES[k]}`;
      og.appendChild(o);
    });
    src.appendChild(og);
  }
  const srcSet = $("chan-source-set");
  srcSet.innerHTML = SET_ORDER.filter((n) => state.fw.channel_sets[n])
    .map((n) => `<option value="${n}">${setLabel(n)}</option>`).join("");
}

// copy curves: the source's current (possibly edited) values become the
// target's new base; the target's own points start over
$("chan-copy").onclick = () => {
  const from = +$("chan-source").value, to = chanActive();
  if (from === to) return;
  state.chan.edits[to] = { base: chanValues(from).slice(), points: IDENT.map((p) => p.slice()) };
  relink();
  chanEditor.deselect();
  requestTone();
};
$("chan-copy-set").onclick = () => {
  const from = chanSet($("chan-source-set").value), to = chanSet();
  const vals = from.map((idx) => chanValues(idx).slice());
  to.forEach((idx, k) => {
    if (idx !== from[k]) state.chan.edits[idx] = { base: vals[k], points: IDENT.map((p) => p.slice()) };
  });
  relink();
  chanEditor.deselect();
  requestTone();
};
$("chan-reset").onclick = () => {
  for (const idx of state.chan.all ? chanSet() : [chanActive()]) delete state.chan.edits[idx];
  relink();
  chanEditor.deselect();
  requestTone();
};
$("chan-reset-set").onclick = () => {
  for (const idx of chanSet()) delete state.chan.edits[idx];
  delete state.grade[state.chan.set];
  relink();
  chanEditor.deselect();
  requestTone();
};

// ---- loading a preset ---------------------------------------------------------

// A preset is the *.looks.json an export writes:
//   { source, matrices: {look: [9]}, tone: {points, families} | null,
//     channel_curves: {idx: [256]} | null, channel_edits: {idx: {base, points}} | null,
//     inherit: {target: source} | null }
// It replaces every current edit. Look data inheritance goes first, because
// it changes which matrix address a look edits.
async function loadPreset(file) {
  const msg = $("preset-msg");
  let ok = false;
  msg.className = "note";
  try {
    let p;
    try { p = JSON.parse(await file.text()); } catch { throw new Error(t("e_json")); }
    if (!p || typeof p !== "object" || !["matrices", "matrix_edits", "tone", "channel_curves", "channel_edits", "inherit", "hsl"].some((k) => k in p))
      throw new Error(t("e_preset"));
    const warn = [];

    const inherit = p.inherit || {};
    const targets = new Set([...Object.keys(state.fw.inherit || {}), ...Object.keys(inherit)]);
    for (const target of targets) {
      const source = inherit[target] || null;
      if (((state.fw.inherit || {})[target] || null) === source) continue;
      const res = await api("/api/inherit", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ target, source }),
      });
      if (!res.ok) { warn.push(t("e_inherit", { target, source, msg: await errorText(res) })); continue; }
      const fw = await res.json();
      Object.assign(state.fw, { looks: fw.looks, inherit: fw.inherit });
      for (const l of fw.looks) state.looks[l.name] = l;
    }

    state.edits = {};
    // newer presets keep base + sliders (incl. calibration) per look
    for (const [name, e] of Object.entries(p.matrix_edits || {})) {
      if (!state.looks[name] || !e) continue;
      const base = e.base == null ? state.looks[name].matrix
        : Array.isArray(e.base) && e.base.length === 9 && e.base.every(Number.isFinite) ? e.base : null;
      if (!base) { warn.push(t("e_matrix", { name })); continue; }
      const cal = neutralCalib();
      for (const q of PRIMS) for (const k of ["hue", "sat"])
        cal[q][k] = Math.max(-100, Math.min(100, +(((e.calib || {})[q] || {})[k]) || 0));
      Object.assign(editOf(name), { base: base.slice(), sat: +e.sat || 1, hue: +e.hue || 0, calib: cal });
    }
    for (const [name, m] of Object.entries(p.matrices || {})) {
      if (p.matrix_edits && p.matrix_edits[name]) continue;
      if (!state.looks[name]) { warn.push(t("e_look", { name })); continue; }
      if (!Array.isArray(m) || m.length !== 9 || m.some((v) => !Number.isFinite(v))) {
        warn.push(t("e_matrix", { name })); continue;
      }
      Object.assign(editOf(name), { base: m.slice(), sat: 1, hue: 0 });
    }

    const matrixCount = new Set([...Object.keys(p.matrices || {}), ...Object.keys(p.matrix_edits || {})]
      .filter((n) => state.looks[n])).size;
    const tone = p.tone || {};
    const pts = Array.isArray(tone.points) && tone.points.length >= 2 &&
      tone.points.every((q) => Array.isArray(q) && q.length === 2 && q.every(Number.isFinite))
      ? tone.points.map((q) => q.slice()).sort((a, b) => a[0] - b[0]) : null;
    if (p.tone && !pts) warn.push(t("e_tone"));
    state.tone.points = pts || IDENT.map((q) => q.slice());
    state.tone.families = Array.isArray(tone.families) ? tone.families.filter((f) => [0, 1, 2].includes(f)) : [0, 1, 2];
    for (const cb of document.querySelectorAll(".fam")) cb.checked = state.tone.families.includes(+cb.value);

    state.chan.edits = {};
    state.grade = {};
    let channels = 0;
    // with wheels the preset keeps the curves before grading, so the wheels stay editable
    const pre = p.channel_grade && p.channel_curves_pre;
    if (p.channel_grade && (pre || p.channel_edits)) {
      for (const [n, g] of Object.entries(p.channel_grade)) {
        if (!state.fw.channel_sets[n]) { warn.push(t("e_set", { name: n })); continue; }
        state.grade[n] = normGrade(g);
      }
    }
    const curve = (v) => Array.isArray(v) && v.length === 256 && v.every(Number.isFinite);
    const validPoints = (v) => Array.isArray(v) && v.length >= 2 &&
      v.every((q) => Array.isArray(q) && q.length === 2 && q.every(Number.isFinite));
    if (p.channel_edits) {
      // newer presets keep the points, so the curves stay editable
      for (const [idx, e] of Object.entries(p.channel_edits)) {
        const i = +idx;
        if (!(i >= 0 && i < 12) || !e || !(e.base == null || curve(e.base)) || !validPoints(e.points)) {
          warn.push(t("e_channel", { idx })); continue;
        }
        state.chan.edits[i] = {
          base: e.base == null ? null : e.base.map((v) => Math.round(v)),
          points: e.points.map((q) => q.slice()).sort((a, b) => a[0] - b[0]),
        };
        channels++;
      }
    } else {
      // older presets: only the finished curves, which become the base under straight points
      for (const [idx, vals] of Object.entries((pre ? p.channel_curves_pre : p.channel_curves) || {})) {
        const i = +idx;
        if (!(i >= 0 && i < 12) || !curve(vals)) { warn.push(t("e_channel", { idx })); continue; }
        state.chan.edits[i] = { base: vals.map((v) => Math.round(v)), points: IDENT.map((q) => q.slice()) };
        channels++;
      }
    }

    warn.push(...hslLoad(p.hsl));
    renderHsl();
    requestHsl();

    detectExtBase();
    relink();
    toneEditor.deselect();
    chanEditor.deselect();
    renderWheels();
    update();
    requestTone();
    if (p.source && state.fw.path && !p.source.endsWith(state.fw.path))
      warn.push(t("e_source", { source: p.source }));
    const parts = [
      matrixCount && t("p_matrices", { n: matrixCount }),
      pts && !isIdent(pts) && t("p_tone"),
      channels && t("p_channels", { n: channels }),
      gradedSets().length && t("p_wheels", { list: gradedSets().map(setLabel).join(", ") }),
      hslChangedLooks().length && t("p_hsl", { list: hslChangedLooks().map(label).join(", ") }),
      Object.keys(inherit).length && Object.entries(inherit).map(([t, s]) => `${t} ← ${s}`).join(", "),
    ].filter(Boolean);
    msg.className = warn.length ? "note warn" : "note";
    ok = !warn.length;
    msg.textContent = t("preset_loaded", { name: file.name, parts: parts.join(" · ") || t("preset_nothing") }) +
      (warn.length ? t("preset_hints", { list: warn.join("; ") }) : "");
  } catch (err) {
    msg.className = "note warn";
    msg.textContent = t("preset_failed", { msg: err.message });
  }
  return ok;
}


// ---- colour grading wheels -----------------------------------------------------

// hue in degrees (red 0, yellow 60, green 120, ...), amount 0..1, lum -1..1
const neutralWheel = () => ({ hue: 0, amount: 0, lum: 0 });
function normGrade(g) {
  const out = { balance: Math.max(-1, Math.min(1, +((g || {}).balance) || 0)) };
  for (const [k] of WHEELS) {
    const w = (g || {})[k] || {};
    out[k] = { hue: ((+w.hue || 0) % 360 + 360) % 360,
               amount: Math.max(0, Math.min(1, +w.amount || 0)),
               lum: Math.max(-1, Math.min(1, +w.lum || 0)) };
  }
  return out;
}
function gradeOf(set) {
  if (!state.grade[set]) state.grade[set] = normGrade({});
  return state.grade[set];
}

function hsl2rgb(h, s, l) {
  const f = (n) => {
    const k = (n + h / 30) % 12, a = s * Math.min(l, 1 - l);
    return l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1));
  };
  return [f(0), f(8), f(4)];
}

let discImage = null;
function disc(n) {
  if (discImage && discImage.width === n) return discImage;
  discImage = new ImageData(n, n);
  const c = n / 2, R = c - 2;
  for (let y = 0; y < n; y++)
    for (let x = 0; x < n; x++) {
      const dx = x + 0.5 - c, dy = c - (y + 0.5), r = Math.hypot(dx, dy) / R;
      const o = (y * n + x) * 4;
      if (r > 1) { discImage.data[o + 3] = 0; continue; }
      const h = (Math.atan2(dy, dx) * 180 / Math.PI + 360) % 360;
      const rgb = hsl2rgb(h, 1, 0.5);
      for (let i = 0; i < 3; i++) discImage.data[o + i] = 255 * (0.32 + (rgb[i] - 0.32) * r * 0.85);
      discImage.data[o + 3] = r > 0.985 ? 255 * (1 - r) / 0.015 : 255;
    }
  return discImage;
}

const wheelUI = {};
let gradeTimer = null;
function gradeChanged() {
  renderWheels();
  renderChanges();
  clearTimeout(gradeTimer);       // dragging sends at most one request per frame
  gradeTimer = setTimeout(requestTone, 16);
}

function buildWheels() {
  const box = $("wheels");
  box.innerHTML = "";
  for (const [key, label] of WHEELS) {
    const el = document.createElement("div");
    el.className = "wheel";
    el.innerHTML = `<div class="title"><span data-i18n="${label}"></span><output></output></div>
      <canvas width="280" height="280"></canvas>
      <input type="range" min="-100" max="100" step="1" value="0" data-i18n-title="luminance">`;
    box.appendChild(el);
    const cv = el.querySelector("canvas"), lum = el.querySelector("input");
    const at = (e) => {
      const r = cv.getBoundingClientRect();
      const dx = (e.clientX - r.left) / r.width * 2 - 1, dy = 1 - (e.clientY - r.top) / r.height * 2;
      const w = gradeOf(state.chan.set)[key];
      w.amount = Math.min(1, Math.hypot(dx, dy) / ((r.width / 2 - 2) / (r.width / 2)));
      w.hue = (Math.atan2(dy, dx) * 180 / Math.PI + 360) % 360;
      gradeChanged();
    };
    let drag = false;
    cv.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 || e.detail > 1) return;
      drag = true; cv.setPointerCapture(e.pointerId); at(e);
    });
    cv.addEventListener("pointermove", (e) => drag && at(e));
    cv.addEventListener("pointerup", () => { drag = false; });
    cv.addEventListener("dblclick", () => { Object.assign(gradeOf(state.chan.set)[key], neutralWheel()); gradeChanged(); });
    lum.oninput = () => { gradeOf(state.chan.set)[key].lum = +lum.value / 100; gradeChanged(); };
    lum.ondblclick = () => { gradeOf(state.chan.set)[key].lum = 0; gradeChanged(); };
    wheelUI[key] = { el, cv, lum, out: el.querySelector("output") };
  }
  $("grade-balance").oninput = (e) => { gradeOf(state.chan.set).balance = +e.target.value / 100; gradeChanged(); };
  $("grade-reset").onclick = () => { delete state.grade[state.chan.set]; gradeChanged(); };
}

function renderWheels() {
  if (!state.fw || !wheelUI.global) return;
  const g = state.grade[state.chan.set] || normGrade({});
  $("grade-set").textContent = setLabel(state.chan.set);
  for (const [key] of WHEELS) {
    const { el, cv, lum, out } = wheelUI[key], w = g[key];
    const ctx = cv.getContext("2d"), n = cv.width, c = n / 2, R = c - 2;
    ctx.putImageData(disc(n), 0, 0);
    ctx.strokeStyle = "rgba(0,0,0,.35)";
    ctx.beginPath(); ctx.moveTo(c - 8, c); ctx.lineTo(c + 8, c); ctx.moveTo(c, c - 8); ctx.lineTo(c, c + 8); ctx.stroke();
    const a = w.hue * Math.PI / 180, px = c + Math.cos(a) * w.amount * R, py = c - Math.sin(a) * w.amount * R;
    ctx.lineWidth = 3;
    ctx.strokeStyle = "#fff";
    ctx.fillStyle = `rgb(${hsl2rgb(w.hue, 1, 0.5).map((v) => 255 * (0.32 + (v - 0.32) * w.amount)).join()})`;
    ctx.beginPath(); ctx.arc(px, py, 11, 0, 7); ctx.fill(); ctx.stroke();
    ctx.lineWidth = 1;
    lum.value = Math.round(w.lum * 100);
    out.textContent = wheelNeutral(w) ? "—"
      : `${Math.round(w.hue)}° ${Math.round(w.amount * 100)}%` + (Math.abs(w.lum) > 1e-4 ? ` L${w.lum > 0 ? "+" : ""}${Math.round(w.lum * 100)}` : "");
    el.classList.toggle("on", !wheelNeutral(w));
  }
  $("grade-balance").value = Math.round(g.balance * 100);
  $("grade-balance-out").textContent = Math.round(g.balance * 100);
  $("grade-panel").classList.toggle("inactive", state.current !== "CrossProcess");
  $("grade-note").className = state.chan.set === "Zufall" && !gradeNeutral(g) ? "note warn" : "note";
  $("grade-note").textContent = "";
}

$("help-open").onclick = () => $("help").showModal();

function renderVignetteNote() {
  const v = state.shot && state.shot.vignette;
  const el = $("vignette-note");
  $("vignette").disabled = !v;
  if (!v) { el.textContent = ""; return; }
  // gain at 80 % of the way from the centre to a corner (r = 1 is the corner)
  const r2 = 0.64, c = v.coef, corner = 1 + r2 * (c[0] + r2 * (c[1] + r2 * c[2]));
  el.textContent = t(v.source === "fitted" ? "vignette_fitted" : "vignette_default", { corner: corner.toFixed(2) });
}

function renderToneNote() {
  const meta = state.shot;
  const has = meta && Object.keys(meta.params || {}).length;
  $("tone-note").className = "note";
  $("tone-note").textContent = !meta ? t("tone_none")
    : (state.paramsFromDng ? t(has ? "tone_from_photo" : "tone_defaults") : t("tone_reset")) +
      t(!meta.residual ? "tone_unmatched" : state.paramsFromDng || !has ? "tone_matched" : "tone_matched_shot");
}

// ---- language -----------------------------------------------------------------

function setLang(lang) {
  LANG = lang;
  try { localStorage.setItem("lang", lang); } catch { /* no storage */ }
  applyI18n();
  if (!state.fw) return;
  $("fw-info").textContent = t("firmware", state.build);
  buildChannelPanel();
  renderInherit();
  renderToneNote();
  renderVignetteNote();
  renderWb();
  renderHsl();
  update();
  renderChannelPanel();
  placeSplit();
}
for (const b of document.querySelectorAll("#lang button")) b.onclick = () => setLang(b.dataset.lang);
applyI18n();
// from the header: the result note sits on the Export tab, so show it when there is a problem
$("head-preset-file").onchange = async (e) => {
  const f = e.target.files[0];
  e.target.value = "";
  if (f && !(await loadPreset(f))) showTab("export");
};
// a look shipped with the editor, to start from
$("example-preset").onclick = async () => {
  const res = await fetch("presets/example.looks.json");
  if (!res.ok) return;
  if (!(await loadPreset(new File([await res.blob()], t("example_name"))))) showTab("export");
};
$("preset-file").onchange = (e) => {
  if (e.target.files[0]) loadPreset(e.target.files[0]);
  e.target.value = "";     // the same file again must trigger a reload
};

$("file").onchange = (e) => {
  const f = e.target.files[0];
  if (f) (/\.json$/i.test(f.name) ? loadPreset(f) : loadDng(f));
  e.target.value = "";
};
const wrap = $("wrap");
wrap.addEventListener("dragover", (e) => { e.preventDefault(); wrap.classList.add("drag"); });
wrap.addEventListener("dragleave", () => wrap.classList.remove("drag"));
wrap.addEventListener("drop", (e) => {
  e.preventDefault();
  wrap.classList.remove("drag");
  const f = e.dataTransfer.files[0];
  if (!f) return;
  if (/\.json$/i.test(f.name)) loadPreset(f);
  else loadDng(f);
});

// ---- start ------------------------------------------------------------------

(async () => {
  try {
    initGL();
    state.fw = await (await api("/api/firmware")).json();
    for (const l of state.fw.looks) state.looks[l.name] = l;
    state.current = INHERIT_TARGET;
    const build = state.fw.path.split("/").slice(-2, -1)[0] || "";
    state.build = { camera: state.fw.camera, version: state.fw.version, build };
    Object.assign(I18N_VARS, { camera: state.fw.camera, update: state.fw.update_file });
    $("fw-info").textContent = t("firmware", state.build);
    if (!store.get("helpSeen")) { $("help").showModal(); store.set("helpSeen", "1"); }
    state.extBases = await loadExtBases();
    buildChannelPanel();
    relink();
    buildWheels();
    initHsl();
    applyI18n();
    renderWheels();
    renderHsl();
    requestHsl();
    update();
    await fetchTone();
    const pre = await api("/api/dng");
    if (pre.status === 200) await show(await pre.json());
    const tab = store.get("tab");
    if (tab && document.querySelector(`#tabs [data-tab="${tab}"]`)) showTab(tab);
  } catch (err) {
    $("fw-info").textContent = t("error_prefix", { msg: err.message });
  }
})();
