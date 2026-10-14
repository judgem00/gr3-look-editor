// Targets (docs/plan-gui-redesign.md): the image controls that get a look of
// the user's, each built from a source look. Every target is independent of
// every other look: its colour edit, curves, wheels and HSL are its own, and
// changing its source keeps them; only the base underneath changes.
//
// Per target the editor keeps
//   state.targets[name]  { source, ext, effects_off }  (ext: id of an extra base;
//                        effects_off: HDR Tone only, always on for it)
//   state.edits[name]    { base | null, sat, hue, calib }  (base null: the source's matrix)
//   state.chan.edits / state.grade  by channel index / set name, as before
//   hsl.looks[name]      HSL curves (hsl.js)
// The core's session holds the targets that are changed (syncTargets), so
// that the HSL encoding and the space meter see the same colour sets as the
// export.

const CROSS = "CrossProcess";
const HDR = "HdrCuisine";
const BLEACH = "BleechBypass_GR";
const BLEACH_VARIANTS = ["BleechBypass_GR · Var. 2", "BleechBypass_GR · Var. 3"];

const targetNames = () => (state.fw ? state.fw.target_names : []);
const isTarget = (name) => targetNames().includes(name);

function targetSpec(name) {
  // HDR Tone as a target always has its effects off (no local tone mapping):
  // the modified firmware never carries them, and only then is its matrix its own
  if (!state.targets[name]) state.targets[name] = { source: name, ext: null, effects_off: name === HDR };
  return state.targets[name];
}

const extOf = (name) => {
  const id = state.targets[name] && state.targets[name].ext;
  return (id && state.extBases.find((x) => x.id === id)) || null;
};
// the look whose data the target computes with (an extra base keeps the target's own)
const sourceOf = (name) => targetSpec(name).source;
const sourceLook = (name) => state.stock[sourceOf(name)];
// Looks that are not offered as a source for another target: their colours
// do not make a good base. As its own source each stays a target.
const NO_SOURCE = ["Retro", "BleechBypass_GR", "BleechBypass_GR · Var. 2", "BleechBypass_GR · Var. 3", "HdrCuisine", "CrossProcess"];
const sourcesFor = (name) => state.fw.sources.filter((n) => n === name || !NO_SOURCE.includes(n));

// the matrix the target starts from: its source's, or the extra base's
const startMatrix = (name) => (extOf(name) ? extMatrix(extOf(name)) : sourceLook(name).matrix);

// ---- colour (matrix) -----------------------------------------------------------

function editOf(name) {
  if (!state.edits[name]) state.edits[name] = { base: null, sat: 1, hue: 0, calib: neutralCalib() };
  const e = state.edits[name];
  if (!e.calib) e.calib = neutralCalib();
  return e;
}
const slidersNeutral = (e) => e.sat === 1 && !e.hue && calibNeutral(e.calib);
const baseMatrix = (name) => editOf(name).base || startMatrix(name);
const applySliders = (e, base) => mul(mul(mul(saturation(e.sat), hueRotation(e.hue)), base), calibMatrix(e.calib));

function effective(name) {
  return applySliders(editOf(name), baseMatrix(name));
}
// Bleach Bypass as its own source keeps its variants: the sliders act on each
const variantEffective = (v) => applySliders(editOf(BLEACH), state.stock[v].matrix);

const baseColourChanged = (name) => !slidersNeutral(editOf(name));
const matrixTyped = (name) => !!editOf(name).base &&
  !same(editOf(name).base, startMatrix(name));

// ---- curves ----------------------------------------------------------------------

// the curve sets of a target: Cross Process's Color Tone 1-3, or the look's own
const setsOf = (name) => (name === CROSS ? SET_ORDER : [name]);
const targetChannels = (name) => setsOf(name).flatMap((n) => chanSet(n));
const ownerOfChannel = (idx) => (idx < 12 ? CROSS : lookSets()[Math.floor((idx - 12) / 3)]);
// the curve a channel starts from: an extra base's, or the firmware's (neutral)
function curveStart(idx) {
  const x = extOf(ownerOfChannel(idx));
  return x && x.channel_curve ? x.channel_curve.map((v) => Math.round(v)) : null;
}
function curvesEdited(name) {
  return targetChannels(name).some((i) => {
    const e = state.chan.edits[i];
    if (!e) return !!curveStart(i);
    const start = curveStart(i) || state.fw.channel_curves[i];
    return !isIdent(e.points) || (e.base ? !sameCurve(e.base, start) : !!curveStart(i));
  });
}
const curvesForExport = (name) => targetChannels(name).filter(chanChanged);
const gradedOf = (name) => setsOf(name).filter((n) => !gradeNeutral(state.grade[n]));

// an extra base puts its curve on every channel of the target
function applyExtCurves(name, x) {
  if (!x || !x.channel_curve) return;
  for (const i of targetChannels(name))
    state.chan.edits[i] = { base: x.channel_curve.map((v) => Math.round(v)), points: IDENT.map((p) => p.slice()) };
}
// leaving an extra base: its curves go where they are still untouched
function clearExtCurves(name, x) {
  if (!x || !x.channel_curve) return;
  const c0 = x.channel_curve.map((v) => Math.round(v));
  for (const i of targetChannels(name)) {
    const c = state.chan.edits[i];
    if (c && sameCurve(c.base, c0) && isIdent(c.points)) delete state.chan.edits[i];
  }
}

// ---- what is changed -----------------------------------------------------------

const hslChangedOf = (name) => !hslNeutral(hsl.looks[name]);
function targetChanged(name) {
  if (!state.fw || !state.stock[name]) return false;
  const s = targetSpec(name);
  return s.source !== name || !!s.ext || !same(effective(name), state.stock[name].matrix) ||
    curvesForExport(name).length > 0 || hslChangedOf(name);
}
const changedTargets = () => targetNames().filter(targetChanged);

// one line per target for the picker: its state
function targetState(name) {
  const s = targetSpec(name), x = extOf(name);
  if (x) return extText(x.label);
  if (s.source !== name) return t("tg_from", { look: label(s.source) });
  return targetChanged(name) ? t("tg_own") : t("tg_stock");
}

// what changed, for the export list
function changesOf(name) {
  const out = [];
  const e = editOf(name);
  if (!slidersNeutral(e) || matrixTyped(name)) out.push(t("ex_matrix"));
  const sets = new Set(curvesForExport(name).map(chanSetOf));
  if (sets.size) out.push(name === CROSS ? t("ex_curves_sets", { list: SET_ORDER.filter((n) => sets.has(n)).map(setLabel).join(", ") }) : t("ex_curves"));
  if (gradedOf(name).length) out.push(t("ex_wheels"));
  if (hslChangedOf(name)) out.push(t("ex_hsl"));
  if (targetSpec(name).effects_off) out.push(t("ex_hdr_off"));
  return out;
}

// ---- the core's session --------------------------------------------------------

function takeFirmware(fw) {
  Object.assign(state.fw, { looks: fw.looks, inherit: fw.inherit, effects_off: fw.effects_off, targets: fw.targets });
  for (const l of fw.looks) state.looks[l.name] = l;
}

// bring the session's targets in line with the changed ones, one request at a time
let syncBusy = false, syncAgain = false, syncTimer = null;
function syncTargets() {
  clearTimeout(syncTimer);
  syncTimer = setTimeout(runSync, 30);
}
async function runSync() {
  if (!state.fw) return;
  if (syncBusy) { syncAgain = true; return; }
  syncBusy = true;
  let changed = false;
  try {
    for (const name of targetNames()) {
      const s = targetSpec(name);
      const want = targetChanged(name) ? { source: s.source, effects_off: s.effects_off } : null;
      const have = (state.fw.targets || []).find((x) => x.name === name);
      const ok = want ? have && have.source === want.source && have.effects_off === want.effects_off
        : !have || (name === CROSS && have.source === CROSS && !have.effects_off);
      if (ok) continue;
      const res = await api("/api/target", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(want ? { target: name, ...want } : { target: name, remove: true }),
      });
      if (!res.ok) { notice(t("e_target_set", { look: label(name), msg: await errorText(res) }), true); continue; }
      takeFirmware(await res.json());
      changed = true;
    }
  } finally {
    syncBusy = false;
  }
  if (syncAgain) { syncAgain = false; return runSync(); }
  if (changed) {
    // the colour sets moved: HSL encoding and the meter read them
    spaceKey = null;
    requestSpace();
    if (state.current) requestHsl();
    renderAdvanced();
  }
}
