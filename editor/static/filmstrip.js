// The filmstrip: "Load image folder" reads the camera JPEG embedded in every
// DNG of the picked folder and shows it as a small strip under the photo.
// Click a thumbnail or use the arrow keys to switch photos.
//
// The embedded JPEGs are found by the core (dng::preview_ranges, worker op
// dng_preview), which walks only the IFD headers — a small file prefix
// suffices, except for files that put IFD0 at the very end (fall back to the
// whole file). Loading a photo runs the normal pipeline (app.js loadDng).

const stripState = {
  entries: [],   // { name, file, status: "idle"|"loading"|"done"|"error", el }
  index: -1,     // the selected entry; -1: none (the first ArrowRight takes entry 0)
  gen: 0,        // bumped on every folder pick; stale decodes drop their result
};

// ---- the embedded JPEG range -------------------------------------------------

// candidates [[off, len], ...] (largest first) from the core; the caps cover
// the usual DNGs with a prefix, up to files whose IFD0 sits at the very end.
// Returns {off, len, flip} of the chosen JPEG.
async function jpegRangeOf(file) {
  let info = { ranges: [], flip: 0 };
  for (const cap of [1 << 20, 8 << 20, Infinity]) {
    const bytes = await file.slice(0, Math.min(file.size, cap)).arrayBuffer();
    info = await api.dngPreview(bytes);
    if (info.ranges.length) break;
  }
  for (const [off, len] of info.ranges) {
    if (off < 0 || len <= 0 || off + len > file.size) continue;
    const head = new Uint8Array(await file.slice(off, off + 2).arrayBuffer());
    if (head[0] === 0xff && head[1] === 0xd8) return { off, len, flip: info.flip };
  }
  return null;
}

// ---- folder picking ----------------------------------------------------------

$("folder-open").onclick = async () => {
  if (window.showDirectoryPicker) {
    let dir = null;
    try {
      // must be the first await: the picker needs the click's user activation
      dir = await window.showDirectoryPicker({ mode: "read" });
    } catch (err) {
      if (err.name === "AbortError") return;   // the user cancelled
      // permission refused or not available here: the input below works too
    }
    if (dir) {
      try {
        const files = [];
        for await (const handle of dir.values()) {
          // flat scan: the camera stores the DNGs flat on the card
          if (handle.kind === "file" && /\.dng$/i.test(handle.name)) {
            files.push(await handle.getFile());   // metadata only; reading is lazy
          }
        }
        return stripBuild(files);
      } catch (err) {
        return notice(t("folder_read", { msg: err.message }), true);
      }
    }
  }
  $("folder-file").click();
};

$("folder-file").onchange = (e) => {
  const files = Array.from(e.target.files || []).filter((f) => /\.dng$/i.test(f.name));
  e.target.value = "";   // re-picking the same folder must fire again
  stripBuild(files);
};

function stripBuild(files) {
  if (!files.length) return notice(t("folder_no_dng"), true);   // keep the old strip
  stripState.gen++;
  stripState.index = -1;
  for (const [, bmp] of stripPreviewCache) bmp.close();   // the old folder's previews
  stripPreviewCache.clear();
  stripState.entries = files
    .map((f) => ({ name: f.name, file: f, status: "idle", el: null }))
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" }));
  if (stripState.entries.length > 600) {
    stripState.entries.length = 600;   // memory: one 168x112 canvas per thumb
    notice(t("folder_cap", { n: 600 }), true);
  }
  const box = $("strip");
  box.innerHTML = "";
  for (const e of stripState.entries) box.appendChild(stripThumb(e));
  $("filmstrip").hidden = false;
  stripDecodeAll(stripState.gen);
  stripPrefetch(0);   // the first arrow likely picks the first photo
}

// ---- thumbnails --------------------------------------------------------------

function stripThumb(e) {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "thumb loading";
  b.title = e.name;   // the filename; language-neutral on purpose
  const cv = document.createElement("canvas");
  cv.width = 168;
  cv.height = 112;    // 2x the CSS size, 3:2
  b.appendChild(cv);
  const lab = document.createElement("span");
  lab.className = "thumb-name";
  lab.textContent = e.name;
  b.appendChild(lab);
  b.onclick = () => stripGoto(stripState.entries.indexOf(e));
  e.el = b;
  return b;
}

// draw `bmp` rotated by the LibRaw flip code into `cv` — the same pixel
// mapping as dng::orient in the core; the canvas takes the rotated
// dimensions (swapped for 5/6). drawImage works in rect coordinates, hence
// the full w/h offsets where orient counts pixels.
function stripDrawRotated(cv, bmp, flip) {
  const sw = bmp.width, sh = bmp.height;
  const w = flip === 5 || flip === 6 ? sh : sw;
  const h = flip === 5 || flip === 6 ? sw : sh;
  if (cv.width !== w || cv.height !== h) { cv.width = w; cv.height = h; }
  const ctx = cv.getContext("2d");
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, w, h);
  if (flip === 3) ctx.setTransform(-1, 0, 0, -1, sw, sh);
  else if (flip === 5) ctx.setTransform(0, -1, 1, 0, 0, sw);   // 90 ccw
  else if (flip === 6) ctx.setTransform(0, 1, -1, 0, sh, 0);   // 90 cw
  ctx.drawImage(bmp, 0, 0);
}

// decode one thumbnail; the status machine makes the sequential queue and the
// on-demand decode race-safe (whoever comes first claims "loading")
async function stripThumbDecode(e, gen) {
  if (e.status !== "idle") return;
  e.status = "loading";
  try {
    const r = await jpegRangeOf(e.file);
    if (!r) throw new Error("no embedded JPEG");
    e.range = r;   // the instant preview (stripShowPreview) reuses it
    const bytes = new Uint8Array(await e.file.slice(r.off, r.off + r.len).arrayBuffer());
    const bmp = await createImageBitmap(new Blob([bytes], { type: "image/jpeg" }),
      { resizeWidth: 168, resizeHeight: 112, resizeQuality: "high" });
    if (gen !== stripState.gen) { bmp.close(); return; }   // the folder changed meanwhile
    const cv = e.el.querySelector("canvas");
    stripDrawRotated(cv, bmp, r.flip);
    bmp.close();
    e.el.classList.toggle("portrait", r.flip === 5 || r.flip === 6);
    e.status = "done";
    e.el.classList.remove("loading");
  } catch {
    if (gen === stripState.gen) {
      e.status = "error";
      e.el.classList.remove("loading");
      e.el.classList.add("err");
    }
  }
}

async function stripDecodeAll(gen) {   // sequential: one 24 MP JPEG decode at a time
  for (const e of stripState.entries) {
    if (gen !== stripState.gen) return;
    await stripThumbDecode(e, gen);
  }
}

// ---- selection, arrow keys, sync ---------------------------------------------

function stripSetIndex(i) {
  stripState.index = i;
  stripState.entries.forEach((e, k) => e.el.classList.toggle("on", k === i));
}

// the instant preview: the photo's camera JPEG at ~1600 px while the raw
// pipeline renders (the busy overlay runs underneath it, hidden by the cover)
const stripPreviewCache = new Map();   // entry -> ImageBitmap, ~7 MB each, capped

async function stripPreviewBitmap(e) {
  const hit = stripPreviewCache.get(e);
  if (hit) { stripPreviewCache.delete(e); stripPreviewCache.set(e, hit); return hit; }
  const r = e.range || await jpegRangeOf(e.file);
  if (!r) return null;
  e.range = r;
  const bytes = new Uint8Array(await e.file.slice(r.off, r.off + r.len).arrayBuffer());
  const bmp = await createImageBitmap(new Blob([bytes], { type: "image/jpeg" }),
    { resizeWidth: 1600, resizeHeight: 1067, resizeQuality: "high" });
  stripPreviewCache.set(e, bmp);
  while (stripPreviewCache.size > 8) {
    const [old, oldBmp] = stripPreviewCache.entries().next().value;
    stripPreviewCache.delete(old);
    oldBmp.close();
  }
  return bmp;
}

async function stripShowPreview(e, seq) {
  try {
    const bmp = await stripPreviewBitmap(e);
    if (!bmp) return;                  // no preview: the busy overlay explains
    if (seq !== state.loadSeq) return; // moved on: the bitmap stays cached
    const cv = $("preview-cover");
    stripDrawRotated(cv, bmp, e.range.flip);
    cv.hidden = false;
  } catch { /* no preview: the busy overlay explains */ }
}

// the neighbours come up instantly on the next arrow
function stripPrefetch(i) {
  const e = stripState.entries[i];
  if (e) stripPreviewBitmap(e).catch(() => {});
}

// one raw load at a time: switching during a load only replaces the pending
// file, so skipped photos never reach the worker (fast flipping)
const stripLoader = { running: false, pending: null };
async function stripLoad(file, seq) {
  stripLoader.pending = { file, seq };
  if (stripLoader.running) return;
  stripLoader.running = true;
  while (stripLoader.pending) {
    const f = stripLoader.pending;
    stripLoader.pending = null;
    await loadDng(f.file, f.seq);
  }
  stripLoader.running = false;
}

async function stripGoto(i, { scroll = true } = {}) {
  if (i < 0 || i >= stripState.entries.length) return;
  const seq = ++state.loadSeq;      // the current photo; older loads discard their results
  const e = stripState.entries[i];
  stripSetIndex(i);
  if (scroll) e.el.scrollIntoView({ inline: "center", block: "nearest" });
  stripThumbDecode(e, stripState.gen);   // the visible photo's thumb comes first
  stripShowPreview(e, seq);        // the camera JPEG right away …
  stripPrefetch(i - 1);
  stripPrefetch(i + 1);
  stripLoad(e.file, seq);          // … the raw pipeline when it gets its turn
}

window.addEventListener("keydown", (e) => {
  if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
  if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
  if (document.querySelector("dialog[open]")) return;   // dialogs keep their own focus
  const t = e.target;
  if (t && t.closest && t.closest("input, select, textarea, [contenteditable]")) return;  // sliders use the arrows
  if (stripState.entries.length < 2) return;
  e.preventDefault();
  const d = e.key === "ArrowRight" ? 1 : -1;
  const base = stripState.index < 0 ? (d > 0 ? -1 : 0) : stripState.index;
  stripGoto(Math.max(0, Math.min(stripState.entries.length - 1, base + d)));
});

// a photo opened any other way (Open photo, drag & drop) selects its thumb
window.addEventListener("shot-shown", (e) => {
  $("preview-cover").hidden = true;   // the real preview replaces the JPEG cover
  // meta.filename is a basename; duplicate names cannot happen in a flat
  // folder pick, the first match would win if they could
  const name = String((e.detail && e.detail.filename) || "").toLowerCase();
  const i = stripState.entries.findIndex((x) => x.name.toLowerCase() === name);
  if (i >= 0) {
    stripSetIndex(i);
    stripState.entries[i].el.scrollIntoView({ inline: "center", block: "nearest" });
  }
});

// the load failed: back to the previous photo instead of a frozen cover
window.addEventListener("shot-failed", () => { $("preview-cover").hidden = true; });
