// Fit to a LUT (docs/plan-spektrafilm-fitter.md): the core fits the selected
// target's look -- matrix, RGB curves, HSL and the post matrix -- to a 3D LUT
// (.cube, lutfit.rs), and the editor takes the result like a preset's target:
// source Standard, every part editable afterwards. The LUT itself stays in
// memory for the "Compare LUT" view; the preset only records which one it was
// (edit.lut: name, title, sha256, input, report).
//
// The fit blocks the worker for some seconds; the core reports each pass.

const LUT_TARGETS = ["Standard", "Vivid", "PositiveFilm", "Retro", "NegativeFilm"];
const LUT_STEPS = { "matrix and curves": "lut_s_curves", "HSL and post matrix": "lut_s_hsl", "HSL encoded": "lut_s_encoded" };
const lut = {
  name: null, text: null, info: null, data: null,   // the chosen .cube
  shown: {},        // target -> { name, info, data, input }: the LUT the preview compares with
  busy: false,
};

// the pick panel's button and the preview's LUT mode follow the selected target
function renderLutButton() {
  const T = state.current;
  const b = $("lut-open");
  b.hidden = !T;
  b.disabled = !T || !LUT_TARGETS.includes(T);
  b.title = T && b.disabled ? t("lut_not_here") : t("lut_open_t");
  const m = document.querySelector('#modes [data-mode="lut"]');
  m.disabled = !(T && lut.shown[T] && state.shot);
  if (m.disabled && state.mode === "lut") $("modes").querySelector("button").click();
}

// the input select: grlog keeps the file's own range ("grlog -8 12")
function lutInput() {
  const v = $("lut-input").value;
  const own = lut.info && lut.info.input;
  return v === "grlog" && own && own.startsWith("grlog") ? own : v;
}

function renderLutDialog() {
  const T = state.current;
  $("lut-target").textContent = T ? label(T) : "—";
  $("lut-file-name").textContent = lut.name || t("lut_pick");
  $("lut-fit").disabled = lut.busy || !lut.text || !T || !LUT_TARGETS.includes(T);
  for (const id of ["lut-file", "lut-input", "lut-post", "lut-hsl"]) $(id).disabled = lut.busy;
  $("lut-dlg").classList.toggle("busy", lut.busy);
}

function lutMsg(text, kind = "") {
  $("lut-msg").className = `note ${kind}`;
  $("lut-msg").textContent = text;
}

$("lut-open").onclick = () => {
  renderLutDialog();
  $("lut-report").hidden = true;
  lutMsg(lut.text ? t("lut_ready", { size: lut.info.size, title: lut.info.title || lut.name }) : "");
  $("lut-dlg").showModal();
};

$("lut-file").onchange = async (e) => {
  const f = e.target.files[0];
  e.target.value = "";
  if (!f) return;
  Object.assign(lut, { name: null, text: null, info: null, data: null });
  renderLutDialog();
  lutMsg(t("lut_reading"));
  try {
    const text = await f.text();
    const r = await api.lut(text);
    Object.assign(lut, { name: f.name, text, info: r.meta, data: r.data });
    const inp = r.meta.input || "display";
    $("lut-input").value = inp.startsWith("grlog") ? "grlog" : inp;
    lutMsg(t("lut_ready", { size: r.meta.size, title: r.meta.title || f.name }) +
      (r.meta.input ? " " + t("lut_input_file") : ""));
  } catch (err) {
    lutMsg(t("lut_bad", { msg: err.message }), "warn");
  }
  renderLutDialog();
};

$("lut-fit").onclick = async () => {
  const T = state.current;
  if (!T || !lut.text || lut.busy) return;
  const body = { name: lut.name, target: T, input: lutInput(), post: $("lut-post").checked, hsl: $("lut-hsl").checked };
  const chosen = { name: lut.name, info: lut.info, data: lut.data, input: body.input };
  lut.busy = true;
  renderLutDialog();
  $("lut-report").hidden = true;
  const t0 = performance.now();
  let step = t("lut_s_start");
  const tick = () => lutMsg(t("lut_running", { step, s: Math.round((performance.now() - t0) / 1000) }));
  tick();
  const timer = setInterval(tick, 1000);
  try {
    const r = await api.lutFit(lut.text, body, (s) => { step = LUT_STEPS[s] ? t(LUT_STEPS[s]) : s; tick(); });
    applyLutFit(T, r.spec);
    lut.shown[T] = chosen;
    renderLutReport(r.report);
    lutMsg(t("lut_done", { look: label(T), s: Math.round((performance.now() - t0) / 1000) }), "ok");
    if (state.shot) { state.mode = "lut"; markMode(); }
  } catch (err) {
    lutMsg(t("lut_failed", { msg: err.message }), "warn");
  } finally {
    clearInterval(timer);
    lut.busy = false;
    renderLutDialog();
    update();
  }
};

// the target starts over: the fit replaces its source, colour, curves, wheels and HSL
function applyLutFit(T, spec) {
  delete state.targets[T];
  delete state.edits[T];
  for (const i of targetChannels(T)) delete state.chan.edits[i];
  for (const n of setsOf(T)) delete state.grade[n];
  delete hsl.looks[T];
  const warn = [], hslIn = {};
  applyTarget(T, spec, warn, hslIn);
  warn.push(...hslLoad(hslIn, true));
  relink();
  chanEditor.deselect();
  select(T);
  if (warn.length) notice(t("preset_hints", { list: warn.join("; ") }), true);
}

// ΔE2000 of the fit on colours it was not fitted on (lutfit.rs metrics)
function renderLutReport(r) {
  const rows = [["matrix_curves", "lut_r_curves"], ["model", "lut_r_model"], ["final", "lut_r_final"]].filter(([k]) => r[k]);
  const cols = ["all", "p95", "chart", "skin", "saturated", "grey"];
  const f = (v) => (Number.isFinite(v) ? v.toFixed(2) : "–");
  $("lut-report").innerHTML =
    `<thead><tr><th></th>${cols.map((c) => `<th>${esc(t(`lut_c_${c}`))}</th>`).join("")}</tr></thead><tbody>` +
    rows.map(([k, key]) => `<tr${k === "final" ? ' class="final"' : ""}><th>${esc(t(key))}</th>${cols.map((c) => `<td>${f(r[k][c])}</td>`).join("")}</tr>`).join("") +
    "</tbody>";
  $("lut-report").hidden = false;
  $("lut-report-note").hidden = false;
}

// ---- the preview's LUT side -------------------------------------------------------

let cubeTex = null, cubeOf = null, remapOf = null;

// tone index of the photo's curve -> the index where Standard's default curve
// (the fitter's) gives the same value; 256 entries
function remapTable(shot, std) {
  const out = new Float32Array(256);
  let j = 0;
  for (let k = 0; k < 256; k++) {
    const y = shot[k];
    while (j < 254 && std[j + 1] < y) j++;
    const a = std[j], b = std[j + 1];
    out[k] = y <= a ? j : y >= b ? j + 1 : j + (y - a) / (b - a);
  }
  return out;
}

// samplers on their own units, always (a sampler3D left on unit 0 would clash
// with the photo); the LUT and Standard's tone curve once per chosen LUT
function lutUniforms() {
  gl.uniform1i(uni("uCube"), 6);
  gl.uniform1i(uni("uStd"), 7);
  gl.uniform1i(uni("uRemap"), 8);
  const L = state.current && lut.shown[state.current];
  if (!L) return;
  const shot = state.curves && state.curves.lut;
  if (shot && (remapOf !== shot || cubeOf !== L)) {
    const r = remapTable(shot, L.info.tone);
    if (tex.remap) gl.deleteTexture(tex.remap);
    tex.remap = texture(8, () => gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, 256, 1, 0, gl.RED, gl.FLOAT, r));
    remapOf = shot;
  }
  const n = L.info.size;
  if (cubeOf !== L) {
    gl.activeTexture(gl.TEXTURE6);
    if (!cubeTex) cubeTex = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_3D, cubeTex);
    for (const p of [gl.TEXTURE_MIN_FILTER, gl.TEXTURE_MAG_FILTER]) gl.texParameteri(gl.TEXTURE_3D, p, gl.LINEAR);
    for (const p of [gl.TEXTURE_WRAP_S, gl.TEXTURE_WRAP_T, gl.TEXTURE_WRAP_R]) gl.texParameteri(gl.TEXTURE_3D, p, gl.CLAMP_TO_EDGE);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage3D(gl.TEXTURE_3D, 0, gl.RGB16F, n, n, n, 0, gl.RGB, gl.FLOAT, L.data);
    const tone = new Float32Array(256 * 4);
    L.info.tone.forEach((v, i) => tone.set([v, v, v, 1], 4 * i));
    if (tex.std) gl.deleteTexture(tex.std);
    tex.std = texture(7, () => gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA32F, 256, 1, 0, gl.RGBA, gl.FLOAT, tone));
    cubeOf = L;
  }
  const g = /^grlog\s+(\S+)\s+(\S+)/.exec(L.input);
  gl.uniform1i(uni("uCubeIn"), L.input === "display" ? 0 : L.input === "linear" ? 1 : 2);
  gl.uniform2fv(uni("uCubeLog"), g ? [+g[1], +g[2]] : [-8, 12]);
  gl.uniform1f(uni("uCubeN"), n);
  gl.uniform1f(uni("uCubeScale"), L.info.scale);
  gl.uniform3fv(uni("uCubeMin"), L.info.min);
  gl.uniform3fv(uni("uCubeMax"), L.info.max);
  gl.uniformMatrix3fv(uni("uSensor"), false, transpose(state.fw.sensor_ccm));
  gl.uniformMatrix3fv(uni("uStdM"), false, transpose(L.info.standard));
}
