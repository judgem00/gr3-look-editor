#!/usr/bin/env node
// Self-check for lightroom.js: node editor/lightroom-check.js [preset files …]
// Without arguments it checks the two formats on built-in fixtures; with files it
// also parses each one and prints what the editor would take over.
const assert = require("assert");
const fs = require("fs");
const { parseLightroom, sampleCurve, lightroomLook } = require("./static/lightroom.js");

const XMP = `<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
 <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal" crs:RedSaturation="+20" crs:ShadowTint="0" crs:ConvertToGrayscale="False"
   crs:SplitToningShadowHue="210" crs:SplitToningShadowSaturation="30" crs:ColorGradeMidtoneLum="-10">
  <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">A1: Contrast +</rdf:li></rdf:Alt></crs:Name>
  <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>28, 15</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
  <crs:ToneCurvePV2012Red><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012Red>
 </rdf:Description></rdf:RDF></x:xmpmeta>`;
const LRT = `s = {
\tid = "99E9",
\ttitle = "S - Agfa Vista 100 +",
\tvalue = {
\t\tsettings = {
\t\t\tBlacks2012 = 25,
\t\t\tConvertToGrayscale = false,
\t\t\tHueAdjustmentRed = 5,
\t\t\tRedHue = 25,
\t\t\tRedSaturation = 40,
\t\t\tSaturation = -5,
\t\t\tToneCurvePV2012 = {
\t\t\t\t0,
\t\t\t\t23,
\t\t\t\t255,
\t\t\t\t245,
\t\t\t},
\t\t\tToneCurvePV2012Red = {
\t\t\t\t0,
\t\t\t\t0,
\t\t\t\t40,
\t\t\t\t9,
\t\t\t\t255,
\t\t\t\t255,
\t\t\t},
\t\t},
\t},
}`;

// xmp: one RGB curve -> the three channels share its points; wheels; +20 parses
let s = parseLightroom(XMP), lr = lightroomLook(s);
assert.strictEqual(lr.name, "A1: Contrast +");
assert.deepStrictEqual(s.ToneCurvePV2012, [[0, 0], [28, 15], [255, 255]]);
assert.ok(lr.curves.every((c) => c.points === lr.curves[0].points && c.base === null));
assert.deepStrictEqual(lr.calib, { r: { sat: 20 } });
assert.deepStrictEqual(lr.grade, { shadows: { hue: 210, amount: 0.3 }, midtones: { lum: -0.1 } });
assert.deepStrictEqual(lr.skipped, []);

// lrtemplate: RGB + channel curves -> RGB baked into the base, channel points kept
s = parseLightroom(LRT); lr = lightroomLook(s);
assert.strictEqual(lr.name, "S - Agfa Vista 100 +");
assert.deepStrictEqual(s.ToneCurvePV2012Red, [[0, 0], [40, 9], [255, 255]]);
assert.strictEqual(s.Blacks2012, 25, "keys before the first nested table must survive");
assert.strictEqual(lr.curves[0].base.length, 256);
assert.strictEqual(lr.curves[0].base[0], Math.round(23 / 255 * 16384));
assert.strictEqual(lr.curves[0].base[255], Math.round(245 / 255 * 16384));
assert.deepStrictEqual(lr.curves[1].points, [[0, 0], [1, 1]]);
assert.strictEqual(lr.curves[0].points[1][0], 40 / 255);
assert.deepStrictEqual(lr.calib, { r: { hue: 25, sat: 40 } });
assert.strictEqual(lr.sat, 0.95);
assert.deepStrictEqual(lr.skipped, ["hsl", "basic"]);

// the spline: identity stays identity, monotone through the points
assert.deepStrictEqual(sampleCurve([[0, 0], [1, 1]], 5), [0, 0.25, 0.5, 0.75, 1]);
const c = sampleCurve([[0, 0], [0.2, 0.1], [0.8, 0.9], [1, 1]], 101);
assert.ok(Math.abs(c[20] - 0.1) < 1e-9 && Math.abs(c[80] - 0.9) < 1e-9);
assert.ok(c.every((v, i) => !i || v >= c[i - 1] - 1e-12));
assert.strictEqual(parseLightroom("{\"matrices\": {}}"), null);

for (const f of process.argv.slice(2)) {
  const p = parseLightroom(fs.readFileSync(f, "utf8"));
  assert.ok(p, `${f}: not recognised`);
  const l = lightroomLook(p);
  console.log(`${l.name || f}: curves=${l.curves ? (l.curves[0].base ? "rgb+channels" : "points") : "-"}` +
    ` calib=${JSON.stringify(l.calib)} sat=${l.sat} wheels=${Object.keys(l.grade).join(",") || "-"}` +
    ` skipped=${l.skipped.join(",") || "-"}`);
}
console.log("lightroom-check ok");
