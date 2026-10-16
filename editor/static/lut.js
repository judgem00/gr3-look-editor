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
  for (const id of ["lut-file", "lut-input", "lut-post", "lut-hsl", "lut-hc"]) $(id).disabled = lut.busy;
  $("lut-dlg").classList.toggle("busy", lut.busy);
}

function lutMsg(text, kind = "") {
  $("lut-msg").className = `note ${kind}`;
  $("lut-msg").textContent = text;
}

$("lut-open").onclick = () => {
  $("lut-hc").checked = hcOf(state.current);
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
  const body = { name: lut.name, target: T, input: lutInput(), post: $("lut-post").checked, hsl: $("lut-hsl").checked,
    hc: $("lut-hc").checked };
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
    setHc(body.hc);
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

// the target starts over: the fit replaces its source, colour, curves, wheels and HSL;
// its grain (a picker of its own) stays
function applyLutFit(T, spec) {
  const grain = state.targets[T] ? state.targets[T].grain : 0;
  delete state.targets[T];
  delete state.edits[T];
  for (const i of targetChannels(T)) delete state.chan.edits[i];
  for (const n of setsOf(T)) delete state.grade[n];
  delete hsl.looks[T];
  const warn = [], hslIn = {};
  applyTarget(T, spec, warn, hslIn);
  if (spec.grain == null) targetSpec(T).grain = grain || 0;
  lutStartFrom(T, spec);
  warn.push(...hslLoad(hslIn, true));
  relink();
  chanEditor.deselect();
  select(T);
  if (warn.length) notice(t("preset_hints", { list: warn.join("; ") }), true);
}

// what a fitted target starts from (targets.js lutStart): the fit's matrix,
// curves and post matrix, kept in edit.lut.start so a preset keeps it too
function lutStartFrom(T, spec) {
  const s = targetSpec(T);
  if (!s.lut) return;
  const m = (spec.matrices && spec.matrices[T]) || (spec.edit && spec.edit.matrix && spec.edit.matrix.base);
  const idx = targetChannels(T);
  const curves = Array.isArray(spec.curves) && spec.curves.length === idx.length
    ? Object.fromEntries(idx.map((i, k) => [i, spec.curves[k].map((v) => Math.round(v))])) : null;
  s.lut.start = { matrix: m ? m.slice() : null, curves, post: spec.post ? spec.post.slice() : null };
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

// ---- the film library ---------------------------------------------------------------
// films/spektrafilm.json (core/examples/film_library.rs): every spektrafilm
// negative on every print medium and every slide, fitted once. A fit does not
// depend on the target, so any LUT target can take any of them at once.
// Derived from spektrafilm by Andrea Volpato, CC BY-SA 4.0 (films/LICENSE.txt).

// films/spektrafilm-hc.json is the same library fitted for the camera's
// highlight correction (docs/plan-highlight-correction.md); a film target
// takes its looks from the library of its setting and records it (lut.hc).
const films = { libs: { off: null, on: null }, byFilm: { off: {}, on: {} }, lib: null };

async function loadLibrary(key, file) {
  try {
    const res = await fetch(`films/${file}`);
    if (!res.ok) return;
    const lib = await res.json();
    films.libs[key] = lib;
    films.byFilm[key] = {};
    for (const l of lib.looks) (films.byFilm[key][l.film] ||= []).push(l);
    films.lib = films.libs.off || films.libs.on;
    if (state.fw) renderPickers();
  } catch { /* no library: no film group */ }
}
function loadFilms() {
  loadLibrary("off", "spektrafilm.json");
  loadLibrary("on", "spektrafilm-hc.json");
}
window.addEventListener("editor-ready", loadFilms);

// the highlight correction setting new fits are made for (the pick panel's
// row; a loaded DNG sets it from its makernote)
const hcState = { on: store.get("hc") === "on" };
function setHc(on) {
  hcState.on = !!on;
  store.set("hc", hcState.on ? "on" : "off");
}
// a target fitted to a film or a LUT: the setting it was fitted for; else the current one
const hcOf = (T) => {
  const l = T && state.targets[T] && state.targets[T].lut;
  return l ? !!l.hc : hcState.on;
};
const hcKey = (on) => (on ? "on" : "off");

const stockName = (id) => (films.lib && films.lib.names[id]) || id;
const filmLooks = (film, on = hcState.on) => films.byFilm[hcKey(on)][film] || [];
const filmOf = (T) => {
  const l = state.targets[T] && state.targets[T].lut;
  return l && l.library === "spektrafilm" ? l : null;
};
const deText = (v) => (Number.isFinite(v) ? v.toFixed(2) : "–");

// the source picker's film group: one entry per film (its own print first)
function filmOptions(T, opt) {
  const on = hcOf(T);
  if (!films.libs[hcKey(on)] || !LUT_TARGETS.includes(T)) return "";
  const list = Object.keys(films.byFilm[hcKey(on)]).sort((a, b) => stockName(a).localeCompare(stockName(b)));
  return `<optgroup label="${esc(t("src_films"))}">` + list.map((f) => {
    const d = filmLooks(f, on).find((l) => l.default) || filmLooks(f, on)[0];
    return opt(`film:${f}`, stockName(f), t("film_t", { print: d.print ? stockName(d.print) : t("print_scan"), de: deText(d.report.final.all) }));
  }).join("") + "</optgroup>";
}

// the print row under the source: shown for a negative from the library
function renderPrintRow(T) {
  renderHcBox(T);
  renderLogBox(T);
  const f = T && filmOf(T);
  const looks = f ? filmLooks(f.film, !!f.hc) : [];
  $("print-row").hidden = !(f && f.print && looks.length);
  if ($("print-row").hidden) return;
  $("print").innerHTML = looks.map((l) => `<option value="${esc(l.print)}">${esc(stockName(l.print))}` +
    `${l.default ? " " + esc(t("print_default")) : ""} · ${deText(l.report.final.all)} ΔE</option>`).join("");
  $("print").value = f.print;
}

// a film of the library on target T: replaces its source, colour, curves, wheels and HSL
// `on`: fitted for highlight correction (default: what the target's row shows)
function applyFilm(T, film, print, on = hcOf(T)) {
  const looks = filmLooks(film, on);
  const l = looks.find((x) => x.print === print) || looks.find((x) => x.default) || looks[0];
  if (!l) return;
  const neutral = { hue: 0, sat: 0 };
  // stored in the preset: language-neutral (a slide has no print)
  const title = (l.print ? `${stockName(l.film)} · ${stockName(l.print)}` : stockName(l.film)) + (on ? " · HC" : "");
  applyLutFit(T, {
    source: "Standard", effects_off: false,
    matrices: { [T]: l.matrix }, curves: l.curves, hsl: l.hsl, post: l.post,
    edit: {
      matrix: { base: l.matrix, sat: 1, hue: 0, calib: { r: neutral, g: neutral, b: neutral } },
      lut: { library: "spektrafilm", film: l.film, print: l.print, title, input: "grlog -8 12",
        spektrafilm: films.libs[hcKey(on)].spektrafilm, report: l.report, ...(on ? { hc: true } : {}) },
    },
  });
}

$("print").onchange = (e) => {
  const f = state.current && filmOf(state.current);
  if (f) applyFilm(state.current, f.film, e.target.value);   // the print changes, the setting stays
};

// the film's fit for the camera's highlight correction: a box of its own under
// the print, so it does not read as a camera switch; for every LUT target
function renderHcBox(T) {
  const box = $("hc-box");
  const log = !!T && lutLib(T) === "grlog";
  box.hidden = !T || !(LUT_TARGETS.includes(T) || log);
  $("hc-note").hidden = true;
  if (box.hidden) return;
  const on = hcOf(T);
  $("hc-sum").textContent = t(log ? "hc_sum_log" : "hc_sum", { state: t(on ? "hc_sum_on" : "hc_sum_off") });
  $("hc-explain").textContent = t(log ? "hc_explain_log" : "hc_explain");
  const f = filmOf(T);
  for (const b of $("hc").querySelectorAll("button")) {
    b.classList.toggle("on", (b.dataset.hc === "on") === on);
    // "with" needs the second library for a film target
    b.disabled = b.dataset.hc === "on" && !!f && !films.libs.on;
  }
  const photo = state.shot && state.shot.highlight_correction;
  const fitted = state.targets[T] && state.targets[T].lut;
  const word = (v) => t(v ? "hc_w_on" : "hc_off_w");
  if (fitted && typeof photo === "boolean" && photo !== on) {
    $("hc-note").textContent = t("hc_photo", { photo: word(photo), fitted: word(on) });
    $("hc-note").hidden = false;
    box.open = true;
  }
}

for (const b of $("hc").querySelectorAll("button")) b.onclick = () => {
  const T = state.current;
  const on = b.dataset.hc === "on";
  if (T && hcOf(T) === on) return;
  const f = T && filmOf(T);
  const l = T && state.targets[T] && state.targets[T].lut;
  if (l && l.library === "grlog") { setHc(on); return applyGrLog(T, on); }   // the log curve on that tone curve
  if (l && !f) return notice(t("hc_lut_refit"), true);   // a LUT of the user's: only its dialog can refit
  setHc(on);
  if (f) return applyFilm(T, f.film, f.print, on);       // the same film, fitted for the setting
  renderPickers();
};

// a loaded DNG says how it was taken: new fits follow it
function hcFromShot(meta) {
  const v = meta && meta.highlight_correction;
  if (typeof v === "boolean" && v !== hcState.on) {
    setHc(v);
    notice(t("hc_loaded", { photo: t(v ? "hc_w_on" : "hc_off_w") }));
  }
  renderPickers();
}

// ---- GR-Log ---------------------------------------------------------------------------
// A log tone curve for video in manual exposure (core/src/grlog.rs,
// docs/plan-gr-log.md): on Standard's colours, the same curve on R, G and B.
// The target records it like a film (edit.lut.library "grlog"), so resets go
// back to it and another source drops it. Post LUTs come from the core.

const GRLOG_TARGETS = ["Standard", "Vivid", "PositiveFilm", "BleechBypass_GR", "Retro"];

function grlogOptions(T, opt) {
  if (!GRLOG_TARGETS.includes(T)) return "";
  return `<optgroup label="${esc(t("src_video"))}">${opt("grlog", "GR-Log", t("src_grlog_t"))}</optgroup>`;
}

// GR-Log on target T, for the camera's highlight correction off or on
async function applyGrLog(T, on = hcOf(T)) {
  try {
    const r = await api.grlog({ target: T, hc: on });
    r.spec.edit.lut.range = [r.first_ev, r.top_ev];
    applyLutFit(T, r.spec);
  } catch (err) {
    notice(t("grlog_failed", { msg: err.message }), true);
    renderPickers();
  }
}

function renderLogBox(T) {
  const l = T && state.targets[T] && state.targets[T].lut;
  const box = $("log-box");
  box.hidden = !(l && l.library === "grlog");
  if (box.hidden) return;
  const ev = (v) => (v >= 0 ? "+" : "−") + Math.abs(v).toLocaleString(LANG, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  $("log-range").hidden = !Array.isArray(l.range);
  if (l.range) $("log-range").textContent = t("grlog_range", { lo: ev(l.range[0]), hi: ev(l.range[1]) });
}

function logMsg(text, kind = "") {
  $("log-msg").className = `note ${kind}`;
  $("log-msg").textContent = text;
  $("log-msg").hidden = !text;
}

// the post LUT as a file to save: .cube text from the core
async function grlogDownload(to, text, stem) {
  const T = state.current;
  const l = T && state.targets[T] && state.targets[T].lut;
  if (!l || l.library !== "grlog") return;
  const name = `GR-Log${l.hc ? "-HC" : ""}_to_${stem}.cube`;
  try {
    const cube = await api.grlogLut({ to, hc: !!l.hc, lo: l.lo, span: l.span, size: 33 }, text);
    const a = document.createElement("a");
    a.href = URL.createObjectURL(new Blob([cube], { type: "text/plain" }));
    a.download = name;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 2000);
    logMsg(t("grlog_saved", { name }), "ok");
  } catch (err) {
    logMsg(t("grlog_failed", { msg: err.message }), "warn");
  }
}

for (const b of $("log-luts").querySelectorAll("button")) b.onclick = () => {
  if (b.dataset.to === "lut") return $("log-lut-file").click();
  grlogDownload(b.dataset.to, "", b.dataset.to === "standard" ? "Standard" : "Rec709");
};
$("log-lut-file").onchange = async (e) => {
  const f = e.target.files[0];
  e.target.value = "";
  if (f) grlogDownload("lut", await f.text(), f.name.replace(/\.cube$/i, ""));
};
