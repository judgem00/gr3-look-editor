// HSL per image control: the camera's MultiAxial stage (docs/multiaxial.md).
//
// The stage sorts every pixel by its hue -- a coordinate 0..8192 round the
// Cb/Cr plane of linear camera RGB (red ~7700, blue ~2500, green ~5050,
// yellow ~6500) -- and applies one 3x3 matrix per hue range, weighted by
// luminance. Here the user draws Hue vs Hue / Sat / Lum curves as in
// DaVinci Resolve; the core (multiaxial.rs) splits them into the steps the
// hardware can do (at most 5 per quarter of the circle, 11 different ones)
// and the preview applies exactly that. The curve editor shows both: the
// smooth curve and, dashed, the steps.

const HSL_CIRCLE = 8192;
// the curves' x axis starts at red and runs red -> yellow -> green -> cyan ->
// blue -> magenta, as in Resolve; on the hue coordinate that is downwards
const HSL_X0 = 7700;
const hslX = (g) => ((((HSL_X0 - g) % HSL_CIRCLE) + HSL_CIRCLE) % HSL_CIRCLE) / HSL_CIRCLE;
const hslG = (x) => ((((HSL_X0 - x * HSL_CIRCLE) % HSL_CIRCLE) + HSL_CIRCLE) % HSL_CIRCLE);
// value range per curve; saturation and luminance: -100..100 (0 = unchanged)
const HSL_CHANNELS = {
  hue: { key: "hsl_hh", max: 45, fmt: (v) => `${v > 0 ? "+" : ""}${v.toFixed(1)}°` },
  sat: { key: "hsl_hs", max: 100, fmt: (v) => `×${(1 + v / 100).toFixed(2)}` },
  lum: { key: "hsl_hl", max: 100, fmt: (v) => `${v > 0 ? "+" : ""}${Math.round(v)}` },
};
const hsl = {
  looks: {},        // look name -> { curves: {hue, sat, lum: [[g, v], ...]}, protect, base? }
                    // base: a fitted film's or LUT's curves (lutfit.rs), not shown;
                    // the core combines it with the curves (multiaxial.rs)
  channel: "sat",
  enc: null,        // /api/hsl answer for the current look
  encLook: null,
  error: "",
  busy: false, pending: false,
  bg: null, bgKey: "",
};
const hslNewProtect = () => ({ shadows: 0, highlights: 4096, highlight_weight: 1 });
// a new curve starts with neutral points at red, yellow, green, cyan, blue and
// magenta (where the camera JPEG shows those hues on Standard, MR024661), as in
// Resolve: moving one point then only bends the curve up to its neighbours
const HSL_ANCHORS = [7700, 6500, 5050, 3550, 2500, 700];
const hslAnchors = () => HSL_ANCHORS.map((g) => [g, 0]);
const hslNewCurves = () => ({ hue: hslAnchors(), sat: hslAnchors(), lum: hslAnchors() });
function hslOf(name) {
  if (!hsl.looks[name]) hsl.looks[name] = { curves: hslNewCurves(), protect: hslNewProtect() };
  return hsl.looks[name];
}
// the user's curves are flat; with a base the look still has HSL
const hslCurvesFlat = (h) => !h || Object.values(h.curves).every((c) => c.every((p) => !p[1]));
const hslHasBase = (h) => !!h && !!h.base && Object.values(h.base).some((c) => c.some((p) => p[1]));
const hslNeutral = (h) => hslCurvesFlat(h) && !hslHasBase(h);
const hslChangedLooks = () => Object.keys(hsl.looks).filter((n) => !hslNeutral(hsl.looks[n]));
// what export and preset get: looks with an effect only
const hslForExport = () => Object.fromEntries(hslChangedLooks().map((n) => [n, hsl.looks[n]]));

function requestHsl() {
  if (hsl.busy) { hsl.pending = true; return; }
  hsl.busy = true;
  fetchHsl().finally(() => {
    hsl.busy = false;
    if (hsl.pending) { hsl.pending = false; requestHsl(); }
  });
}

async function fetchHsl() {
  const look = state.current;
  if (!look) return;
  const res = await api("/api/hsl", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ look, hsl: hslOf(look) }),
  });
  if (res.ok) {
    hsl.enc = await res.json();
    hsl.encLook = look;
    hsl.error = "";
  } else {
    // nothing that cannot go to the camera stays in the preview
    hsl.enc = null;
    hsl.encLook = look;
    hsl.error = await errorText(res);
  }
  renderHslStatus();
  drawHslCurve();
  draw();
}

// ---- preview (GPU) ------------------------------------------------------------

const HSL_GLSL = `
uniform bool uMa;
uniform vec4 uAxes[4];
uniform int uNib[20];
uniform mat3 uBank[12];
uniform mat3 uYcc;
uniform vec4 uWStart, uWSlope, uWBp;

float maWeight(float x) {
  int s = x >= uWBp.w ? 3 : x >= uWBp.z ? 2 : x >= uWBp.y ? 1 : 0;
  return clamp((uWStart[s] + uWSlope[s] * (x - uWBp[s]) / 32.) / 1024., 0., 1.);
}

// the MultiAxial stage on linear RGB (white = 1.0), as multiaxial.rs models it
vec3 multiaxial(vec3 c) {
  vec3 ycc = uYcc * c;
  float cb = ycc.y, cr = ycc.z, a, b;
  int q;
  if (cr >= 0.) { if (cb >= 0.) { q = 0; a = cr; b = cb; } else { q = 1; a = -cb; b = cr; } }
  else { if (cb >= 0.) { q = 3; a = cb; b = -cr; } else { q = 2; a = -cr; b = -cb; } }
  float x = a + b > 0. ? 2048. * b / (a + b) : 0.;
  vec4 ax = uAxes[q];
  int n = x >= ax.x ? 0 : x >= ax.y ? 1 : x >= ax.z ? 2 : x >= ax.w ? 3 : 4;
  vec3 o = uBank[uNib[5 * q + n]] * c;
  float w = maWeight(clamp(4096. * ycc.x, 0., 4095.));
  return max(c + w * (o - c), 0.);
}
`;

const hslActive = () => !!state.current && !!hsl.enc && hsl.encLook === state.current && !hslNeutral(hsl.looks[state.current]);

function hslUniforms() {
  const on = hslActive();
  gl.uniform1i(uni("uMa"), on ? 1 : 0);
  if (!on) return;
  const e = hsl.enc;
  gl.uniform4fv(uni("uAxes"), e.axes.flat());
  gl.uniform1iv(uni("uNib"), e.nibbles);
  gl.uniformMatrix3fv(uni("uBank"), false, e.banks.flatMap(transpose));
  gl.uniformMatrix3fv(uni("uYcc"), false, transpose(e.ycc));
  const c = e.control;
  const signed9 = (v) => (v & 0x100 ? v - 0x200 : v);
  gl.uniform4fv(uni("uWStart"), c.slice(10, 14));
  gl.uniform4fv(uni("uWSlope"), c.slice(14, 18).map(signed9));
  gl.uniform4fv(uni("uWBp"), [0, c[18], c[19], c[20]]);
}

// ---- colours of the background: a stage colour after the camera's tone curve ---

// unit (Cb, Cr) direction of a hue coordinate, as multiaxial.rs direction()
function hslDirection(g) {
  g = ((g % HSL_CIRCLE) + HSL_CIRCLE) % HSL_CIRCLE;
  const quarter = Math.min(3, Math.floor(g / 2048));
  const r = (g - 2048 * quarter) / 2048, a = 1 - r, b = r;
  const [cb, cr] = [[b, a], [a, -b], [-b, -a], [-a, b]][quarter];
  const n = Math.hypot(cb, cr);
  return [cb / n, cr / n];
}

// display colour of hue coordinate g with the curve value v applied to channel ch
function hslColor(g, ch, v) {
  const ycc = (hsl.enc && hsl.enc.ycc) || state.fw.multiaxial.ycc;
  let [cb, cr] = hslDirection(g);
  let y = 0.24, chroma = 0.12;
  if (ch === "hue") {
    // positive turns towards lower coordinates (red -> yellow -> green)
    const th = -v * Math.PI / 180, c = Math.cos(th), s = Math.sin(th);
    [cb, cr] = [c * cb + s * cr, -s * cb + c * cr];
  } else if (ch === "sat") chroma *= 1 + v / 100;
  else y += (v / 100) * chroma;
  const m = mul(inverse(ycc), [y, 0, 0, chroma * cb, 0, 0, chroma * cr, 0, 0]);
  const lut = state.curves && state.curves.lut;
  const scale = (state.shot && state.shot.scale) || 4000;
  return [m[0], m[3], m[6]].map((x) => {
    x = Math.max(0, x);
    if (!lut) return 255 * Math.pow(Math.min(1, x * 2), 1 / 2.2);
    const f = Math.min(255, x * scale / 16), i = Math.min(254, Math.floor(f));
    return 255 * (lut[i] + (lut[i + 1] - lut[i]) * (f - i)) / 16384;
  });
}

// ---- the curve editor ---------------------------------------------------------------

const hslCv = () => $("hsl-curve");
const PAD = 8;
function hslGeom() {
  const cv = hslCv(), max = HSL_CHANNELS[hsl.channel].max;
  const w = cv.width, h = cv.height, ih = h - 2 * PAD;
  return {
    w, h,
    px: (g) => hslX(g) * w,
    py: (v) => PAD + (1 - (v + max) / (2 * max)) * ih,
    val: (y) => Math.max(-max, Math.min(max, (1 - (y - PAD) / ih) * 2 * max - max)),
  };
}

// background: the hue running across, the channel's effect running up
function hslBackground() {
  const cv = hslCv();
  const key = [hsl.channel, cv.width, cv.height, state.curves && state.curves.lut && state.curves.lut[128],
    state.shot && state.shot.scale].join();
  if (hsl.bg && hsl.bgKey === key) return hsl.bg;
  const { w, h, val } = hslGeom();
  const img = new ImageData(w, h);
  const rows = 24, cols = Math.ceil(w / 2);
  const grid = [];
  for (let r = 0; r <= rows; r++) {
    const v = val(PAD + (r / rows) * (h - 2 * PAD));
    grid.push(Array.from({ length: cols + 1 }, (_, c) => hslColor(hslG(c / cols), hsl.channel, v)));
  }
  for (let y = 0; y < h; y++) {
    const fr = Math.max(0, Math.min(rows, ((y - PAD) / (h - 2 * PAD)) * rows));
    const r0 = Math.min(rows - 1, Math.floor(fr)), tr = fr - r0;
    for (let x = 0; x < w; x++) {
      const fc = (x / w) * cols, c0 = Math.min(cols - 1, Math.floor(fc)), tc = fc - c0;
      const o = 4 * (y * w + x);
      for (let k = 0; k < 3; k++) {
        const a = grid[r0][c0][k] * (1 - tc) + grid[r0][c0 + 1][k] * tc;
        const b = grid[r0 + 1][c0][k] * (1 - tc) + grid[r0 + 1][c0 + 1][k] * tc;
        img.data[o + k] = (a * (1 - tr) + b * tr) * 0.8;   // a little darker, so the lines stand out
      }
      img.data[o + 3] = 255;
    }
  }
  hsl.bg = img;
  hsl.bgKey = key;
  return img;
}

function drawHslCurve() {
  const cv = hslCv();
  if (!cv || !state.fw || !state.current) return;
  const ctx = cv.getContext("2d");
  const { w, h, px, py } = hslGeom();
  ctx.putImageData(hslBackground(), 0, 0);
  // zero line and the quarter borders, where edges are free
  ctx.strokeStyle = "rgba(255,255,255,.35)";
  ctx.lineWidth = 1;
  ctx.beginPath(); ctx.moveTo(0, py(0) + 0.5); ctx.lineTo(w, py(0) + 0.5); ctx.stroke();
  ctx.setLineDash([2, 4]);
  for (const g of [0, 2048, 4096, 6144]) {
    const x = Math.round(px(g)) + 0.5;
    ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, h); ctx.stroke();
  }
  ctx.setLineDash([]);
  const pts = hslOf(state.current).curves[hsl.channel];
  const res = hsl.enc && hsl.encLook === state.current && hsl.enc.curves;
  // a per-bin series on the hue coordinate, drawn left to right across the x axis
  const series = (vals, stepped) => {
    const n = vals.length, bin = HSL_CIRCLE / n;
    const order = Array.from({ length: n }, (_, i) => i).sort((a, b) => hslX((a + 0.5) * bin) - hslX((b + 0.5) * bin));
    ctx.beginPath();
    order.forEach((i, k) => {
      const y = py(vals[i]);
      if (stepped) {
        // a bin's lower coordinate is its right edge on screen
        const xl = hslX((i + 1) * bin) * w, xr = xl + w / n;
        if (k === 0) ctx.moveTo(xl, y); else ctx.lineTo(xl, y);
        ctx.lineTo(xr, y);
      } else {
        const x = hslX((i + 0.5) * bin) * w;
        if (k === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
      }
    });
    ctx.stroke();
  };
  if (res) {
    ctx.strokeStyle = "#f4f1ea";
    ctx.lineWidth = 2;
    series(res.smooth[hsl.channel], false);
    ctx.strokeStyle = "#e8b04a";
    ctx.lineWidth = 1.5;
    ctx.setLineDash([5, 3]);
    series(res.steps[hsl.channel], true);
    ctx.setLineDash([]);
  }
  for (const [i, p] of pts.entries()) {
    ctx.beginPath();
    ctx.arc(px(p[0]), py(p[1]), i === hsl.drag ? 6 : 4.5, 0, 2 * Math.PI);
    ctx.fillStyle = i === hsl.drag ? "#e8b04a" : "#f4f1ea";
    ctx.fill();
    ctx.strokeStyle = "#1d1d20";
    ctx.stroke();
  }
  if (hsl.drag != null && pts[hsl.drag]) {
    const p = pts[hsl.drag];
    $("hsl-readout").textContent = HSL_CHANNELS[hsl.channel].fmt(p[1]);
  } else $("hsl-readout").textContent = "";
}

function hslChanged() {
  renderHslStatus();
  drawHslCurve();
  renderPickers();
  renderSections();
  renderChanges();
  syncTargets();
  requestHsl();
}

function hslPointer() {
  const cv = hslCv();
  const pos = (e) => {
    const r = cv.getBoundingClientRect();
    return [(e.clientX - r.left) * cv.width / r.width, (e.clientY - r.top) * cv.height / r.height];
  };
  const hit = (x, y) => {
    const { px, py, w } = hslGeom();
    const pts = hslOf(state.current).curves[hsl.channel];
    let best = -1, bd = 10;
    pts.forEach((p, i) => {
      let dx = Math.abs(px(p[0]) - x);
      dx = Math.min(dx, w - dx);
      const d = Math.hypot(dx, py(p[1]) - y);
      if (d < bd) { bd = d; best = i; }
    });
    return best;
  };
  // the value the curve has at x now (a new point sits on the curve)
  const onCurve = (x) => {
    const res = hsl.enc && hsl.encLook === state.current && hsl.enc.curves;
    if (!res) return 0;
    const vals = res.smooth[hsl.channel];
    return vals[Math.floor(hslG(x / hslGeom().w) / (HSL_CIRCLE / vals.length)) % vals.length];
  };
  cv.onpointerdown = (e) => {
    if (!state.current) return;
    const [x, y] = pos(e);
    const pts = hslOf(state.current).curves[hsl.channel];
    let i = hit(x, y);
    if (i < 0) {
      pts.push([Math.round(hslG(x / hslGeom().w)), +onCurve(x).toFixed(1)]);
      i = pts.length - 1;
    }
    hsl.drag = i;
    cv.setPointerCapture(e.pointerId);
    hslChanged();
  };
  cv.onpointermove = (e) => {
    const [x, y] = pos(e);
    if (hsl.drag == null) { cv.style.cursor = hit(x, y) >= 0 ? "grab" : "crosshair"; return; }
    const { w, val } = hslGeom();
    const p = hslOf(state.current).curves[hsl.channel][hsl.drag];
    p[0] = Math.round(hslG(Math.max(0, Math.min(w - 1, x)) / w));
    p[1] = +val(y).toFixed(1);
    hslChanged();
  };
  cv.onpointerup = () => { hsl.drag = null; drawHslCurve(); };
  cv.ondblclick = (e) => {
    const [x, y] = pos(e);
    const i = hit(x, y);
    if (i < 0) return;
    hslOf(state.current).curves[hsl.channel].splice(i, 1);
    hsl.drag = null;
    hslChanged();
  };
}

function renderHsl() {
  for (const b of document.querySelectorAll("#hsl-channels button"))
    b.classList.toggle("on", b.dataset.ch === hsl.channel);
  renderHslProtect();
  renderHslStatus();
  drawHslCurve();
}

function renderHslProtect() {
  if (!state.current) return;
  const p = hslOf(state.current).protect;
  const pct = (v) => `${Math.round(v / 40.96)} %`;
  $("hsl-shadows").value = p.shadows;
  $("hsl-shadows-out").textContent = p.shadows ? pct(p.shadows) : t("hsl_off");
  $("hsl-hi").value = p.highlights;
  $("hsl-hi-out").textContent = p.highlights >= 4096 ? t("hsl_off") : pct(p.highlights);
  $("hsl-hi-w").value = Math.round(p.highlight_weight * 100);
  $("hsl-hi-w-out").textContent = `${Math.round(p.highlight_weight * 100)} %`;
}

function renderHslStatus() {
  const el = $("hsl-status");
  if (!el || !state.current) return;
  if (hsl.error) { el.className = "note warn"; el.textContent = `${hsl.error} ${t("hsl_error_preview")}`; return; }
  const e = hsl.enc && hsl.encLook === state.current ? hsl.enc : null;
  const notes = e && e.notes && e.notes.length ? e.notes : [];
  el.className = "note muted";
  const h = hslOf(state.current);
  el.textContent = hslNeutral(h) ? t("hsl_none")
    : e ? [hslHasBase(h) ? t("hsl_base") : "",
      t("hsl_budget", { banks: e.banks_used - 1, max: 11, edges: e.axes.map((a) => a.filter(Boolean).length).join(" · ") }),
      ...notes.map((n) => t("hsl_simplified", { what: n }))].filter(Boolean).join(" ") : "";
}

// called once from app.js when the page starts
function initHsl() {
  for (const b of document.querySelectorAll("#hsl-channels button"))
    b.onclick = () => { hsl.channel = b.dataset.ch; hsl.drag = null; renderHsl(); };
  hslPointer();
  $("hsl-reset-curve").onclick = () => { hslOf(state.current).curves[hsl.channel] = hslAnchors(); hslChanged(); };
  $("hsl-shadows").oninput = (e) => { hslOf(state.current).protect.shadows = +e.target.value; renderHslProtect(); hslChanged(); };
  $("hsl-hi").oninput = (e) => { hslOf(state.current).protect.highlights = +e.target.value; renderHslProtect(); hslChanged(); };
  $("hsl-hi-w").oninput = (e) => { hslOf(state.current).protect.highlight_weight = +e.target.value / 100; renderHslProtect(); hslChanged(); };
}

// a preset's "hsl": {look: {curves | bands, protect}}; returns warnings.
// Bands (presets from before the curves) become curve points. `keep` keeps
// the other looks' HSL (a single target, lut.js).
function hslLoad(p, keep = false) {
  if (!keep) hsl.looks = {};
  const warn = [];
  const num = (v, lo, hi, d) => (Number.isFinite(+v) ? Math.max(lo, Math.min(hi, +v)) : d);
  for (const [name, v] of Object.entries(p || {})) {
    if (!state.looks[name] || !v || !(v.curves || v.base || Array.isArray(v.bands))) { warn.push(t("e_look", { name })); continue; }
    const curves = { hue: [], sat: [], lum: [] };
    for (const ch of Object.keys(curves)) {
      const max = HSL_CHANNELS[ch].max;
      const src = (v.curves && v.curves[ch]) || [];
      curves[ch] = src.filter((q) => Array.isArray(q) && q.length === 2)
        .map((q) => [num(q[0], 0, HSL_CIRCLE - 1, 0), num(q[1], -max, max, 0)]);
      if ((v.curves || v.base) && !curves[ch].length) curves[ch] = hslAnchors();
    }
    // a fitted film's or LUT's HSL under the curves: kept as it is, not shown
    let base = null;
    if (v.base && typeof v.base === "object") {
      base = {};
      for (const ch of ["hue", "sat", "lum"])
        base[ch] = ((v.base[ch]) || []).filter((q) => Array.isArray(q) && q.length === 2 && q.every(Number.isFinite))
          .map((q) => [num(q[0], 0, HSL_CIRCLE - 1, 0), num(q[1], -HSL_CHANNELS[ch].max, HSL_CHANNELS[ch].max, 0)]);
    }
    if (!v.curves && !v.base) {
      for (const b of v.bands) {
        const lo = +b.center - b.width / 2, hi = +b.center + b.width / 2, f = +b.feather || 0;
        for (const ch of Object.keys(curves)) {
          const val = num(b[ch], -HSL_CHANNELS[ch].max, HSL_CHANNELS[ch].max, 0);
          if (!val) continue;
          const wrap = (g) => Math.round(((g % HSL_CIRCLE) + HSL_CIRCLE) % HSL_CIRCLE);
          curves[ch].push([wrap(lo - f - 50), 0], [wrap(lo), val], [wrap(hi), val], [wrap(hi + f + 50), 0]);
        }
      }
    }
    const pr = v.protect || {};
    hsl.looks[name] = {
      curves,
      protect: { shadows: num(pr.shadows, 0, 2048, 0), highlights: num(pr.highlights, 2048, 4096, 4096),
        highlight_weight: num(pr.highlight_weight, 0, 1, 1) },
    };
    if (base) hsl.looks[name].base = base;
  }
  return warn;
}
