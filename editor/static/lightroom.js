// Lightroom develop presets as a starting point for a look: .xmp (Lightroom,
// Camera Raw) and .lrtemplate (Lightroom Classic). Like in Lightroom, a preset
// sets only what it contains. What the camera can do is taken over -- point
// curves, calibration, saturation, colour grading -- the rest is reported.

const LR_IDENT = () => [[0, 0], [1, 1]];

// settings as {key: number | string | boolean | [[x, y], ...]}, or null if the
// text is neither format
function parseLightroom(text) {
  const s = {};
  const scalar = (v) => {
    v = v.trim();
    if (/^[-+]?\d+(\.\d+)?$/.test(v)) return +v;
    if (/^true$/i.test(v)) return true;
    if (/^false$/i.test(v)) return false;
    return v;
  };
  const pairs = (nums) => {
    const out = [];
    for (let i = 0; i + 1 < nums.length; i += 2) out.push([nums[i], nums[i + 1]]);
    return out;
  };
  const numbers = (str) => str.split(",").map((x) => x.trim()).filter(Boolean).map(Number);
  if (/<x:xmpmeta|<rdf:RDF/.test(text)) {
    // ponytail: regex over the XML; masks nest further rdf:Description blocks, but
    // their attributes are all Local*/Mask*/Correction* and never collide
    for (const m of text.matchAll(/\bcrs:(\w+)="([^"]*)"/g)) s[m[1]] = scalar(m[2]);
    for (const m of text.matchAll(/<crs:(\w+)>\s*<rdf:Seq>([\s\S]*?)<\/rdf:Seq>/g))
      s[m[1]] = pairs([...m[2].matchAll(/<rdf:li>([^<]*)<\/rdf:li>/g)].flatMap((l) => numbers(l[1])));
    const name = text.match(/<crs:Name>[\s\S]*?<rdf:li[^>]*>([^<]*)<\/rdf:li>/);
    if (name) s.Name = name[1];
  } else if (/^\s*s\s*=\s*\{/.test(text)) {
    // a Lua table: key = number | "string" | true | { numbers }; nested tables
    // (value, settings) match nothing and are simply walked into
    for (const m of text.matchAll(/(\w+)\s*=\s*(\{[\s\d.,+-]*\}|"[^"]*"|[-+\d.]+|true|false)/g)) {
      const v = m[2];
      s[m[1]] = v[0] === "{" ? pairs(numbers(v.slice(1, -1))) : v[0] === '"' ? v.slice(1, -1) : scalar(v);
    }
    if (s.title) s.Name = s.title;
  } else return null;
  return s;
}

// a curve through the points sampled at n places on 0..1: the monotone cubic
// of core/src/tone.rs (transfer), so a baked curve looks like an edited one
function sampleCurve(points, n) {
  let pts = points.map(([x, y]) => [Math.min(1, Math.max(0, x)), Math.min(1, Math.max(0, y))])
    .sort((a, b) => a[0] - b[0]);
  if (!pts.length || pts[0][0] > 0) pts.unshift([0, 0]);
  if (pts[pts.length - 1][0] < 1) pts.push([1, 1]);
  for (let k = 1; k < pts.length; k++) pts[k][1] = Math.max(pts[k][1], pts[k - 1][1]);
  pts = pts.filter((p, k) => !k || p[0] - pts[k - 1][0] > 1e-6);
  const xs = pts.map((p) => p[0]), ys = pts.map((p) => p[1]);
  const t = Array.from({ length: n }, (_, i) => i / (n - 1));
  const seg = (v) => Math.min(xs.length - 2, Math.max(0, xs.findLastIndex((x) => x <= v)));
  if (xs.length < 3)
    return t.map((v) => { const k = seg(v); return ys[k] + (ys[k + 1] - ys[k]) * (v - xs[k]) / (xs[k + 1] - xs[k]); });
  const h = xs.slice(1).map((x, k) => x - xs[k]);
  const d = h.map((hk, k) => (ys[k + 1] - ys[k]) / hk);
  const m = xs.map((_, k) => {
    if (k === 0) return d[0];
    if (k === xs.length - 1) return d[d.length - 1];
    if (d[k - 1] * d[k] <= 0) return 0;
    const w1 = 2 * h[k] + h[k - 1], w2 = h[k] + 2 * h[k - 1];
    return (w1 + w2) / (w1 / d[k - 1] + w2 / d[k]);
  });
  return t.map((v) => {
    const k = seg(v), s = (v - xs[k]) / h[k], s2 = s * s, s3 = s2 * s;
    return (2 * s3 - 3 * s2 + 1) * ys[k] + (s3 - 2 * s2 + s) * h[k] * m[k]
      + (-2 * s3 + 3 * s2) * ys[k + 1] + (s3 - s2) * h[k] * m[k + 1];
  });
}

// what Lightroom does that the camera cannot: a key that is set and not neutral
const LR_SKIPPED = [
  ["hsl", /^(Hue|Saturation|Luminance)Adjustment/, 0],
  ["basic", /^(Exposure|Contrast|Highlights|Shadows|Whites|Blacks|Clarity)2012$|^(Dehaze|Texture|Vibrance)$/, 0],
  ["wb", /^(Temperature|Tint|IncrementalTemperature|IncrementalTint)$/, 0],
  ["parametric", /^Parametric(Shadows|Darks|Lights|Highlights)$/, 0],
  ["tint", /^ShadowTint$/, 0],
  ["grain", /^GrainAmount$/, 0],
  ["bw", /^ConvertToGrayscale$/, false],
  ["masks", /^(CorrectionMasks|MaskGroupBasedCorrections)$/, null],
  ["profile", /^(RGBTable|LookTable)$/, null],
];

// the part of the preset the look can take over, in the editor's units:
//   curves  [R, G, B] -> {base, points} for the variant's channel curves, or null
//   calib   {r: {hue, sat}, ...} with only the keys the preset sets
//   sat     0..2 or null
//   grade   {shadows: {hue, amount, lum}, ..., balance} with only the keys set
//   skipped ids from LR_SKIPPED
function lightroomLook(s) {
  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, +v || 0));
  const out = { name: s.Name || "", curves: null, calib: {}, sat: null, grade: {}, skipped: [] };

  const pts = (k) => (Array.isArray(s[k]) ? s[k].map(([x, y]) => [x / 255, y / 255]) : null);
  const ident = (p) => !p || p.every(([x, y]) => Math.abs(x - y) < 1e-4);
  const master = pts("ToneCurvePV2012") || pts("ToneCurve");
  const chans = ["Red", "Green", "Blue"].map((c) => pts("ToneCurvePV2012" + c) || pts("ToneCurve" + c));
  if (master || chans.some(Boolean)) {
    // Lightroom runs the RGB curve first, then the channel curves. The RGB curve
    // alone becomes the points of all three (linked); with channel curves it is
    // baked into the base so the channel points stay editable. 64 k is the
    // editor's linear curve, the last entry 16383 keeps white white.
    const both = !ident(master) && chans.some((c) => !ident(c));
    const base = both ? sampleCurve(master, 256).map((v) => Math.min(16383, Math.round(v * 16384))) : null;
    const shared = !both && !ident(master) ? master : null;
    out.curves = chans.map((c) => ({ base: base && base.slice(), points: shared || c || LR_IDENT() }));
  }

  for (const [p, k] of [["r", "Red"], ["g", "Green"], ["b", "Blue"]])
    for (const [f, kk] of [["hue", "Hue"], ["sat", "Saturation"]])
      if (s[k + kk] != null) (out.calib[p] = out.calib[p] || {})[f] = clamp(s[k + kk], -100, 100);
  if (s.Saturation != null) out.sat = clamp(1 + s.Saturation / 100, 0, 2);

  // split toning (hue, saturation of shadows/highlights, balance) and the newer
  // colour grading (midtones, global, luminance) are the editor's wheels 1:1
  const wheel = (name, hue, sat, lum) => {
    const w = {};
    if (s[hue] != null) w.hue = ((+s[hue] || 0) % 360 + 360) % 360;
    if (s[sat] != null) w.amount = clamp(s[sat] / 100, 0, 1);
    if (s[lum] != null) w.lum = clamp(s[lum] / 100, -1, 1);
    if (Object.keys(w).length) out.grade[name] = w;
  };
  wheel("shadows", "SplitToningShadowHue", "SplitToningShadowSaturation", "ColorGradeShadowLum");
  wheel("highlights", "SplitToningHighlightHue", "SplitToningHighlightSaturation", "ColorGradeHighlightLum");
  wheel("midtones", "ColorGradeMidtoneHue", "ColorGradeMidtoneSat", "ColorGradeMidtoneLum");
  wheel("global", "ColorGradeGlobalHue", "ColorGradeGlobalSat", "ColorGradeGlobalLum");
  if (s.SplitToningBalance != null) out.grade.balance = clamp(s.SplitToningBalance / 100, -1, 1);

  for (const [id, re, neutral] of LR_SKIPPED)
    if (Object.keys(s).some((k) => re.test(k) && s[k] !== neutral && s[k] !== "")) out.skipped.push(id);
  return out;
}

if (typeof module !== "undefined") module.exports = { parseLightroom, sampleCurve, lightroomLook };
