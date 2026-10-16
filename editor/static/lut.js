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

const films = { lib: null, byFilm: {} };

async function loadFilms() {
  try {
    const res = await fetch("films/spektrafilm.json");
    if (!res.ok) return;
    films.lib = await res.json();
    films.byFilm = {};
    for (const l of films.lib.looks) (films.byFilm[l.film] ||= []).push(l);
    if (state.fw) renderPickers();
  } catch { /* no library: no film group */ }
}
window.addEventListener("editor-ready", loadFilms);

const stockName = (id) => (films.lib && films.lib.names[id]) || id;
const filmLooks = (film) => films.byFilm[film] || [];
const filmOf = (T) => {
  const l = state.targets[T] && state.targets[T].lut;
  return l && l.library === "spektrafilm" ? l : null;
};
const deText = (v) => (Number.isFinite(v) ? v.toFixed(2) : "–");

// the source picker's film group: one entry per film (its own print first)
function filmOptions(T, opt) {
  if (!films.lib || !LUT_TARGETS.includes(T)) return "";
  const list = Object.keys(films.byFilm).sort((a, b) => stockName(a).localeCompare(stockName(b)));
  return `<optgroup label="${esc(t("src_films"))}">` + list.map((f) => {
    const d = filmLooks(f).find((l) => l.default) || filmLooks(f)[0];
    return opt(`film:${f}`, stockName(f), t("film_t", { print: d.print ? stockName(d.print) : t("print_scan"), de: deText(d.report.final.all) }));
  }).join("") + "</optgroup>";
}

// the print row under the source: shown for a negative from the library
function renderPrintRow(T) {
  const f = T && filmOf(T);
  const looks = f ? filmLooks(f.film) : [];
  $("print-row").hidden = !(f && f.print && looks.length);
  if ($("print-row").hidden) return;
  $("print").innerHTML = looks.map((l) => `<option value="${esc(l.print)}">${esc(stockName(l.print))}` +
    `${l.default ? " " + esc(t("print_default")) : ""} · ${deText(l.report.final.all)} ΔE</option>`).join("");
  $("print").value = f.print;
}

// a film of the library on target T: replaces its source, colour, curves, wheels and HSL
function applyFilm(T, film, print) {
  const looks = filmLooks(film);
  const l = looks.find((x) => x.print === print) || looks.find((x) => x.default) || looks[0];
  if (!l) return;
  const neutral = { hue: 0, sat: 0 };
  // stored in the preset: language-neutral (a slide has no print)
  const title = l.print ? `${stockName(l.film)} · ${stockName(l.print)}` : stockName(l.film);
  applyLutFit(T, {
    source: "Standard", effects_off: false,
    matrices: { [T]: l.matrix }, curves: l.curves, hsl: l.hsl, post: l.post,
    edit: {
      matrix: { base: l.matrix, sat: 1, hue: 0, calib: { r: neutral, g: neutral, b: neutral } },
      lut: { library: "spektrafilm", film: l.film, print: l.print, title, input: "grlog -8 12",
        spektrafilm: films.lib.spektrafilm, report: l.report },
    },
  });
}

$("print").onchange = (e) => {
  const f = state.current && filmOf(state.current);
  if (f) applyFilm(state.current, f.film, e.target.value);
};
