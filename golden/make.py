#!/usr/bin/env python3
"""Reference data for the WebAssembly core, from the Python editor.

    .venv/bin/python golden/make.py --update fwdc239b.bin [--dng a.DNG ...]

Runs the Flask editor (editor/server.py) through its test client and writes
what it answers to work/golden/. The Rust tests (core/tests) read it and must
reproduce it. Everything there derives from Ricoh's firmware or from photos,
so it stays out of git like the rest of work/.
"""
import argparse, hashlib, json, os, struct, sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(REPO, "editor"))
sys.path.insert(0, os.path.join(REPO, "fwpack"))

import chancurves  # noqa: E402
import firmware as fwmod  # noqa: E402
import fwgamma  # noqa: E402
import server  # noqa: E402

OUT = os.path.join(REPO, "work", "golden")


def setup(update):
    image = server.unpack(os.path.abspath(update))
    with open(image, "rb") as f:
        stock = f.read()
    linear = chancurves.write(stock, {i: chancurves.LINEAR for i in range(chancurves.COUNT)})
    server.state["base_fw"] = fwmod.Firmware(image, linear)
    server.state["fw"] = server.state["base_fw"].with_inheritance({})
    server.state["engine"] = fwgamma.GammaEngine(server.state["fw"].data)
    server.state["container"] = os.path.abspath(update)
    server.state["inherit"] = {}
    return server.app.test_client()


def post(client, url, body):
    r = client.post(url, json=body)
    return r.status_code, (r.get_json() if r.is_json else r.get_data(as_text=True))


def gamma_grid(rng):
    """GammaTuning on the stock base curves and on edited ones"""
    engine = server.state["engine"]
    cases, curves = [], []
    engine.set_curves({})
    for key in range(-4, 5):
        for contrast in range(-4, 5):
            for highlight in range(-4, 5):
                for shadow in range(-4, 5):
                    p = dict(key=key, contrast=contrast, highlight=highlight, shadow=shadow)
                    cases.append({**p, "gain": 0x1800, "flag": 0, "edit": None})
                    curves.append(engine.curve(gain=0x1800, **p))
    for _ in range(400):
        p = {k: int(rng.integers(-4, 5)) for k in ("key", "contrast", "highlight", "shadow")}
        gain = int(rng.integers(0x400, 0x9000))
        flag = int(rng.integers(0, 2))
        cases.append({**p, "gain": gain, "flag": flag, "edit": None})
        curves.append(engine.curve(gain=gain, flag=flag, **p))
    # random edited base curves, sometimes non-monotone or clipped
    import toneedit
    for _ in range(300):
        pts = sorted([[float(x), float(y)] for x, y in rng.random((int(rng.integers(1, 5)), 2))])
        fams = [f for f in (0, 1, 2) if rng.random() < 0.7] or [1]
        edit = toneedit.apply(engine.base, pts, tuple(fams))
        engine.set_curves(edit)
        p = {k: int(rng.integers(-4, 5)) for k in ("key", "contrast", "highlight", "shadow")}
        gain = int(rng.integers(0x400, 0x9000))
        cases.append({**p, "gain": gain, "flag": 0, "edit": {"points": pts, "families": fams}})
        curves.append(engine.curve(gain=gain, **p))
        engine.set_curves({})
    return cases, np.asarray(curves, dtype="<u2")


TONE_REQUESTS = [
    {},
    {"params": {"key": 2, "contrast": -3, "highlight": 1, "shadow": 4}, "gain": 0x2400},
    {"edit": {"points": [[0, 0], [0.25, 0.18], [0.75, 0.85], [1, 1]], "families": [0, 1, 2]}},
    {"edit": {"points": [[0, 0.1], [0.5, 0.4], [1, 0.9]], "families": [1]},
     "params": {"contrast": 2}, "gain": 0x1000},
    {"chan": {"set": [3, 4, 5], "edits": {"4": {"points": [[0, 0], [0.4, 0.55], [1, 1]]}}}},
    {"chan": {"set": [6, 7, 8], "edits": {"6": {"base": [min(65535, 70 * k) for k in range(256)],
                                                "points": [[0, 0], [1, 1]]}},
              "grade": {"Preset 2": {"shadows": {"hue": 200, "amount": 0.4, "lum": -0.1},
                                     "highlights": {"hue": 40, "amount": 0.3, "lum": 0.2},
                                     "midtones": {"hue": 0, "amount": 0, "lum": 0},
                                     "global": {"hue": 90, "amount": 0.1, "lum": 0},
                                     "balance": 0.3}}}},
]

EXPORT_REQUESTS = [
    {},
    {"matrices": {"CrossProcess": [1.1, -0.05, -0.05, -0.1, 1.2, -0.1, 0.0, -0.2, 1.2]},
     "tone": {"points": [[0, 0], [0.3, 0.25], [1, 1]], "families": [0, 1, 2]},
     "channels": {"3": [min(16383, 66 * k) for k in range(256)],
                  "9": [max(0, 64 * k - 300) for k in range(256)]},
     "matrix_edits": {"CrossProcess": {"base": [1, 0, 0, 0, 1, 0, 0, 0, 1], "sat": 1.1}}},
    {"matrices": {"Vivid": [1.3, -0.2, -0.1, -0.2, 1.3, -0.1, -0.1, -0.2, 1.3]}},
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--update", required=True)
    ap.add_argument("--dng", nargs="*", default=[])
    args = ap.parse_args()
    os.makedirs(OUT, exist_ok=True)
    rng = np.random.default_rng(1)
    client = setup(args.update)

    gold = {"firmware": client.get("/api/firmware").get_json()}
    gold["tone"] = [{"request": rq, "response": post(client, "/api/tone", rq)[1]} for rq in TONE_REQUESTS]

    gold["inherit"] = {}
    st, gold["inherit"]["NegativeFilm"] = post(client, "/api/inherit", {"target": "CrossProcess", "source": "NegativeFilm"})
    assert st == 200
    gold["tone_inherited"] = post(client, "/api/tone", TONE_REQUESTS[1])[1]
    post(client, "/api/inherit", {"target": "CrossProcess", "source": None})

    gold["export"] = []
    work = os.path.join(REPO, "work")
    for i, rq in enumerate(EXPORT_REQUESTS):
        for inherit in (None, "Retro"):
            post(client, "/api/inherit", {"target": "CrossProcess", "source": inherit})
            body = {**rq, "update_file": True, "output": os.path.join(work, "golden-export", f"e{i}", "unpackedFW-looks")}
            st, r = post(client, "/api/export", body)
            assert st == 200, r
            exp = server.state["exported"]
            packed = open(exp["update"], "rb").read()
            preset = json.load(open(exp["preset"]))
            gold["export"].append({"request": rq, "inherit": inherit,
                                   "update_sha256": hashlib.sha256(packed).hexdigest(),
                                   "update_size": len(packed),
                                   "frames_reencoded": r["frames_reencoded"], "preset": preset})
    post(client, "/api/inherit", {"target": "CrossProcess", "source": None})

    cases, curves = gamma_grid(rng)
    gold["gamma_cases"] = cases
    curves.tofile(os.path.join(OUT, "gamma.u16"))

    gold["dng"] = []
    for path in args.dng:
        with open(path, "rb") as f:
            r = client.post("/api/dng", data={"file": (f, os.path.basename(path))},
                            content_type="multipart/form-data")
        if r.status_code != 200:
            print(path, r.status_code, r.get_data(as_text=True)[:200])
            continue
        meta = r.get_json()
        stem = os.path.splitext(os.path.basename(path))[0]
        with open(os.path.join(OUT, stem + ".linear.f32"), "wb") as f:
            f.write(client.get("/api/dng/linear").get_data())
        cam = client.get("/api/dng/camera.jpg")
        if cam.status_code == 200:
            with open(os.path.join(OUT, stem + ".camera.jpg"), "wb") as f:
                f.write(cam.get_data())
        gold["dng"].append({"path": os.path.abspath(path), "stem": stem, "meta": meta})

    with open(os.path.join(OUT, "golden.json"), "w") as f:
        json.dump(gold, f)
    print(f"wrote {OUT}: {len(cases)} gamma cases, {len(gold['tone'])} tone, "
          f"{len(gold['export'])} exports, {len(gold['dng'])} DNGs")


if __name__ == "__main__":
    main()
