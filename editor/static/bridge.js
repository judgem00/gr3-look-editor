// api(url, options) answers the editor's /api/... requests like fetch().
//
// There is no server: the requests go to worker.js, which runs the Rust core
// as WebAssembly. Before the first request the user drops Ricoh's update file
// in; it is checked against the known build and never leaves the browser.

const api = (() => {
  let worker = null;
  let seq = 0;
  const pending = new Map();
  const shot = { linear: null, camera: null };
  let urls = [];
  let firmwareReady = null;

  function call(op, args, transfer = []) {
    if (!worker) {
      worker = new Worker("worker.js", { type: "module" });
      worker.onmessage = ({ data }) => {
        const p = pending.get(data.id);
        pending.delete(data.id);
        if (data.ok) p.resolve(data.result); else p.reject(new Error(data.error));
      };
      worker.onerror = (e) => {
        for (const p of pending.values()) p.reject(new Error(e.message || "worker failed"));
        pending.clear();
      };
    }
    const id = ++seq;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      worker.postMessage({ id, op, args }, transfer);
    });
  }

  const json = (v) => new Response(JSON.stringify(v), { headers: { "Content-Type": "application/json" } });
  const fail = (msg, status = 400) => new Response(`<p>${esc(msg)}</p>`, { status, headers: { "Content-Type": "text/html" } });

  // ---- the firmware screen ----------------------------------------------------

  function gate() {
    if (firmwareReady) return firmwareReady;
    firmwareReady = new Promise((resolve) => {
      const el = document.getElementById("fw-gate");
      const msg = document.getElementById("gate-msg");
      const input = document.getElementById("gate-file");
      el.hidden = false;
      let busy = false;
      const take = async (file) => {
        if (!file || busy) return;
        busy = true;
        el.classList.add("busy");
        msg.className = "note";
        msg.textContent = t("gate_checking");
        try {
          const bytes = await file.arrayBuffer();
          await call("open", { bytes, name: file.name }, [bytes]);
          el.hidden = true;
          resolve();
        } catch (err) {
          msg.className = "note warn";
          msg.textContent = t("gate_error", { msg: err.message });
        } finally {
          busy = false;
          el.classList.remove("busy");
        }
      };
      input.onchange = () => { take(input.files[0]); input.value = ""; };
      el.addEventListener("dragover", (e) => { e.preventDefault(); el.classList.add("drag"); });
      el.addEventListener("dragleave", () => el.classList.remove("drag"));
      el.addEventListener("drop", (e) => {
        e.preventDefault();
        el.classList.remove("drag");
        take(e.dataTransfer.files[0]);
      });
    });
    return firmwareReady;
  }

  // ---- the routes ------------------------------------------------------------------

  async function web(url, opts = {}) {
    const method = (opts.method || "GET").toUpperCase();
    const path = url.replace(/^\//, "").split("?")[0];
    const body = typeof opts.body === "string" ? opts.body : "{}";
    try {
      if (path === "api/firmware") {
        await gate();
        return json(await call("firmware"));
      }
      if (path === "api/inherit" && method === "POST") return json(await call("inherit", { body }));
      if (path === "api/tone" && method === "POST") return json(await call("tone", { body }));
      if (path === "api/hsl" && method === "POST") return json(await call("hsl", { body }));
      if (path === "api/effects" && method === "POST") return json(await call("effects", { body }));
      if (path === "api/target" && method === "POST") return json(await call("target", { body }));
      if (path === "api/space" && method === "POST") return json(await call("space", { body }));
      if (path === "api/dng" && method === "POST") {
        const file = opts.body.get("file");
        const bytes = await file.arrayBuffer();
        const r = await call("dng", { bytes, name: file.name }, [bytes]);
        shot.linear = r.linear;
        shot.camera = r.camera;
        return json(r.meta);
      }
      if (path === "api/dng") return new Response(null, { status: 204 });
      if (path === "api/dng/linear") return shot.linear ? new Response(shot.linear) : fail("no photo", 404);
      if (path === "api/dng/camera.jpg") return shot.camera ? new Response(shot.camera) : fail("no camera JPEG", 404);
      if (path === "api/export" && method === "POST") {
        const r = await call("export", { body });
        for (const u of urls) URL.revokeObjectURL(u);
        const update = URL.createObjectURL(new Blob([r.update], { type: "application/octet-stream" }));
        const preset = URL.createObjectURL(new Blob([r.preset], { type: "application/json" }));
        urls = [update, preset];
        return json({ ...r.result, web: true, downloads: { update, preset } });
      }
      return fail(`no such request: ${method} ${url}`, 404);
    } catch (err) {
      return fail(err.message);
    }
  }

  return web;
})();
