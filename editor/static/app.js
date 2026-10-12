// GR III look editor — preview, matrix and tone curve editing.
//
// The preview runs the colour part of ColorMatrixTuning on the GPU:
//   camera RGB -> white balance -> sensor CCM -> look matrix -> (mono mix)
// followed by the camera's tone curve. That curve comes from the server, which
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
  shot: null,          // /api/dng meta
  tone: { points: [[0, 0], [1, 1]], families: [0, 1, 2] },
  params: { key: 0, contrast: 0, highlight: 0, shadow: 0, saturation: 0, hue: 0 },
  residual: true,
  curves: null,        // last /api/tone answer
  chan: { edits: {}, set: "Preset 1", channel: 0 },   // Cross Process channel curves
  grade: {},           // set name -> colour wheels, applied on the server after the edits
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
]);
const toneChanged = () => !state.tone.points.every(([x, y]) => Math.abs(x - y) < 1e-4);

// firmware names -> the names in the camera menu
const LOOK_LABELS = {
  Standard: "Standard", Vivid: "Vivid", MonotoneGR: "Monotone", SoftMonotone: "Soft Monotone",
  HardMonotone: "Hard Monotone", HighContrastBW: "Hi-Contrast B&W", PositiveFilm: "Positive Film",
  BleechBypass_GR: "Bleach Bypass", "BleechBypass_GR · Var. 2": "Bleach Bypass (Variante 2)",
  "BleechBypass_GR · Var. 3": "Bleach Bypass (Variante 3)", Retro: "Retro", HdrCuisine: "HDR Tone",
  "HdrCuisine · Var.": "HDR Tone (Variante)", CrossProcess: "Cross Process", NegativeFilm: "Negative Film",
};
const label = (name) => LOOK_LABELS[name] || name;
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
uniform bool uMono, uResidual;
uniform vec3 uWb;
uniform float uExp, uScale, uSplit;
uniform int uMode;   // 0 edit, 1 original|edit, 2 camera|edit, 3 camera

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

vec3 render(mat3 m, sampler2D curve) {
  vec3 c = texture(uImg, uv).rgb * uWb;
  c = max(m * c, 0.);
  if (uMono) c = vec3((c.r + c.g + c.b) / 3.);
  vec3 g = lut3(curve, c * uScale * uExp / 16.) / 16384.;
  g = uHue * g;
  if (uResidual) g = vec3(lut(uRes, clamp(g.r, 0., 1.) * 255.),
                          lut(uRes, clamp(g.g, 0., 1.) * 255.),
                          lut(uRes, clamp(g.b, 0., 1.) * 255.));
  return g;
}

void main() {
  vec3 cam = texture(uCam, uv).rgb;
  bool left = uv.x < uSplit;
  vec3 c;
  if (uMode == 3) c = cam;
  else if (uMode == 1 && left) c = render(uMRef, uLutRef);
  else if (uMode == 2 && left) c = cam;
  else c = render(uM, uLut);
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
  if (!gl) throw new Error("WebGL2 nicht verfügbar");
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
  gl.uniform3fv(uni("uWb"), state.shot.wb);
  gl.uniform1f(uni("uExp"), Math.pow(2, state.ev));
  gl.uniform1f(uni("uScale"), state.shot.scale);
  gl.uniform1f(uni("uSplit"), state.mode === "edit" || state.mode === "jpeg" ? -1 : state.split);
  gl.uniform1i(uni("uMode"), { edit: 0, ab: 1, camera: 2, jpeg: 3 }[state.mode]);
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
  $("label-l").textContent = state.mode === "ab" ? "Original" : "Kamera-JPEG";
  $("label-r").textContent = "Bearbeitet";
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
      (isChanged(look.name) ? "  ● geändert" : "") +
      (state.shot && state.shot.shot_look === look.name ? "  · Bildstil des Fotos" : "");
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
  $("sums").innerHTML = "Zeilensummen: " + sums
    .map((s) => `<span class="${Math.abs(s - 1) > 0.001 ? "bad" : ""}">${s.toFixed(4)}</span>`)
    .join(" · ") + (sums.some((s) => Math.abs(s - 1) > 0.001) ? ' <span class="bad">— Grau bekommt einen Stich</span>' : "");
  $("addr").textContent = `0x${look.address.toString(16)}`;
  const note = $("shared-note");
  note.hidden = !look.shared_with.length;
  note.className = "note warn";
  note.textContent = `Achtung: ${label(look.name)} teilt seine Farben mit ${look.shared_with.map(label).join(" und ")}. Änderungen gelten für alle.`;
  const rows = [0, 1, 2].map((r) => m.slice(r * 3, r * 3 + 3).map((v) => v.toFixed(4)).join());
  if (!look.mono_mix && rows[0] === rows[1] && rows[1] === rows[2]) {
    note.hidden = false;
    note.className = "note";
    note.textContent = "Schwarzweiß-Look: Die Regler bestimmen, wie hell einzelne Farben im Grau erscheinen (wie ein Farbfilter).";
  }
  if (look.mono_mix) {
    note.hidden = false;
    note.className = "note";
    note.textContent = "Schwarzweiß-Look: Die Kamera mischt die Farben danach zu Grau. Farbänderungen wirken hier kaum.";
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

function renderInherit() {
  const box = $("inherit-box");
  box.hidden = state.current !== INHERIT_TARGET;
  if (box.hidden) return;
  const src = (state.fw.inherit || {})[INHERIT_TARGET] || "";
  const sel = $("inherit-src");
  if (!sel.options.length) {
    sel.innerHTML = `<option value="">Cross Process (Original)</option>` +
      INHERIT_SOURCES.map((n) => `<option value="${n}">${label(n)}</option>`).join("");
  }
  sel.value = src;
  const p = state.shot && state.shot.shot_look === src && state.shot.params;
  $("inherit-note").textContent = src
    ? `Cross Process rechnet mit den Farben von ${label(src)}, dazu kommen deine RGB-Kurven. ` +
      `Für dasselbe Ergebnis in der Kamera bei Cross Process dieselben Regler wie bei ${label(src)} einstellen` +
      (p ? ` (laut DNG: Key ${signed(p.key || 0)}, Kontrast ${signed(p.contrast || 0)}, ` +
           `Lichter ${signed(p.highlight || 0)}, Schatten ${signed(p.shadow || 0)}, ` +
           `Sättigung ${signed(p.saturation || 0)}, Schärfe ${signed(p.sharpness || 0)})` : "") +
      ` und die Farbton-Variante wählen, deren Kanalkurven du bearbeitet hast.`
    : "Optional: Cross Process mit den Farben eines anderen Bildstils rechnen lassen.";
}

$("inherit-src").onchange = async (e) => {
  const res = await fetch("/api/inherit", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ target: INHERIT_TARGET, source: e.target.value || null }),
  });
  if (!res.ok) { renderInherit(); return; }
  const fw = await res.json();
  Object.assign(state.fw, { looks: fw.looks, inherit: fw.inherit });
  for (const l of fw.looks) state.looks[l.name] = l;
  update();
  requestTone();
};

function renderChanges() {
  if (!$("downloads").hidden && state.lastExport !== exportKey()) {
    $("downloads").hidden = true;
    $("export").classList.add("primary");
    $("export-msg").textContent = "";
  }
  const looks = state.fw.looks.map((l) => l.name).filter(isChanged);
  const names = looks.map((n) => `Farben: ${label(n)}`);
  if (toneChanged()) names.push("Tonkurve (alle Bildstile)");
  const sets = new Set(changedChannels().map(chanSetOf));
  for (const n of SET_ORDER) if (sets.has(n)) names.push(`Cross Process ${setLabel(n)}: RGB-Kurven` +
    (gradedSets().includes(n) ? " und Farbräder" : ""));
  for (const [t, src] of Object.entries(state.fw.inherit || {})) names.push(`${label(t)} mit den Farben von ${label(src)}`);
  $("changes").innerHTML = names.length
    ? names.map((n) => `<li>${n}</li>`).join("")
    : '<li class="muted">Noch keine Änderungen</li>';
  $("export").disabled = !names.length;
  const badge = { color: looks.length > 0,
    tone: toneChanged(), cross: sets.size > 0 || Object.keys(state.fw.inherit || {}).length > 0,
    export: names.length > 0 };
  for (const b of document.querySelectorAll("#tabs button")) b.classList.toggle("dirty", !!badge[b.dataset.tab]);
}
const chanSetOf = (idx) => Object.keys(state.fw.channel_sets).find((n) => state.fw.channel_sets[n].includes(idx));

// ---- tabs -------------------------------------------------------------------

let lookBeforeCross = null;
function showTab(tab) {
  for (const b of document.querySelectorAll("#tabs button")) b.classList.toggle("on", b.dataset.tab === tab);
  for (const s of document.querySelectorAll("section.tab")) s.hidden = s.dataset.tab !== tab;
  store.set("tab", tab);
  document.querySelector("aside").dataset.tab = tab;
  // the Cross Process tools only act on that look: preview it while they are open
  if (tab === "cross" && state.current !== "CrossProcess") {
    lookBeforeCross = state.current;
    select("CrossProcess");
  } else if (tab !== "cross" && tab !== "export" && lookBeforeCross && state.current === "CrossProcess") {
    select(lookBeforeCross);
    lookBeforeCross = null;
  }
  if (tab === "tone") { toneEditor.draw(); drawEffective(); }
  if (tab === "cross") { chanEditor.draw(); drawChannels(); renderWheels(); }
}
for (const b of document.querySelectorAll("#tabs button")) b.onclick = () => showTab(b.dataset.tab);

function update() {
  renderMatrix();
  renderLooks();
  renderChanges();
  draw();
}

function select(name) {
  if (name !== "CrossProcess") lookBeforeCross = null;
  const wasCross = state.current === "CrossProcess";
  state.current = name;
  update();
  if (wasCross !== (name === "CrossProcess")) requestTone();
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
  const names = { r: ["Rot", "#e0645a"], g: ["Grün", "#6fbf73"], b: ["Blau", "#5b8fe0"] };
  PRIMS.forEach((p, n) => {
    if (n) box.insertAdjacentHTML("beforeend", '<span class="gap"></span>');
    for (const [k, label] of [["hue", "Farbton"], ["sat", "Sättigung"]]) {
      const head = document.createElement("span");
      head.innerHTML = k === "hue" ? `<span class="prim" style="color:${names[p][1]}">${names[p][0]}</span> <span class="lbl">${label}</span>`
                                   : `<span class="lbl">${label}</span>`;
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
$("reset").onclick = () => {
  const look = state.looks[state.current];
  state.edits[look.address] = { base: look.matrix.slice(), sat: 1, hue: 0, calib: neutralCalib() };
  update();
};

$("export").onclick = async () => {
  const matrices = {};
  for (const l of state.fw.looks) if (isChanged(l.name)) matrices[l.name] = effective(l.name);
  $("export-msg").className = "note";
  $("export-msg").textContent = "Erstelle Update-Datei …";
  const res = await fetch("/api/export", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      matrices,
      tone: toneChanged() ? state.tone : null,
      channels: Object.fromEntries(changedChannels().map((i) => [i, chanValues(i)])),
      matrix_edits: Object.fromEntries(state.fw.looks.filter((l) => isChanged(l.name))
        .map((l) => [l.name, editOf(l.name)])),
      channels_pre: gradedSets().length ? Object.fromEntries(changedChannels().map((i) => [i, chanPre(i)])) : null,
      grade: gradedSets().length ? Object.fromEntries(gradedSets().map((n) => [n, state.grade[n]])) : null,
      update_file: !!state.fw.container,
    }),
  });
  if (res.ok) {
    const r = await res.json();
    $("export-msg").className = "note ok";
    $("export-msg").textContent = r.update_file
      ? "Fertig. Die Datei wurde geprüft (Prüfsummen ok) und liegt auch unter " + r.update_file + "."
      : `Geschrieben: ${r.written}`;
    state.lastExport = exportKey();
    offerDownloads(r.downloads || {}, r.update_file);
  } else {
    $("export-msg").className = "note warn";
    $("export-msg").textContent = await errorText(res);
  }
};

function offerDownloads(urls, updateFile) {
  // a fresh query string per export, so the browser never hands out a cached older file
  const stamp = Date.now();
  const set = (id, url, name) => {
    const a = $(id);
    a.hidden = !url;
    if (url) { a.href = `${url}?t=${stamp}`; a.download = name || ""; }
  };
  set("dl-update", urls.update, updateFile && updateFile.split("/").pop());
  set("dl-preset", urls.preset, "mein-look.looks.json");
  $("downloads").hidden = !urls.update && !urls.preset;
  $("export").classList.toggle("primary", $("downloads").hidden);
  if (urls.update) $("dl-update").focus();
}

async function errorText(res) {
  const t = await res.text();
  const m = t.match(/<p>(.*?)<\/p>/s);
  return m ? m[1] : `Fehler ${res.status}`;
}

// ---- loading a DNG ----------------------------------------------------------

async function loadDng(file) {
  $("busy").hidden = false;
  try {
    const fd = new FormData();
    fd.append("file", file);
    const res = await fetch("/api/dng", { method: "POST", body: fd });
    if (!res.ok) throw new Error(await errorText(res));
    await show(await res.json());
  } catch (err) {
    alert(`Das Foto konnte nicht geladen werden: ${err.message}`);
  } finally {
    $("busy").hidden = true;
  }
}

async function show(meta) {
  const lin = new Float32Array(await (await fetch("/api/dng/linear")).arrayBuffer());

  canvas.width = meta.width;
  canvas.height = meta.height;
  tex.img = texture(0, () =>
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGB32F, meta.width, meta.height, 0, gl.RGB, gl.FLOAT, lin));
  const res = new Float32Array(meta.residual || Array.from({ length: 256 }, (_, i) => i / 255));
  tex.res = texture(4, () =>
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, 256, 1, 0, gl.RED, gl.FLOAT, res));

  let cam = null;
  const camRes = await fetch("/api/dng/camera.jpg");
  if (camRes.ok) cam = await createImageBitmap(await camRes.blob());
  tex.cam = texture(2, () => {
    if (cam) gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, cam);
    else gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(4));
  });
  for (const b of document.querySelectorAll('#modes [data-mode="camera"], #modes [data-mode="jpeg"]'))
    b.disabled = !cam;
  if (!cam && (state.mode === "camera" || state.mode === "jpeg")) $("modes").querySelector("button").click();

  state.shot = meta;
  $("empty").hidden = true;
  $("shot-info").textContent = [meta.filename, meta.shot_look ? label(meta.shot_look) : meta.image_tone]
    .filter(Boolean).join(" · ");
  const p = meta.params || {};
  for (const k of [...TONE_KEYS, ...COLOR_KEYS]) {
    // Cross Process: the DNG's saturation value does not act like A[sat]
    // in measurements (MR024511/12 fit better without), so start at 0
    state.params[k] = k === "saturation" && meta.shot_look === "CrossProcess" ? 0 : p[k] || 0;
    $(`p-${k}`).value = state.params[k];
    $(`p-${k}-out`).textContent = signed(state.params[k]);
  }
  $("residual").disabled = !meta.residual;
  $("tone-note").className = "note";
  $("tone-note").textContent = (Object.keys(p).length
    ? "Aus dem Foto übernommen. "
    : "Keine Einstellungen im Foto gefunden, Mittelstellung. ") +
    (meta.residual ? "Die Vorschau ist am Kamera-JPEG abgeglichen." : "Ohne Kamera-JPEG: Standardwerte.");
  if (meta.shot_look) state.current = meta.shot_look;
  if (meta.shot_look === "CrossProcess" && p.cross_variant) {
    state.chan.set = `Preset ${p.cross_variant}`;
    renderWheels();
  }
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
const SET_LABEL = { "Preset 1": "Farbton 1", "Preset 2": "Farbton 2", "Preset 3": "Farbton 3",
  Zufall: "Zufall" };
const setLabel = (name) => SET_LABEL[name] || name;
const SET_ORDER = ["Preset 1", "Preset 2", "Preset 3", "Zufall"];
const chanActive = () => chanSet()[state.chan.channel];
function chanEdit(idx) {
  if (!state.chan.edits[idx]) state.chan.edits[idx] = { base: null, points: IDENT.map((p) => p.slice()) };
  return state.chan.edits[idx];
}
const chanValues = (idx) => (state.curves && state.curves.channel_curves
  ? state.curves.channel_curves[idx] : state.fw.channel_curves[idx]);
const WHEELS = [["shadows", "Schatten"], ["midtones", "Mitteltöne"], ["highlights", "Lichter"], ["global", "Global"]];
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
  return `Kurve ${idx}`;
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
  const res = await fetch("/api/tone", {
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
  color: () => CH_COLORS[state.chan.channel],
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
    polyline(ctx, pts(chanValues(idx)), CH_COLORS[k], k === state.chan.channel ? 4 : 2);
  });
}

function renderChannelPanel() {
  if (!state.fw) return;
  for (const b of document.querySelectorAll("#chan-sets button"))
    b.classList.toggle("on", b.dataset.set === state.chan.set);
  document.querySelectorAll("#chan-channels button").forEach((b, k) => {
    b.classList.toggle("on", k === state.chan.channel);
    b.textContent = CH_NAMES[k] + (chanChanged(chanSet()[k]) ? " •" : "");
  });
  $("chan-title").textContent = `${setLabel(state.chan.set)}, Kanal ${CH_NAMES[state.chan.channel]}. Klicken setzt einen Punkt, Ziehen verschiebt ihn.`;
  $("chan-panel").classList.toggle("inactive", state.current !== "CrossProcess");
  renderWheels();
  $("chan-note").className = state.chan.set === "Zufall" ? "note warn" : "note";
  $("chan-note").textContent = state.chan.set === "Zufall"
    ? "„Zufall“ ist in der Kamera nicht auswählbar; sie verteilt diese Kurven zufällig auf R, G und B."
    : "";
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
    b.style.color = CH_COLORS[k];
    b.onclick = () => { state.chan.channel = k; chanEditor.deselect(); renderChannelPanel(); chanEditor.draw(); drawChannels(); };
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
  chanEditor.deselect();
  requestTone();
};
$("chan-copy-set").onclick = () => {
  const from = chanSet($("chan-source-set").value), to = chanSet();
  const vals = from.map((idx) => chanValues(idx).slice());
  to.forEach((idx, k) => {
    if (idx !== from[k]) state.chan.edits[idx] = { base: vals[k], points: IDENT.map((p) => p.slice()) };
  });
  chanEditor.deselect();
  requestTone();
};
function setLinear(indices) {
  for (const idx of indices)
    state.chan.edits[idx] = { base: state.fw.channel_linear.slice(), points: IDENT.map((p) => p.slice()) };
  chanEditor.deselect();
  requestTone();
}
$("chan-linear").onclick = () => setLinear(chanSet());
$("chan-linear-all").onclick = () => setLinear([...Array(12).keys()]);
$("chan-reset").onclick = () => { delete state.chan.edits[chanActive()]; chanEditor.deselect(); requestTone(); };
$("chan-reset-set").onclick = () => {
  for (const idx of chanSet()) delete state.chan.edits[idx];
  delete state.grade[state.chan.set];
  chanEditor.deselect();
  requestTone();
};

// ---- loading a preset ---------------------------------------------------------

// A preset is the *.looks.json an export writes:
//   { source, matrices: {look: [9]}, tone: {points, families} | null,
//     channel_curves: {idx: [256]} | null, inherit: {target: source} | null }
// It replaces every current edit. Look data inheritance goes first, because
// it changes which matrix address a look edits.
async function loadPreset(file) {
  const msg = $("preset-msg");
  msg.className = "note";
  try {
    let p;
    try { p = JSON.parse(await file.text()); } catch { throw new Error("keine gültige JSON-Datei"); }
    if (!p || typeof p !== "object" || !("matrices" in p || "tone" in p || "channel_curves" in p || "inherit" in p))
      throw new Error("kein Look-Editor-Preset");
    const warn = [];

    const inherit = p.inherit || {};
    const targets = new Set([...Object.keys(state.fw.inherit || {}), ...Object.keys(inherit)]);
    for (const target of targets) {
      const source = inherit[target] || null;
      if (((state.fw.inherit || {})[target] || null) === source) continue;
      const res = await fetch("/api/inherit", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ target, source }),
      });
      if (!res.ok) { warn.push(`Übernahme ${target} ← ${source}: ${await errorText(res)}`); continue; }
      const fw = await res.json();
      Object.assign(state.fw, { looks: fw.looks, inherit: fw.inherit });
      for (const l of fw.looks) state.looks[l.name] = l;
    }

    state.edits = {};
    // newer presets keep base + sliders (incl. calibration) per look
    for (const [name, e] of Object.entries(p.matrix_edits || {})) {
      if (!state.looks[name] || !Array.isArray(e.base) || e.base.length !== 9) continue;
      const cal = neutralCalib();
      for (const q of PRIMS) for (const k of ["hue", "sat"])
        cal[q][k] = Math.max(-100, Math.min(100, +(((e.calib || {})[q] || {})[k]) || 0));
      Object.assign(editOf(name), { base: e.base.slice(), sat: +e.sat || 1, hue: +e.hue || 0, calib: cal });
    }
    for (const [name, m] of Object.entries(p.matrices || {})) {
      if (p.matrix_edits && p.matrix_edits[name]) continue;
      if (!state.looks[name]) { warn.push(`unbekannter Bildstil ${name}`); continue; }
      if (!Array.isArray(m) || m.length !== 9 || m.some((v) => !Number.isFinite(v))) {
        warn.push(`Matrix ${name}: 9 Zahlen erwartet`); continue;
      }
      Object.assign(editOf(name), { base: m.slice(), sat: 1, hue: 0 });
    }

    const tone = p.tone || {};
    const pts = Array.isArray(tone.points) && tone.points.length >= 2 &&
      tone.points.every((q) => Array.isArray(q) && q.length === 2 && q.every(Number.isFinite))
      ? tone.points.map((q) => q.slice()).sort((a, b) => a[0] - b[0]) : null;
    if (p.tone && !pts) warn.push("Tonkurve unlesbar");
    state.tone.points = pts || IDENT.map((q) => q.slice());
    state.tone.families = Array.isArray(tone.families) ? tone.families.filter((f) => [0, 1, 2].includes(f)) : [0, 1, 2];
    for (const cb of document.querySelectorAll(".fam")) cb.checked = state.tone.families.includes(+cb.value);

    state.chan.edits = {};
    state.grade = {};
    let channels = 0;
    // with wheels the preset keeps the curves before grading, so the wheels stay editable
    const pre = p.channel_grade && p.channel_curves_pre;
    if (pre) {
      for (const [n, g] of Object.entries(p.channel_grade)) {
        if (!state.fw.channel_sets[n]) { warn.push(`unbekannter Satz ${n}`); continue; }
        state.grade[n] = normGrade(g);
      }
    }
    for (const [idx, vals] of Object.entries((pre ? p.channel_curves_pre : p.channel_curves) || {})) {
      const i = +idx;
      if (!(i >= 0 && i < 12) || !Array.isArray(vals) || vals.length !== 256 || vals.some((v) => !Number.isFinite(v))) {
        warn.push(`Kanalkurve ${idx}: 256 Werte erwartet`); continue;
      }
      state.chan.edits[i] = { base: vals.map((v) => Math.round(v)), points: IDENT.map((q) => q.slice()) };
      channels++;
    }

    toneEditor.deselect();
    chanEditor.deselect();
    renderWheels();
    update();
    requestTone();
    if (p.source && state.fw.path && !p.source.endsWith(state.fw.path))
      warn.push(`erstellt für ${p.source}`);
    const parts = [
      Object.keys(p.matrices || {}).length && `${Object.keys(p.matrices).length} Matrizen`,
      pts && !isIdent(pts) && "Tonkurve",
      channels && `${channels} Kanalkurven`,
      gradedSets().length && `Farbräder ${gradedSets().map(setLabel).join(", ")}`,
      Object.keys(inherit).length && Object.entries(inherit).map(([t, s]) => `${t} ← ${s}`).join(", "),
    ].filter(Boolean);
    msg.className = warn.length ? "note warn" : "note";
    msg.textContent = `Preset „${file.name}“ geladen: ${parts.join(" · ") || "keine Änderungen"}` +
      (warn.length ? ` — Hinweise: ${warn.join("; ")}` : "");
  } catch (err) {
    msg.className = "note warn";
    msg.textContent = `Preset konnte nicht geladen werden: ${err.message}`;
  }
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
    el.innerHTML = `<div class="title"><span>${label}</span><output></output></div>
      <canvas width="280" height="280"></canvas>
      <input type="range" min="-100" max="100" step="1" value="0" title="Luminanz">`;
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
  $("grade-note").textContent = state.chan.set === "Zufall"
    ? "Für Farbräder besser Farbton 1–3 verwenden: „Zufall“ verteilt die Kurven zufällig."
    : "";
}

$("help-open").onclick = () => $("help").showModal();
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
    state.fw = await (await fetch("/api/firmware")).json();
    for (const l of state.fw.looks) state.looks[l.name] = l;
    state.current = "Standard";
    const build = state.fw.path.split("/").slice(-2, -1)[0] || "";
    const VERSIONS = { fwdc239b: "2.10" };
    $("fw-info").textContent = `Firmware ${VERSIONS[build] || "?"} (${build})`;
    if (!store.get("helpSeen")) { $("help").showModal(); store.set("helpSeen", "1"); }
    buildChannelPanel();
    buildWheels();
    renderWheels();
    update();
    await fetchTone();
    const pre = await fetch("/api/dng");
    if (pre.status === 200) await show(await pre.json());
    const tab = store.get("tab");
    if (tab && document.querySelector(`#tabs [data-tab="${tab}"]`)) showTab(tab);
  } catch (err) {
    $("fw-info").textContent = `Fehler: ${err.message}`;
  }
})();
