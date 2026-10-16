// GR III / GR IIIx look editor — preview and the edit column of one target.
//
// The preview runs the colour part of ColorMatrixTuning on the GPU:
//   camera RGB -> white balance -> sensor CCM -> look matrix -> (mono mix)
//   -> MultiAxial (HSL, hsl.js)
// followed by the camera's tone curve. That curve comes from the core (worker), which
// runs the camera's own GammaTuning code on the base curves; a small residual
// fitted from the camera JPEG stands in for its image-dependent shadow
// correction. It always shows the selected target (targets.js) as the camera
// would: its source's look data, its own matrix, curves and HSL.

const $ = (id) => document.getElementById(id);
const SCALE = 8192;

const state = {
  fw: null,            // /api/firmware
  looks: {},           // name -> look from the session's firmware (addresses)
  stock: {},           // name -> look as in stock firmware (matrices, sat and hue matrices)
  targets: {},         // target name -> { source, ext, effects_off } (targets.js)
  edits: {},           // target name -> { base, sat, hue, calib }
  current: null,       // the selected target; null: none yet
  mode: "edit",
  split: 0.5,
  ev: 0,
  wb: { kelvin: false, k: 5200, ab: 0, gm: 0 },   // preview-only white balance (false: as shot)
  wbCal: null,         // [R, B] calibration of the photo's camera, null: typical
  wbShot: null,        // { k, gm } closest to the shot's white balance
  shot: null,          // /api/dng meta
  params: { key: 0, contrast: 0, highlight: 0, shadow: 0, saturation: 0, hue: 0, shadowcorr: 0 },
  residual: false,     // fine match to the camera JPEG: dormant, no GUI switch any more
  vignette: true,      // the camera's lens shading correction in the preview
  curves: null,        // last /api/tone answer
  chan: { edits: {}, set: "Preset 1", channel: 0, all: true },   // channel curves by index (Color Tone 1-3, the looks' own)
  extBases: [],        // extra bases: BUILTIN_BASES and _local/bases.json (not in the repo)
  grade: {},           // set name -> colour wheels, applied by the core after the edits
};
const TONE_KEYS = ["key", "contrast", "highlight", "shadow"];
// camera settings that act through matrices, not the tone curve: saturation
// selects A[sat] in ColorMatrixTuning, hue a Cb/Cr rotation after the curve
const COLOR_KEYS = ["saturation", "hue"];

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
const SOURCE_HINTS = {
  Standard: "hint_standard", Vivid: "hint_vivid", PositiveFilm: "hint_positive", NegativeFilm: "hint_negative",
  Retro: "hint_retro", BleechBypass_GR: "hint_bleach", "BleechBypass_GR · Var. 2": "hint_bleach_var",
  "BleechBypass_GR · Var. 3": "hint_bleach_var", HdrCuisine: "hint_hdr", CrossProcess: "hint_cross",
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
// the camera's saturation matrix for a look and the current setting
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

// ---- calibration ------------------------------------------------------------

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
uniform mat3 uM, uMRef, uHue, uPost;
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
  // the target's post matrix P, then the hue setting: the camera's YCbCr
  // stage computes H B P (postmatrix.rs); the stock look has no P
  if (ma) g = uPost * g;
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

// what the preview renders: the target as the camera would (its source's look
// data, its own matrix), against the target's stock look; without a target the
// photo's own image control, unedited
function previewLooks() {
  const T = state.current;
  if (!T) {
    const l = state.stock[(state.shot && state.shot.shot_look) || "Standard"] || state.stock.Standard;
    return { m: l.matrix, look: l, ref: l.matrix, refLook: l, mono: l.mono_mix, post: ident() };
  }
  const own = state.stock[T];
  return { m: effective(T), look: sourceLook(T), ref: own.matrix, refLook: own, mono: false, post: postOf(T) || ident() };
}

function draw() {
  if (!state.shot || !tex.lut) return;
  const p = previewLooks();
  const sensor = state.fw.sensor_ccm;
  const m = mul(mul(satMatrix(p.look), p.m), sensor);
  const mRef = mul(mul(satMatrix(p.refLook), p.ref), sensor);
  gl.viewport(0, 0, canvas.width, canvas.height);
  gl.uniform1i(uni("uImg"), 0);
  gl.uniform1i(uni("uLut"), 1);
  gl.uniform1i(uni("uCam"), 2);
  gl.uniform1i(uni("uLutRef"), 3);
  gl.uniform1i(uni("uRes"), 4);
  gl.uniformMatrix3fv(uni("uM"), false, transpose(m));
  gl.uniformMatrix3fv(uni("uMRef"), false, transpose(mRef));
  gl.uniformMatrix3fv(uni("uHue"), false, transpose(hueMatrix(p.look)));
  gl.uniformMatrix3fv(uni("uPost"), false, transpose(p.post));
  gl.uniform1i(uni("uMono"), p.mono ? 1 : 0);
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
  const T = state.current;
  $("label-l").textContent = state.mode !== "ab" ? t("label_camera") : T ? t("label_stock", { look: label(T) }) : t("label_original");
  $("label-r").textContent = T ? t("label_edited") : t("label_original");
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

// ---- target and source pickers ------------------------------------------------

function renderPickers() {
  if (!state.fw) return;
  const T = state.current;
  const tsel = $("target");
  if (!T) $("print-row").hidden = $("hc-box").hidden = $("log-box").hidden = true;
  tsel.innerHTML = `<option value="">${esc(t("target_choose"))}</option>` + targetNames().map((n) =>
    `<option value="${esc(n)}">${esc(label(n))} — ${esc(targetState(n))}${targetChanged(n) ? " ●" : ""}</option>`).join("");
  tsel.value = T || "";

  const ssel = $("source"), gsel = $("grain");
  ssel.disabled = gsel.disabled = !T;
  if (!T) { ssel.innerHTML = gsel.innerHTML = ""; $("pick-note").hidden = true; return; }
  const s = targetSpec(T);
  gsel.innerHTML = [0, 3, 2, 1].map((g) =>
    `<option value="${g}">${esc(g ? grainText(g) : t(GRAIN_OWN.includes(T) ? "grain_camera" : "grain_off"))}</option>`).join("");
  gsel.value = String(s.grain);
  gsel.title = t("grain_t");
  const opt = (v, text, hint) => `<option value="${esc(v)}"${hint ? ` title="${esc(hint)}"` : ""}>${esc(text)}</option>`;
  const looks = sourcesFor(T).map((n) =>
    opt(`look:${n}`, n === T ? t("src_own", { look: label(n) }) : label(n), SOURCE_HINTS[n] && t(SOURCE_HINTS[n])));
  const exts = state.extBases.map((x) => opt(`ext:${x.id}`, extText(x.label), extText(x.hint)));
  // a target fitted to a LUT shows the LUT, one from the film library the film
  // (both compute on Standard's tone curve)
  const film = filmOf(T), log = lutLib(T) === "grlog";
  const fitted = !film && !log && lutName(T);
  ssel.innerHTML = (fitted ? `<optgroup label="${esc(t("src_lut_group"))}">${opt("lut", fitted, t("src_lut_t"))}</optgroup>` : "") +
    grlogOptions(T, opt) + filmOptions(T, opt) +
    `<optgroup label="${esc(t("src_looks"))}">${looks.join("")}</optgroup>` +
    (exts.length ? `<optgroup label="${esc(t("src_extra"))}">${exts.join("")}</optgroup>` : "");
  ssel.value = film ? `film:${film.film}` : log ? "grlog" : fitted ? "lut" : s.ext ? `ext:${s.ext}` : `look:${s.source}`;
  ssel.title = film ? film.title : log ? t("src_grlog_t") : fitted ? t("src_lut_t") : s.ext ? extText(extOf(T).tip) : "";
  renderPrintRow(T);

  // at most one line about the target's rules
  const note = $("pick-note");
  const key = s.grain ? "pick_grain"
    : T === HDR ? "pick_hdr"
    : T === BLEACH && s.source !== BLEACH ? "pick_bleach"
    : T === CROSS ? "pick_cross" : null;
  note.hidden = !key && !film && !log;
  if (film) note.innerHTML = t("pick_film");      // the attribution spektrafilm's licence asks for
  else if (log) note.textContent = t("pick_grlog");
  else if (key) note.textContent = t(key);
}

$("target").onchange = (e) => select(e.target.value || null);
$("source").onchange = (e) => setSource(state.current, e.target.value);
$("grain").onchange = (e) => {
  if (!state.current) return;
  targetSpec(state.current).grain = +e.target.value;
  update();
};
// targets whose camera menu has a grain setting of its own (0 leaves it to that)
const GRAIN_OWN = ["NegativeFilm"];
// the firmware's grain 1 is the strongest, 3 the weakest (camera test, docs/grain.md)
const grainText = (g) => t(["", "grain_strong", "grain_mid", "grain_weak"][g]);

// switch the target's base; curves, wheels, HSL and the sliders stay
function setSource(T, value) {
  if (!T || value === "lut") return;
  if (value === "grlog") return applyGrLog(T);       // the log curve for video (lut.js)
  if (value.startsWith("film:")) {
    // a film of the library: its own print, or the print it had
    const f = filmOf(T), film = value.slice(5);
    return applyFilm(T, film, f && f.film === film ? f.print : null);
  }
  const s = targetSpec(T);
  const ext = value.startsWith("ext:") ? value.slice(4) : null;
  const source = ext ? T : value.slice(5);
  // another source: the fitted values stay, the target no longer says it is the LUT
  const wasLut = !!s.lut;
  s.lut = null;
  if (s.ext === ext && s.source === source) { if (wasLut) update(); return; }
  clearExtCurves(T, extOf(T));
  s.source = source;
  s.ext = ext;
  editOf(T).base = null;     // hand-typed matrix values belonged to the old base
  applyExtCurves(T, extOf(T));
  relink();
  chanEditor.deselect();
  update();
  requestTone();
}

function select(name) {
  state.current = name && isTarget(name) ? name : null;
  if (state.current) {
    hsl.drag = null;
    renderHsl();
    requestHsl();
  }
  chanEditor.deselect();
  update();
  requestTone();
}

// ---- the edit column ----------------------------------------------------------

const SECTIONS = ["base", "curves", "wheels", "hsl", "adv"];
function sectionDirty(sec, T) {
  switch (sec) {
    case "base": return baseColourChanged(T);
    case "curves": return curvesEdited(T);
    case "wheels": return gradedOf(T).length > 0;
    case "hsl": return !hslCurvesFlat(hsl.looks[T]);   // the user's curves; a film's base is not an edit
    case "adv": return matrixTyped(T) || postEdited(T);
  }
  return false;
}

function renderSections() {
  const T = state.current;
  $("no-target").hidden = !!T;
  for (const sec of SECTIONS) {
    const el = $(`sec-${sec}`);
    el.hidden = !T;
    if (el.hidden) continue;
    const dirty = sectionDirty(sec, T);
    el.classList.toggle("dirty", dirty);
    el.querySelector("button.reset").disabled = !dirty;
  }
  if (!T) return;
  renderBase();
  renderAdvanced();
}

function resetSection(sec) {
  const T = state.current;
  if (!T) return;
  const e = editOf(T);
  switch (sec) {
    case "base": Object.assign(e, { sat: 1, hue: 0, calib: neutralCalib() }); break;
    case "adv": e.base = null; targetSpec(T).post = startPost(T) && startPost(T).slice(); break;
    case "curves":
      for (const i of targetChannels(T)) delete state.chan.edits[i];
      applyStartCurves(T);
      relink();
      chanEditor.deselect();
      requestTone();
      break;
    case "wheels":
      for (const n of setsOf(T)) delete state.grade[n];
      requestTone();
      break;
    case "hsl": {
      // the user's curves go; a fitted film's or LUT's base stays
      const base = hsl.looks[T] && hsl.looks[T].base;
      delete hsl.looks[T];
      if (base) hslOf(T).base = base;
      renderHsl(); requestHsl(); break;
    }
  }
  update();
}
for (const sec of SECTIONS) {
  const el = $(`sec-${sec}`);
  el.querySelector("button.reset").onclick = (ev) => { ev.preventDefault(); ev.stopPropagation(); resetSection(sec); };
  el.addEventListener("toggle", () => {
    store.set(`sec-${sec}`, el.open ? "1" : "0");
    if (!el.open) return;
    // canvases draw at their size once visible
    if (sec === "curves") { chanEditor.draw(); }
    if (sec === "wheels") renderWheels();
    if (sec === "hsl") drawHslCurve();
    if (sec === "adv") drawChannels();
  });
}
function restoreSections() {
  for (const sec of SECTIONS) {
    const v = store.get(`sec-${sec}`);
    $(`sec-${sec}`).open = v ? v === "1" : sec === "base" || sec === "curves";
  }
}

function renderBase() {
  const T = state.current;
  const e = editOf(T);
  $("sat").value = e.sat; $("sat-out").textContent = e.sat.toFixed(2);
  $("hue").value = e.hue; $("hue-out").textContent = `${e.hue.toFixed(1)}°`;
  for (const p of PRIMS) for (const k of ["hue", "sat"]) {
    const el = calibUI[p + k];
    if (!el) continue;
    el.input.value = e.calib[p][k];
    el.out.textContent = e.calib[p][k] > 0 ? `+${e.calib[p][k]}` : `${e.calib[p][k]}`;
  }
}

function renderAdvanced() {
  const T = state.current;
  if (!T) return;
  const start = startMatrix(T);
  const m = effective(T);
  const box = $("matrix");
  box.innerHTML = "";
  m.forEach((v, i) => {
    const inp = document.createElement("input");
    inp.type = "number";
    inp.step = "0.01";
    inp.value = v.toFixed(4);
    if (Math.round(v * SCALE) !== Math.round(start[i] * SCALE)) inp.classList.add("diff");
    inp.onchange = () => {
      const val = parseFloat(inp.value);
      if (Number.isNaN(val)) return renderAdvanced();
      const cur = effective(T);
      cur[i] = val;
      Object.assign(editOf(T), { base: cur, sat: 1, hue: 0, calib: neutralCalib() });
      update();
    };
    box.appendChild(inp);
    if (i % 3 === 2) box.appendChild(document.createElement("span"));
  });
  const sums = [0, 1, 2].map((r) => m[r * 3] + m[r * 3 + 1] + m[r * 3 + 2]);
  $("sums").innerHTML = t("row_sums") + sums
    .map((s) => `<span class="${Math.abs(s - 1) > 0.001 ? "bad" : ""}">${s.toFixed(4)}</span>`)
    .join(" · ") + (sums.some((s) => Math.abs(s - 1) > 0.001) ? ` <span class="bad">${t("cast")}</span>` : "");
  const look = state.looks[T];
  $("addr").textContent = look ? `0x${look.address.toString(16)}` : "";
  renderPost(T);
}

// the post matrix P (after the tone curve, docs/plan-post-matrix.md): typed in
// or from a preset; the core refuses rows that do not sum to 1 and values the
// camera's registers cannot hold
function renderPost(T) {
  const p = postOf(T) || ident();
  const box = $("post");
  box.innerHTML = "";
  p.forEach((v, i) => {
    const inp = document.createElement("input");
    inp.type = "number";
    inp.step = "0.01";
    inp.value = v.toFixed(4);
    if (Math.abs(v - (i % 4 === 0 ? 1 : 0)) > 1e-9) inp.classList.add("diff");
    inp.onchange = () => {
      const val = parseFloat(inp.value);
      if (Number.isNaN(val)) return renderPost(T);
      const cur = (postOf(T) || ident()).slice();
      cur[i] = val;
      // keep the row summing to 1: the diagonal takes up the difference
      const r = Math.floor(i / 3);
      if (i !== 4 * r) cur[4 * r] = 1 - (cur[3 * r] + cur[3 * r + 1] + cur[3 * r + 2] - cur[4 * r]);
      targetSpec(T).post = cur;
      update();
    };
    box.appendChild(inp);
    if (i % 3 === 2) box.appendChild(document.createElement("span"));
  });
}

// the header's export button and the list in the export dialog
function renderChanges() {
  if (!state.fw) return;
  const list = changedTargets();
  $("export-open").textContent = list.length ? t("export_n", { n: list.length }) : t("export_btn");
  renderExportDialog();
  requestSpace();
}
const chanSetOf = (idx) => Object.keys(state.fw.channel_sets).find((n) => state.fw.channel_sets[n].includes(idx));

function update() {
  if (!state.fw) return;
  renderPickers();
  renderSections();
  renderChannelPanel();
  renderChanges();
  renderLutButton();
  syncTargets();
  draw();
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
$("sat").ondblclick = () => { editOf(state.current).sat = 1; update(); };
$("hue").ondblclick = () => { editOf(state.current).hue = 0; update(); };
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

async function errorText(res) {
  const body = await res.text();
  const m = body.match(/<p>(.*?)<\/p>/s);
  return m ? m[1] : t("error", { status: res.status });
}

// a one-line message under the header (preset loaded, a refused request)
function notice(text, warn) {
  $("notice").hidden = !text;
  $("notice").classList.toggle("warn", !!warn);
  $("notice-text").textContent = text || "";
}
$("notice-close").onclick = () => notice("");

// ---- extra bases ----------------------------------------------------------------
// Bases that are not image controls of the GR III / GR IIIx firmware.
// Each: { id, label, hint, tip: {en, de}, matrix_from: look name, matrix_mul: [9],
//         channel_curve: [256] }. The matrix is matrix_mul x the look's stock
// matrix of the loaded firmware; the curve goes on R, G and B of the target's
// curves (Color Tone 1-3 for Cross Process). The target keeps its own look
// data. An optional _local/bases.json (not in the repo) adds bases or replaces
// one with the same id.

// Extra bases built into the editor: a colour matrix relative to Standard and a
// tone curve for the channel curves.
const BUILTIN_BASES = [{
  id: "positive-film-classic",
  label: {"en": "Oldschool Positive Film", "de": "Oldschool Positive Film"},
  hint: {"en": "rich red, muted green and yellow, deep blacks", "de": "kräftiges Rot, gedämpftes Grün und Gelb, tiefe Schwärzen"},
  tip: {"en": "Colour as a matrix, tone as curves on R, G and B. In the camera: the target's controls at 0.", "de": "Farbe als Matrix, Ton als Kurven auf R, G und B. In der Kamera: Regler des Ziels auf 0."},
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
const extMatrix = (x) => mul(x.matrix_mul, state.stock[x.matrix_from].matrix);
const sameCurve = (a, b) => a && b && a.length === b.length && a.every((v, k) => v === b[k]);

async function loadExtBases() {
  let local = [];
  try {
    const res = await fetch("_local/bases.json", { cache: "no-store" });
    if (res.ok) local = await res.json();
  } catch { /* no local bases */ }
  const curve = (v) => Array.isArray(v) && v.length === 256 && v.every(Number.isFinite);
  const valid = (x) => x && typeof x.id === "string" &&
    state.stock[x.matrix_from] && Array.isArray(x.matrix_mul) && x.matrix_mul.length === 9 &&
    x.matrix_mul.every(Number.isFinite) && (x.channel_curve == null || curve(x.channel_curve));
  const byId = new Map(BUILTIN_BASES.filter(valid).map((x) => [x.id, x]));
  for (const x of (Array.isArray(local) ? local : []).filter(valid)) byId.set(x.id, x);
  return [...byId.values()];
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
  hcFromShot(meta);
  state.wbShot = wbEstimate(meta.wb, wbCal());   // a new photo starts from its own white balance
  wbAsShot();
  $("empty").hidden = true;
  $("shot-info").textContent = [meta.filename, meta.shot_look ? label(meta.shot_look) : meta.image_tone]
    .filter(Boolean).join(" · ");
  // a new photo starts with the preview settings at their defaults; only the
  // white balance comes from the DNG ("Load preview settings from DNG" takes the rest)
  applyShotParams(false);
  renderVignetteNote();
  await fetchTone();
  update();
}

// ---- channel curves ---------------------------------------------------------------

const IDENT = [[0, 0], [1, 1]];
const isIdent = (pts) => pts.every(([x, y]) => Math.abs(x - y) < 1e-4);
const CH_NAMES = ["R", "G", "B"];
const CH_COLORS = ["#e0645a", "#6fbf73", "#5b8fe0"];

const chanSet = (name) => state.fw.channel_sets[name || curSet()];
// the other colour looks' curves (core: look_sets), merged into channel_sets at start
const lookSets = () => (state.fw && state.fw.curve_looks) || [];
// the set the curves section edits: Cross Process's Color Tone, or the target's own
const curSet = () => (state.current && state.current !== CROSS ? state.current : state.chan.set);
const allSets = () => [...SET_ORDER, ...lookSets()];
function mergeLookSets() {
  Object.assign(state.fw.channel_sets, state.fw.look_channel_sets);
  while (state.fw.channel_curves.length < 12 + 3 * lookSets().length)
    state.fw.channel_curves.push(state.fw.channel_linear.slice());
}
// internal set names stay (presets store them); the camera menu calls them Farbton 1-3
const SET_LABEL = { "Preset 1": "set_1", "Preset 2": "set_2", "Preset 3": "set_3" };
const setLabel = (name) => (SET_LABEL[name] ? t(SET_LABEL[name]) : LOOK_LABELS[name] ? label(name) : name);
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
const relink = () => { if (state.chan.all) for (const n of allSets()) linkSet(n); };
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
const chanChanged = (idx) => {
  if (chanGraded(idx)) return true;
  const e = state.chan.edits[idx];
  if (!e) return false;
  if (e.base && e.base.some((v, k) => v !== state.fw.channel_curves[idx][k])) return true;
  return !isIdent(e.points);
};
const changedChannels = () => [...Array(state.fw.channel_curves.length).keys()].filter(chanChanged);
// the set the preview applies: the target's
const previewSet = () => (state.current ? chanSet() : null);

let toneBusy = false, tonePending = false;
function requestTone() {
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
  chanEditor.draw();
  drawChannels();
  renderChannelPanel();
  renderPickers();
  renderSections();
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
    if (e.button !== 0 || !state.current) return;
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

const chanEditor = pointEditor($("chan-curve"), {
  points: () => (state.fw && state.current ? chanEdit(chanActive()).points : IDENT),
  transfer: () => state.current && state.curves && state.curves.channel_transfer &&
    state.curves.channel_transfer[chanActive()],
  color: () => (state.chan.all ? css("--text") : CH_COLORS[state.chan.channel]),
});

function drawChannels() {
  const cv = $("chan-effective"), ctx = cv.getContext("2d"), w = cv.width, h = cv.height;
  drawGrid(ctx, w, h);
  if (!state.fw || !state.current) return;
  // the three channel curves of the set: input = computed tone value, 0..0x4000
  const pts = (c) => c.map((v, i) => [(i / 255) * w, h - (v / 16384) * h]);
  chanSet().forEach((idx, k) => {
    polyline(ctx, pts(state.fw.channel_curves[idx]), CH_COLORS[k], 1, [6, 6]);
    polyline(ctx, pts(chanValues(idx)), CH_COLORS[k], state.chan.all || k === state.chan.channel ? 3 : 1.5);
  });
}

function renderChannelPanel() {
  if (!state.fw || !state.current) return;
  const cross = state.current === CROSS;
  $("chan-variant").hidden = !cross;
  for (const b of document.querySelectorAll("#chan-sets button")) {
    b.classList.toggle("on", b.dataset.set === state.chan.set);
    b.textContent = setLabel(b.dataset.set) + (chanSet(b.dataset.set).some(chanChanged) ? " •" : "");
  }
  document.querySelectorAll("#chan-channels button").forEach((b, k) => {
    b.classList.toggle("on", k === 0 ? state.chan.all : !state.chan.all && k - 1 === state.chan.channel);
    b.textContent = k === 0 ? "RGB" : CH_NAMES[k - 1] + (chanChanged(chanSet()[k - 1]) ? " •" : "");
  });
  $("chan-title").textContent = state.chan.all
    ? t("chan_all")
    : t("chan_one", { ch: CH_NAMES[state.chan.channel], up: t(["red", "green", "blue"][state.chan.channel]),
                       down: ["Cyan", "Magenta", LANG === "de" ? "Gelb" : "Yellow"][state.chan.channel] });
  renderWheels();
}

function buildChannelPanel() {
  const sets = $("chan-sets");
  sets.innerHTML = "";
  for (const name of SET_ORDER.filter((n) => state.fw.channel_sets[n])) {
    const b = document.createElement("button");
    b.dataset.set = name;
    b.textContent = setLabel(name);
    b.onclick = () => { state.chan.set = name; chanEditor.deselect(); renderChannelPanel(); requestTone(); };
    sets.appendChild(b);
  }
  document.querySelectorAll("#chan-channels button").forEach((b, k) => {
    if (k) b.style.color = CH_COLORS[k - 1];
    b.onclick = () => {
      if (k === 0 && !state.chan.all) {
        if (!linkSet(curSet()) &&
            !confirm(t("link_confirm"))) return;
        linkSet(curSet(), true);
        state.chan.all = true;
      } else if (k > 0) {
        if (state.chan.all) for (const n of allSets()) unlinkSet(n);
        state.chan.all = false;
        state.chan.channel = k - 1;
      }
      chanEditor.deselect(); renderChannelPanel(); requestTone(); drawChannels();
    };
  });
  const src = $("chan-source");
  src.innerHTML = "";
  for (const name of allSets().filter((n) => state.fw.channel_sets[n])) {
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
  srcSet.innerHTML = allSets().filter((n) => state.fw.channel_sets[n])
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
// back to where the curve started (an extra base's curve, or neutral)
$("chan-reset").onclick = () => {
  for (const idx of state.chan.all ? chanSet() : [chanActive()]) {
    const c0 = curveStart(idx);
    if (c0) state.chan.edits[idx] = { base: c0, points: IDENT.map((p) => p.slice()) };
    else delete state.chan.edits[idx];
  }
  relink();
  chanEditor.deselect();
  requestTone();
};

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
  renderSections();
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
      const w = gradeOf(curSet())[key];
      w.amount = Math.min(1, Math.hypot(dx, dy) / ((r.width / 2 - 2) / (r.width / 2)));
      w.hue = (Math.atan2(dy, dx) * 180 / Math.PI + 360) % 360;
      gradeChanged();
    };
    let drag = false;
    cv.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 || e.detail > 1 || !state.current) return;
      drag = true; cv.setPointerCapture(e.pointerId); at(e);
    });
    cv.addEventListener("pointermove", (e) => drag && at(e));
    cv.addEventListener("pointerup", () => { drag = false; });
    cv.addEventListener("dblclick", () => { Object.assign(gradeOf(curSet())[key], neutralWheel()); gradeChanged(); });
    lum.oninput = () => { gradeOf(curSet())[key].lum = +lum.value / 100; gradeChanged(); };
    lum.ondblclick = () => { gradeOf(curSet())[key].lum = 0; gradeChanged(); };
    wheelUI[key] = { el, cv, lum, out: el.querySelector("output") };
  }
  $("grade-balance").oninput = (e) => { gradeOf(curSet()).balance = +e.target.value / 100; gradeChanged(); };
}

function renderWheels() {
  if (!state.fw || !wheelUI.global || !state.current) return;
  const g = state.grade[curSet()] || normGrade({});
  // Cross Process: the wheels of the variant chosen under RGB curves
  $("grade-set").hidden = state.current !== CROSS;
  $("grade-set").textContent = t("wheels_for", { set: setLabel(curSet()) });
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
}

$("help-open").onclick = () => $("help").showModal();
for (const b of document.querySelectorAll(".credits-open")) b.onclick = () => $("credits").showModal();

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

// ---- free space -------------------------------------------------------------------

// the pool HSL sets, curves and the targets' colour sets share (core: space.rs)
let spaceKey = null, spaceBusy = false, spacePending = false;
async function requestSpace() {
  if (!state.fw) return;
  const looks = changedTargets().filter((n) => n !== CROSS && curvesForExport(n).length);
  const body = JSON.stringify({ hsl: hslForExport(), looks });
  if (body === spaceKey) return;
  if (spaceBusy) { spacePending = true; return; }
  spaceKey = body;
  spaceBusy = true;
  try {
    const res = await api("/api/space", { method: "POST", headers: { "Content-Type": "application/json" }, body });
    if (res.ok) renderSpace(await res.json());
  } finally {
    spaceBusy = false;
    if (spacePending) { spacePending = false; requestSpace(); }
  }
}
function renderSpace(r) {
  const kb = (b) => (b / 1024).toFixed(1);
  const text = !r.fits ? t("space_full", { hsl: r.hsl, rgb: r.rgb })
    : t("space_ok", { free: kb(r.free), total: kb(r.total), rgb: r.more_rgb, hsl: r.more_hsl });
  $("space-export").className = r.fits ? "note muted" : "note warn";
  $("space-export").textContent = text;
  $("space").classList.toggle("full", !r.fits);
  $("space").title = text;
  $("space-bar").style.width = `${Math.min(100, (100 * r.used) / r.total).toFixed(1)}%`;
  $("space-text").textContent = t("space_meter", { used: kb(r.used), total: kb(r.total) });
}

// ---- language -----------------------------------------------------------------

function setLang(lang) {
  LANG = lang;
  try { localStorage.setItem("lang", lang); } catch { /* no storage */ }
  applyI18n();
  if (!state.fw) return;
  $("fw-info").textContent = t("firmware", state.build);
  spaceKey = null;
  buildChannelPanel();
  renderToneNote();
  renderVignetteNote();
  renderWb();
  renderHsl();
  update();
  placeSplit();
}
for (const b of document.querySelectorAll("#lang button")) b.onclick = () => setLang(b.dataset.lang);
applyI18n();

// ---- start ------------------------------------------------------------------

// the beta of a branch is published under /beta/ (.github/workflows/pages.yml)
if (/\/beta\//.test(location.pathname)) {
  $("beta-badge").hidden = false;
  document.title += " (beta)";
}

// after every script has loaded (presets.js holds the export and preset parts)
window.addEventListener("DOMContentLoaded", async () => {
  try {
    initGL();
    state.fw = await (await api("/api/firmware")).json();
    for (const l of state.fw.looks) state.looks[l.name] = l;
    // the session starts with every look stock (Cross Process's own set is a
    // copy of its stock one): the matrices every target starts from
    state.stock = Object.fromEntries(state.fw.looks.map((l) => [l.name, l]));
    mergeLookSets();
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
    restoreSections();
    applyI18n();
    update();
    await fetchTone();
    const pre = await api("/api/dng");
    if (pre.status === 200) await show(await pre.json());
    window.dispatchEvent(new Event("editor-ready"));
  } catch (err) {
    $("fw-info").textContent = t("error_prefix", { msg: err.message });
  }
});
