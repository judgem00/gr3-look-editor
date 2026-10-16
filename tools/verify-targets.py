#!/usr/bin/env python3
"""Phase 0 of docs/plan-gui-redesign.md: a colour matrix of its own for every
target, checked in Unicorn before the core writes it.

    .venv/bin/python tools/verify-targets.py
    .venv/bin/python tools/verify-targets.py --patched IMAGE --preset PRESET

--patched checks an image the core exported (TARGETS_DUMP=1 cargo test
--test targets every_target writes one to work/targets-test/) against its
v2 preset: every look that is not a target is bit-identical to stock, and
every target to its source in a stock image where the source's matrix is the
target's (targets with HSL or curves are only checked for not touching others).

A target takes its source's data by pointing its entries in the three
per-look tables at the source's (looks::inherit). The first table holds the
ColorMatrix set, so target and source then share one matrix. Here every
target gets a copy of the set and its matrix in free space (as
looks::private_set does for Cross Process and HDR) and the copy is edited.

Checked per Make (still and live view, info block and the four gamma tables):

- every look that is not a target is bit-identical to stock;
- a target with an unedited private copy is bit-identical to its source;
- a target with an edited copy is bit-identical to its source in an image
  where the source's own matrix got the same edit -- so the copy reaches the
  pipeline exactly like the stock matrix, and nothing else changes.

Bleach Bypass selects table 18, 22 or 23 by SI+0x786 (1, 2, 3) and HDR table
24 when SI+0x788 is 1 or 2 (the setting filler, 0x536dc26c ff.); the cases
cover both.
"""
import argparse
import json
import struct
import sys

import emu

VT = __import__("verify-tuning")

LOOK_TABLES = (0x55018D94, 0x55018CF0, 0x55018E4C)
CM_TABLE = LOOK_TABLES[0]
SET_WORDS = 21
FREE = 0x53003000                  # zero padding (space.rs), 0x68 per set

# (name, look byte SI+0x77c, settings) -- every colour look and its table variants
CASES = [
    ("Standard", 11, {}), ("Vivid", 12, {}), ("Positive Film", 17, {}),
    ("Bleach Bypass", 18, {0x786: 1}), ("Bleach Bypass v2", 18, {0x786: 2}),
    ("Bleach Bypass v3", 18, {0x786: 3}), ("Retro", 19, {}),
    ("HDR", 20, {0x788: 0}), ("HDR v1", 20, {0x788: 1}), ("HDR v2", 20, {0x788: 2}),
    ("Cross Process", 21, {0x797: 0}), ("Negative Film", 22, {}),
]
SATURATION = 0x77D                 # -4..+4 as 0..8; 4 = neutral


def off(a):
    """RTOS slice offset of a run-time address"""
    if a >= emu.DATA_VMA:
        return emu.DATA_LMA - emu.BASE + (a - emu.DATA_VMA)
    return a - emu.BASE


def r32(img, a):
    return struct.unpack_from("<I", img, off(a))[0]


def w32(img, a, v):
    struct.pack_into("<I", img, off(a), v)


def matrix_of(img, table):
    m = r32(img, r32(img, CM_TABLE + 4 * table) + 4)
    return m, list(struct.unpack_from("<9h", img, off(m)))


def edit(m):
    """a visible, white-preserving edit: more saturation (rows keep their sums)"""
    out = []
    for r in range(3):
        row = m[3 * r:3 * r + 3]
        s = sum(row)
        new = [round(v * 1.25 - s * 0.25 / 3) for v in row]
        new[r] += s - sum(new)
        out += new
    return out


def inherit(img, targets, source):
    for t in LOOK_TABLES:
        for tg in targets:
            w32(img, t + 4 * tg, r32(img, t + 4 * source))


TONING_WORDS = (11, 12, 13, 14)    # tables indexed by the look's toning (setting+0x60, 0x536e73d0)


def private(img, targets, at, matrix=None, toning=0):
    """copy the set the first target points at to `at` (+0x54 its matrix) and
    point every table in `targets` there; `toning` moves the toning tables so
    that the target's index 0 reads the source's entry `toning`"""
    assert all(b == 0 for b in img[off(at):off(at) + 0x68]), f"{at:#x} not free"
    cur = r32(img, CM_TABLE + 4 * targets[0])
    words = [r32(img, cur + 4 * k) for k in range(SET_WORDS)]
    m = list(struct.unpack_from("<9h", img, off(words[1])))
    struct.pack_into("<9h", img, off(at + 0x54), *(matrix(m) if matrix else m))
    words[1] = at + 0x54
    for k in TONING_WORDS:
        words[k] += 2 * toning
    for k, v in enumerate(words):
        w32(img, at + 4 * k, v)
    for tg in targets:
        w32(img, CM_TABLE + 4 * tg, at)


def set_matrix(img, table, fn):
    m, vals = matrix_of(img, table)
    struct.pack_into("<9h", img, off(m), *fn(vals))


def runs(img, cases, settings_extra):
    out = {}
    for live in (False, True):
        r = VT.Run(bytes(img))
        for name, look, st in cases:
            for sat in settings_extra:
                out[(name, live, sat)] = r.make(look, {**st, SATURATION: sat}, live)
    return out


def same_but_gamma(a, b):
    """Cross Process always runs its channel curves (even linear ones), so its
    gamma LUTs never match another look's; everything else must"""
    lo, hi = VT.GAMMA_INFO
    return not [r for r in VT.diff_runs(a[0], b[0]) if not (lo <= r[0] and r[0] + r[1] <= hi)]


def same_but_csp(a, b):
    """a target with a post matrix: everything but the nine YCbCr registers
    (those: verify-tuning.py --post)"""
    lo, hi = VT.CSP
    return not [r for r in VT.diff_runs(a[0], b[0]) if not (lo <= r[0] and r[0] + r[1] <= hi)] and a[1] == b[1]


def same(a, b):
    # info+0x82af follows the look history (verify-tuning.py, TRANSITION)
    return not VT.diff_runs(a[0], b[0]) and a[1] == b[1]


# firmware name -> (table, cases) for --patched; Retro also at toning -4 and +4
FW = {
    "Standard": (11, ["Standard"]), "Vivid": (12, ["Vivid"]), "PositiveFilm": (17, ["Positive Film"]),
    "BleechBypass_GR": (18, ["Bleach Bypass", "Bleach Bypass v2", "Bleach Bypass v3"]),
    "BleechBypass_GR \u00b7 Var. 2": (22, ["Bleach Bypass v2"]),
    "BleechBypass_GR \u00b7 Var. 3": (23, ["Bleach Bypass v3"]),
    "Retro": (19, ["Retro", "Retro toning -4", "Retro toning +4"]),
    "HdrCuisine": (20, ["HDR", "HDR v1", "HDR v2"]),
    "CrossProcess": (21, ["Cross Process"]), "NegativeFilm": (25, ["Negative Film"]),
}
PATCHED_CASES = CASES + [("Retro toning -4", 19, {0x787: 0}), ("Retro toning +4", 19, {0x787: 8})]


def check_patched(image, preset, rep):
    stock = bytearray(emu.load_rtos())
    img = bytearray(emu.load_rtos(image))
    targets = json.load(open(preset))["targets"]
    sats = (4, 8)
    pairs = ", ".join(f"{n} <- {t['source']}" for n, t in targets.items())
    print(f"\n[patched] {image}: {pairs}")
    got = runs(img, PATCHED_CASES, sats)
    base = runs(stock, PATCHED_CASES, sats)
    covered = {c for n in targets for c in FW[n][1]}
    for (name, live, sat), out in got.items():
        if name not in covered:
            rep.check(same(out, base[(name, live, sat)]), f"{'live ' if live else 'still'} {name} sat {sat - 4:+d} = stock")
    for n, t in targets.items():
        if t.get("hsl") or t.get("curves") or t.get("channels"):
            print(f"  {n}: HSL or curves, only checked for leaving the others alone")
            continue
        table, cases = FW[n]
        src_table, src_cases = FW[t["source"]]
        ref_img = bytearray(stock)
        _, m = matrix_of(img, table)
        set_matrix(ref_img, src_table, lambda _: m)
        src_case = [c for c in PATCHED_CASES if c[0] == src_cases[0]]
        ref = runs(ref_img, src_case, sats)
        eq, what = (same_but_gamma, " (but the gamma LUTs)") if n == "CrossProcess" else (same, "")
        if t.get("post"):
            if n == "CrossProcess":
                raise SystemExit("--patched: Cross Process with a post matrix is not modelled here")
            eq, what = same_but_csp, " (but the YCbCr registers: P)"
        for (name, live, sat), out in got.items():
            if name in cases:
                rep.check(eq(out, ref[(src_cases[0], live, sat)]),
                          f"{'live ' if live else 'still'} {name} sat {sat - 4:+d} = {t['source']} with {n}'s matrix{what}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--patched", help="unpacked image exported by the core")
    ap.add_argument("--preset", help="its v2 preset (targets and sources)")
    args = ap.parse_args()
    if args.patched:
        rep = VT.Report()
        check_patched(args.patched, args.preset, rep)
        print(f"\n{rep.failed} failed" if rep.failed else "\nall checks passed")
        return 1 if rep.failed else 0
    rep = VT.Report()
    stock = bytearray(emu.load_rtos())
    sats = (4, 8)                                   # neutral and +4
    base = runs(stock, CASES, sats)

    # probe: does Make see the look matrix at all?
    probe = bytearray(stock)
    set_matrix(probe, 25, edit)
    p = runs(probe, [c for c in CASES if c[0] == "Negative Film"], sats)
    rep.check(not same(p[("Negative Film", False, 4)], base[("Negative Film", False, 4)]),
              "probe: an edited matrix changes Make's output")

    # name, target tables, source table, edited?, toning index of the source
    # (SI+0x786 for Bleach Bypass; Retro's look fills 4)
    scenarios = [
        ("Vivid <- Negative Film, copy", [12], 25, False, 0),
        ("Vivid <- Negative Film, edited", [12], 25, True, 0),
        ("Standard <- Standard, edited (HDR, Cross stay)", [11], 11, True, 0),
        ("Bleach Bypass 18/22/23 <- Positive Film, edited", [18, 22, 23], 17, True, 0),
        ("Positive Film <- Bleach Bypass v3, edited, no toning shift", [17], 23, True, 0),
        ("Positive Film <- Bleach Bypass v3, edited", [17], 23, True, 3),
        ("Vivid <- Bleach Bypass, edited", [12], 18, True, 1),
        ("Negative Film <- Retro, edited", [25], 19, True, 4),
    ]
    name_of = {11: "Standard", 12: "Vivid", 17: "Positive Film", 18: "Bleach Bypass",
               19: "Retro", 22: "Bleach Bypass v2", 23: "Bleach Bypass v3", 25: "Negative Film"}
    expect_fail = {"Positive Film <- Bleach Bypass v3, edited, no toning shift"}
    target_cases = {12: ["Vivid"], 11: ["Standard"], 17: ["Positive Film"], 25: ["Negative Film"],
                    18: ["Bleach Bypass", "Bleach Bypass v2", "Bleach Bypass v3"]}
    for title, targets, source, edited, toning in scenarios:
        print(f"\n[{title}]")
        img = bytearray(stock)
        inherit(img, targets, source)
        private(img, targets, FREE, edit if edited else None, toning)
        got = runs(img, CASES, sats)
        # reference: the source look in an image where its own matrix got the edit
        ref_img = bytearray(stock)
        if edited:
            set_matrix(ref_img, source, edit)
        src_case = [c for c in CASES if c[0] == name_of[source]]
        ref = runs(ref_img, src_case, sats)
        tnames = target_cases[targets[0]]
        for (name, live, sat), out in got.items():
            tag = f"{'live ' if live else 'still'} {name} sat {sat - 4:+d}"
            if name in tnames:
                ok = same(out, ref[(name_of[source], live, sat)])
                if title in expect_fail:     # documents why the shift is needed
                    rep.check(not ok, f"{tag} differs from {name_of[source]} (expected)")
                else:
                    rep.check(ok, f"{tag} = {name_of[source]}{' with the edit' if edited else ''}")
            else:
                rep.check(same(out, base[(name, live, sat)]), f"{tag} = stock")
    print(f"\n{rep.failed} failed" if rep.failed else "\nall checks passed")
    return 1 if rep.failed else 0


if __name__ == "__main__":
    sys.exit(main())
