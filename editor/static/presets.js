// Export and presets (docs/plan-gui-redesign.md, "Export" and "Preset format (v2)").
//
// The export sends one object per changed target, the way the core takes it:
//   {"targets": {Name: {source, effects_off, matrices, hsl, curves | channels, edit}}}
// `curves` are [R, G, B] for every target but Cross Process, whose Color Tone
// curves go as `channels` {index: [256]}. `edit` is the editor's own state
// (sliders, extra base, curve points, wheels); the core keeps it in the preset
// as it is. The preset the core writes is that request with the sources filled
// in, plus "version": 2.
//
// Presets of the earlier editor (v1: matrices, matrix_edits, inherit,
// channel_edits, look_curves, hsl, effects_off, tone, ...) are converted to v2
// on load (fromV1). Its tone curve for all image controls is ignored with a note.

// ---- export ---------------------------------------------------------------------

function targetExport(T) {
  const s = targetSpec(T), e = editOf(T);
  const out = { source: s.source, effects_off: s.effects_off };
  const m = {};
  if (!same(effective(T), sourceLook(T).matrix)) m[T] = effective(T);
  // Bleach Bypass as its own source keeps three variants with sets of their own
  if (T === BLEACH && s.source === BLEACH && !s.ext && !slidersNeutral(e))
    for (const v of BLEACH_VARIANTS) m[v] = variantEffective(v);
  if (Object.keys(m).length) out.matrices = m;
  if (hslChangedOf(T)) out.hsl = hsl.looks[T];
  if (postOf(T)) out.post = postOf(T);
  const idx = targetChannels(T);
  if (T === CROSS) {
    const ch = idx.filter(chanChanged);
    if (ch.length) out.channels = Object.fromEntries(ch.map((i) => [i, chanValues(i)]));
  } else if (idx.some(chanChanged)) out.curves = idx.map(chanValues);
  // the points behind the curves and the sliders, so a loaded preset can be edited again
  const edits = Object.fromEntries(idx.filter((i) => state.chan.edits[i])
    .map((i) => [i, { base: state.chan.edits[i].base, points: state.chan.edits[i].points }]));
  out.edit = {
    matrix: { base: e.base, sat: e.sat, hue: e.hue, calib: e.calib },
    ext: s.ext,
    channel_edits: Object.keys(edits).length ? edits : null,
    grade: gradedOf(T).length ? Object.fromEntries(gradedOf(T).map((n) => [n, state.grade[n]])) : null,
  };
  // the LUT the look was fitted to (lut.js): name, input, report
  if (s.lut) out.edit.lut = s.lut;
  return out;
}

const exportBody = () => ({ targets: Object.fromEntries(changedTargets().map((T) => [T, targetExport(T)])) });
// what the last export contained; a later edit makes its downloads stale
const exportKey = () => JSON.stringify(exportBody());

function renderExportDialog() {
  const list = changedTargets();
  $("export-none").hidden = list.length > 0;
  $("changes").innerHTML = list.map((T) => {
    const s = targetSpec(T), x = extOf(T);
    const from = x ? extText(x.label) : lutName(T) ? t(lutFilm(T) ? "ex_film" : "ex_lut", { name: lutName(T) })
      : s.source !== T ? t("ex_from", { look: label(s.source) }) : t("ex_own");
    const what = changesOf(T);
    return `<li><b>${esc(label(T))}</b> <span class="muted">· ${esc(from)}</span>${what.length ? `<br><span class="muted">${esc(what.join(", "))}</span>` : ""}</li>`;
  }).join("");
  $("export").disabled = !list.length || !$("risk-ok").checked;
  $("flash-4").innerHTML = t("flash_4", { list: list.length ? list.map((n) => esc(label(n))).join(", ") : "—" });
  if (!$("downloads").hidden && state.lastExport !== exportKey()) {
    $("downloads").hidden = true;
    $("export").classList.add("primary");
    $("export-msg").textContent = "";
  }
}

$("export-open").onclick = () => {
  renderExportDialog();
  requestSpace();
  $("export-dlg").showModal();
};
$("risk-ok").onchange = () => renderExportDialog();

$("export").onclick = async () => {
  $("export-msg").className = "note";
  $("export-msg").textContent = t("creating");
  const body = exportBody();
  const res = await api("/api/export", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (res.ok) {
    const r = await res.json();
    $("export-msg").className = "note ok";
    $("export-msg").textContent = t("done_web");
    state.lastExport = JSON.stringify(body);
    if (r.space) renderSpace(r.space);
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

// ---- presets: v1 -> v2 ----------------------------------------------------------

const V1_KEYS = ["matrices", "matrix_edits", "tone", "channel_curves", "channel_edits", "inherit", "hsl", "look_curves", "effects_off"];

function fromV1(p, warn) {
  const out = {};
  const tg = (T) => (out[T] ||= { source: T, effects_off: false, edit: {} });
  const skip = (n) => warn.push(t("e_not_target", { look: label(n) }));
  for (const [T, src] of Object.entries(p.inherit || {})) {
    if (isTarget(T)) tg(T).source = src || T; else skip(T);
  }
  const me = p.matrix_edits || {};
  for (const [n, e] of Object.entries(me)) {
    if (isTarget(n)) tg(n).edit.matrix = e; else skip(n);
  }
  for (const [n, m] of Object.entries(p.matrices || {})) {
    if (me[n]) continue;
    if (isTarget(n)) tg(n).matrices = { [n]: m }; else skip(n);
  }
  // channel curves: 0..11 Cross Process's, 12.. three per look (core: look_sets)
  const owner = (i) => (i >= 0 && i < 12 ? CROSS : lookSets()[Math.floor((i - 12) / 3)]);
  const pre = p.channel_grade && p.channel_curves_pre;
  const edits = (T) => (tg(T).edit.channel_edits ||= {});
  if (p.channel_edits) {
    for (const [idx, e] of Object.entries(p.channel_edits)) {
      const T = owner(+idx);
      if (T) edits(T)[idx] = e; else warn.push(t("e_channel", { idx }));
    }
  } else {
    // older presets: only the finished curves, which become the base under straight points
    for (const [idx, vals] of Object.entries((pre ? p.channel_curves_pre : p.channel_curves) || {})) {
      const T = owner(+idx);
      if (T) edits(T)[idx] = { base: vals, points: IDENT }; else warn.push(t("e_channel", { idx }));
    }
  }
  // curves of other looks given only as finished curves
  for (const [n, rgb] of Object.entries(p.look_curves || {})) {
    const set = (state.fw.look_channel_sets || {})[n];
    if (!set || !Array.isArray(rgb) || rgb.length !== 3) { warn.push(t("e_set", { name: n })); continue; }
    if (set.some((i) => (out[n] && out[n].edit.channel_edits || {})[i])) continue;
    set.forEach((i, k) => { edits(n)[i] = { base: rgb[k], points: IDENT }; });
  }
  if (p.channel_grade && (pre || p.channel_edits)) {
    for (const [n, g] of Object.entries(p.channel_grade)) {
      const T = SET_ORDER.includes(n) ? CROSS : isTarget(n) ? n : null;
      if (!T) { warn.push(t("e_set", { name: n })); continue; }
      (tg(T).edit.grade ||= {})[n] = g;
    }
  }
  for (const [n, v] of Object.entries(p.hsl || {})) {
    if (isTarget(n)) tg(n).hsl = v; else skip(n);
  }
  const tone = p.tone && Array.isArray(p.tone.points) ? p.tone.points : null;
  if (tone && !tone.every((q) => Array.isArray(q) && Math.abs(q[0] - q[1]) < 1e-4)) warn.push(t("e_tone_ignored"));
  return { version: 2, firmware: p.source, targets: out };
}

// ---- presets: loading -------------------------------------------------------------

const isMatrix = (m) => Array.isArray(m) && m.length === 9 && m.every(Number.isFinite);
const isCurve = (v) => Array.isArray(v) && v.length === 256 && v.every(Number.isFinite);
const isPoints = (v) => Array.isArray(v) && v.length >= 2 &&
  v.every((q) => Array.isArray(q) && q.length === 2 && q.every(Number.isFinite));

// replace every edit by the v2 preset's targets
function applyPreset(p, warn) {
  state.targets = {};
  state.edits = {};
  state.chan.edits = {};
  state.grade = {};
  const hslIn = {};
  for (const [T, spec] of Object.entries(p.targets || {})) {
    if (!isTarget(T)) { warn.push(t("e_not_target", { look: label(T) })); continue; }
    if (!spec || typeof spec !== "object") continue;
    applyTarget(T, spec, warn, hslIn);
  }
  warn.push(...hslLoad(hslIn));
}

// one v2 target into the state (the target's edits are empty before);
// its HSL goes to hslIn for hslLoad
function applyTarget(T, spec, warn, hslIn) {
  const s = targetSpec(T), ed = spec.edit || {};
  const src = spec.source || T;
  if (sourcesFor(T).includes(src)) s.source = src;
  else warn.push(t(state.fw.sources.includes(src) ? "e_source_off" : "e_source_bad", { look: label(T), name: label(src) }));
  if (ed.ext) {
    if (state.extBases.some((x) => x.id === ed.ext)) Object.assign(s, { ext: ed.ext, source: T });
    else warn.push(t("e_ext", { name: ed.ext }));
  }

  // the post matrix: nine numbers with rows summing to 1 (the core checks the range)
  if (spec.post != null) {
    const ok = isMatrix(spec.post) && [0, 1, 2].every((r) => Math.abs(spec.post[3 * r] + spec.post[3 * r + 1] + spec.post[3 * r + 2] - 1) <= 1e-3);
    if (ok) s.post = spec.post.slice();
    else warn.push(t("e_post", { name: label(T) }));
  }

  // colour: sliders and the base if typed in; otherwise the exported matrix as the base
  const e = editOf(T);
  const m = ed.matrix;
  if (m && typeof m === "object") {
    if (m.base != null && !isMatrix(m.base)) warn.push(t("e_matrix", { name: label(T) }));
    e.base = isMatrix(m.base) ? m.base.slice() : null;
    e.sat = Number.isFinite(+m.sat) && m.sat !== null ? +m.sat : 1;
    e.hue = +m.hue || 0;
    const cal = neutralCalib();
    for (const q of PRIMS) for (const k of ["hue", "sat"])
      cal[q][k] = Math.max(-100, Math.min(100, +(((m.calib || {})[q] || {})[k]) || 0));
    e.calib = cal;
  } else if (spec.matrices && spec.matrices[T] != null) {
    if (isMatrix(spec.matrices[T])) e.base = spec.matrices[T].slice();
    else warn.push(t("e_matrix", { name: label(T) }));
  }
  // a preset of the earlier editor made from an extra base: its matrix says so
  if (!s.ext && s.source === T && e.base)
    for (const x of state.extBases) {
      if (extMatrix(x).every((v, k) => Math.abs(v - e.base[k]) < 1e-6)) { s.ext = x.id; break; }
    }
  if (e.base && same(e.base, startMatrix(T))) e.base = null;

  // curves: the points behind them, or the finished curves as the base
  const own = targetChannels(T);
  if (ed.channel_edits) {
    for (const [idx, c] of Object.entries(ed.channel_edits)) {
      const i = +idx;
      if (!own.includes(i) || !c || !(c.base == null || isCurve(c.base)) || !isPoints(c.points)) {
        warn.push(t("e_channel", { idx })); continue;
      }
      state.chan.edits[i] = {
        base: c.base == null ? null : c.base.map((v) => Math.round(v)),
        points: c.points.map((q) => q.slice()).sort((a, b) => a[0] - b[0]),
      };
    }
  } else if (T === CROSS && spec.channels) {
    for (const [idx, vals] of Object.entries(spec.channels)) {
      const i = +idx;
      if (!own.includes(i) || !isCurve(vals)) { warn.push(t("e_channel", { idx })); continue; }
      state.chan.edits[i] = { base: vals.map((v) => Math.round(v)), points: IDENT.map((q) => q.slice()) };
    }
  } else if (T !== CROSS && spec.curves) {
    if (Array.isArray(spec.curves) && spec.curves.length === 3 && spec.curves.every(isCurve))
      own.forEach((i, k) => {
        state.chan.edits[i] = { base: spec.curves[k].map((v) => Math.round(v)), points: IDENT.map((q) => q.slice()) };
      });
    else warn.push(t("e_set", { name: label(T) }));
  } else applyExtCurves(T, extOf(T));
  for (const [n, g] of Object.entries(ed.grade || {})) {
    if (setsOf(T).includes(n)) state.grade[n] = normGrade(g); else warn.push(t("e_set", { name: n }));
  }
  if (spec.hsl) hslIn[T] = spec.hsl;
  if (ed.lut && typeof ed.lut === "object") s.lut = ed.lut;
}

async function loadPreset(file) {
  let ok = false;
  try {
    let p;
    try { p = JSON.parse(await file.text()); } catch { throw new Error(t("e_json")); }
    if (!p || typeof p !== "object") throw new Error(t("e_preset"));
    const warn = [];
    if (p.version === 2 && p.targets && typeof p.targets === "object") { /* current format */ }
    else if (V1_KEYS.some((k) => k in p)) p = fromV1(p, warn);
    else throw new Error(t("e_preset"));
    applyPreset(p, warn);
    relink();
    chanEditor.deselect();
    if (state.current) { renderHsl(); requestHsl(); }
    // show the first changed target, so the preview has something to say
    const list = changedTargets();
    select(state.current && list.includes(state.current) ? state.current : list[0] || null);
    if (p.firmware && state.fw.path && !p.firmware.endsWith(state.fw.path)) warn.push(t("e_source", { source: p.firmware }));
    const parts = list.map((T) => {
      const s = targetSpec(T), x = extOf(T);
      return x ? `${label(T)} ← ${extText(x.label)}` : lutName(T) ? `${label(T)} ← ${lutFilm(T) ? "" : "LUT "}${lutName(T)}`
        : s.source !== T ? `${label(T)} ← ${label(s.source)}` : label(T);
    });
    ok = !warn.length;
    notice(t("preset_loaded", { name: file.name, parts: parts.join(" · ") || t("preset_nothing") }) +
      (warn.length ? t("preset_hints", { list: warn.join("; ") }) : ""), !ok);
  } catch (err) {
    notice(t("preset_failed", { msg: err.message }), true);
  }
  return ok;
}

$("head-preset-file").onchange = async (e) => {
  const f = e.target.files[0];
  e.target.value = "";     // the same file again must trigger a reload
  if (f) await loadPreset(f);
};
// index.html?preset=tutorial: a preset shipped with the editor (presets/<name>.looks.json),
// loaded once the firmware is in (the tutorial links here)
window.addEventListener("editor-ready", async () => {
  const name = new URLSearchParams(location.search).get("preset");
  if (!name || !/^[a-z0-9-]+$/.test(name)) return;
  const res = await fetch(`presets/${name}.looks.json`);
  if (res.ok) await loadPreset(new File([await res.blob()], `${name}.looks.json`));
});
// a look shipped with the editor, to start from
$("example-preset").onclick = async () => {
  const res = await fetch("presets/example.looks.json");
  if (!res.ok) return;
  await loadPreset(new File([await res.blob()], t("example_name")));
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
