// Fit to a LUT (docs/plan-spektrafilm-fitter.md): the core fits the selected
// target's look -- matrix, RGB curves, HSL and the post matrix -- to a 3D LUT
// (.cube, lutfit.rs), and the editor takes the result like a preset's target:
// source Standard (the picker shows the LUT), every part editable afterwards.
// The preset records which LUT it was (edit.lut: name, title, sha256, input,
// report), not the LUT itself.
//
// The fit blocks the worker for some seconds; the core reports each pass.

const LUT_TARGETS = ["Standard", "Vivid", "PositiveFilm", "Retro", "NegativeFilm"];
const LUT_STEPS = { "matrix and curves": "lut_s_curves", "HSL and post matrix": "lut_s_hsl", "HSL encoded": "lut_s_encoded" };
const lut = {
  name: null, text: null, info: null,   // the chosen .cube
  busy: false,
};

// the pick panel's button follows the selected target
function renderLutButton() {
  const T = state.current;
  const b = $("lut-open");
  b.hidden = !T;
  b.disabled = !T || !LUT_TARGETS.includes(T);
  b.title = T && b.disabled ? t("lut_not_here") : t("lut_open_t");
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
  Object.assign(lut, { name: null, text: null, info: null });
  renderLutDialog();
  lutMsg(t("lut_reading"));
  try {
    const text = await f.text();
    const info = await api.lut(text);
    Object.assign(lut, { name: f.name, text, info });
    const inp = info.input || "display";
    $("lut-input").value = inp.startsWith("grlog") ? "grlog" : inp;
    lutMsg(t("lut_ready", { size: info.size, title: info.title || f.name }) +
      (info.input ? " " + t("lut_input_file") : ""));
  } catch (err) {
    lutMsg(t("lut_bad", { msg: err.message }), "warn");
  }
  renderLutDialog();
};

$("lut-fit").onclick = async () => {
  const T = state.current;
  if (!T || !lut.text || lut.busy) return;
  const body = { name: lut.name, target: T, input: lutInput(), post: $("lut-post").checked, hsl: $("lut-hsl").checked };
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
    renderLutReport(r.report);
    lutMsg(t("lut_done", { look: label(T), s: Math.round((performance.now() - t0) / 1000) }), "ok");
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
