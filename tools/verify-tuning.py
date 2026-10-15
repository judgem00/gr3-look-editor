#!/usr/bin/env python3
"""Phase 0 of docs/plan-look-curves.md: run the camera's ImageTuning Make in
Unicorn and check the planned per-look curve selector before it is written
as ARM code.

    .venv/bin/python tools/verify-tuning.py            # all checks and questions
    .venv/bin/python tools/verify-tuning.py --only q2
    .venv/bin/python tools/verify-tuning.py --patched IMAGE   # an exported image

The selector is modelled in Python: a hook at 0x536dd450 (the site the stub
will branch from) does what the stub will do -- for an assigned look it
writes three curve pointers to the library slots 9..11 (.data 0x55037604)
and sets the variant SI+0x38A98 = 4, otherwise variant 0 -- and returns
through the context fill's epilogue. Cross Process Color Tone 1..3 are
three ordinary entries that point at the stock curves 3-5, 6-8 and 9-11.

q5 checks the post-gamma matrix data patch of docs/plan-post-matrix.md (a
look's own hue table) the same way.

Compared per Make: the info block (R2Y register image at +0x120 and the
LUT tables) and GammaTuning's four 256-entry tables (R, G, B, luma).
"""
import argparse
import json
import struct
import sys

from unicorn import UC_HOOK_MEM_WRITE

import emu

LIBRARY = 0x55037604            # .data, 12 channel-curve pointers
SITE = 0x536DD450               # ldrne r3,[r6,#0x2d0] after cmp r1,#21
EPILOGUE = 0x536DD464           # ldm sp, {..., pc} of the context fill
VARIANT = 0x38A98               # shooting info (info+0x82d0) + this
SELECT = 0x536EF140             # Cross Process coordinator, curve selection
CHAN_FILL = 0x536E8660          # GammaTuning output -> R, G, B, luma tables
GAMMA_TABLES = (0x2A08, 0x2C08, 0x2E08, 0x3008)   # R, G, B, luma
GAMMA_INFO = (0x2688, 0x5088)   # gamma LUTs in the info block (image 0x2568..0x4f68)
# info+0x82af: the flag routine (0x536db254) sets it when the curve filter goes
# from on to off (stock: Cross Process -> any other look). It follows the
# filter state, so it differs wherever the assignment changes that state.
TRANSITION = 0x82AF

CROSS = 21
COLOR_TONE = 0x797
# colour looks (menu index) and the settings that pick their table variants
CASES = [
    ("Standard", 11, {}), ("Vivid", 12, {}), ("Positive Film", 17, {}),
    ("Bleach Bypass", 18, {0x786: 1}), ("Bleach Bypass v2", 18, {0x786: 2}),
    ("Bleach Bypass v3", 18, {0x786: 3}), ("Retro", 19, {}),
    ("HDR", 20, {0x788: 0}), ("HDR v1", 20, {0x788: 1}), ("HDR v2", 20, {0x788: 2}),
    ("Cross Process CT1", 21, {COLOR_TONE: 0}), ("Cross Process CT2", 21, {COLOR_TONE: 1}),
    ("Cross Process CT3", 21, {COLOR_TONE: 2}), ("Negative Film", 22, {}),
]

# HDR without its effects (plan, "HDR without effects"): site, stock word, patched word
HDR_OFF = [
    (0x535131FC, None, "b_to:0x53513218"),   # local tone mapping: beq -> as Standard
    (0x536DBEB4, None, 0xE3A09000),          # ldrb sb,[r3,#0x788] -> mov sb,#0
    (0x536DC27C, None, 0xE320F000),          # beq 0x536dc818 -> nop
    (0x536DBEBC, None, 0xE3A02000),          # ldrb r2,[r3,#0x796] -> mov r2,#0 (ctx+0x1fa)
    (0x53503188, None, 0xE3A03000),          # ldrb r3,[r5,#0x796] -> mov r3,#0 (local processing)
]
LOCAL_FILL = 0x5350304C                      # local processing parameters (clarity, shading, HDR level)


def linear():
    return [64 * k for k in range(255)] + [16383]


def marker(kind):
    """test curves; each look gets a different one so mix-ups show"""
    lin = linear()
    lift = [min(16383, int(v * 1.25)) for v in lin]
    dip = [int(v * 0.8) for v in lin]
    shadow = [max(v, 1920 - 8 * k) if k < 240 else v for k, v in enumerate(lin)]
    sets = {
        "red": (lift, lin, lin), "green": (lin, lift, lin), "blue": (lin, lin, lift),
        "blue-shadow": (lin, lin, shadow), "cyan": (dip, lin, lin),
        "magenta": (lin, dip, lin), "yellow": (lin, lin, dip),
        "warm": (lift, lin, dip), "cool": (dip, lin, lift),
    }
    return sets[kind]


def compose(base, curve):
    """channel table from the computed tone curve, as 0x536e8660 does it (chan.rs)"""
    out = []
    for b in base:
        k, f = b >> 6, b & 0x3F
        if k >= 0xFF:
            out.append(curve[0xFF])
            continue
        d = (curve[k + 1] - curve[k]) * f
        if d < 0:
            d += 0x3F
        out.append((curve[k] + (d >> 6)) & 0xFFFF)
    return out


def u16s(b):
    return list(struct.unpack(f"<{len(b) // 2}H", b))


class Run:
    """one emulated ImageTuning with an optional modelled selector"""

    def __init__(self, rtos, table=None, pointers=None):
        self.t = emu.Tuning(rtos)
        e = self.e = self.t.e
        self.stock_ptrs = [e.r32(LIBRARY + 4 * i) for i in range(12)]
        self.gamma_obj = None
        e.watch[CHAN_FILL] = self._chan_fill
        self.table = None
        if table is not None:
            self.set_table(table)
        if pointers is not None:            # {key: [R, G, B] addresses} as they are
            self.table = pointers
            e.watch[SITE] = self._selector

    def _chan_fill(self, e):
        self.gamma_obj = e.reg("r0")

    def set_table(self, table):
        """table: {(look, color_tone or None): [R, G, B] curves}; None = no selector"""
        e = self.e
        self.table = {}
        # Cross Process keeps its stock curves unless the table says otherwise
        for ct in range(3):
            self.table[(CROSS, ct)] = self.stock_ptrs[3 * ct + 3:3 * ct + 6]
        for key, curves in table.items():
            if curves is None:              # explicitly without curves
                self.table[key] = None
                continue
            ptrs = []
            for c in curves:
                a = e.alloc(512)
                e.u.mem_write(a, struct.pack("<256H", *c))
                ptrs.append(a)
            self.table[key] = ptrs
        e.watch[SITE] = self._selector

    def _selector(self, e):
        look = e.reg("r1")
        si_stage, info8 = e.reg("r7"), e.reg("r6")
        si = e.r32(info8 + 0x2D0)
        key = (look, e.read(si_stage + COLOR_TONE, 1)[0] if look == CROSS else None)
        ptrs = self.table.get(key)
        if ptrs:
            for i, p in enumerate(ptrs):
                e.w32(LIBRARY + 4 * (9 + i), p)
        e.w8(si + VARIANT, 4 if ptrs else 0)
        e.u.reg_write(emu.A.UC_ARM_REG_PC, EPILOGUE)

    def make(self, look, settings=None, live=False):
        info = self.t.make(look, live=live, settings=settings)
        tables = [u16s(self.e.read(self.gamma_obj + o, 512)) for o in GAMMA_TABLES]
        return info, tables


def diff_runs(a, b, gap=16, ignore=(TRANSITION,)):
    d = [i for i in range(min(len(a), len(b))) if a[i] != b[i] and i not in ignore]
    out = []
    for i in d:
        if out and i - out[-1][1] <= gap:
            out[-1][1] = i
        else:
            out.append([i, i])
    return [(x, y - x + 1) for x, y in out]


def fmt_runs(runs):
    return ", ".join(f"{x:#x}+{n}" for x, n in runs[:8]) + (" ..." if len(runs) > 8 else "")


class Report:
    def __init__(self):
        self.failed = 0

    def check(self, ok, what, detail=""):
        print(f"  {'ok  ' if ok else 'FAIL'} {what}{(': ' + detail) if detail and not ok else ''}")
        if not ok:
            self.failed += 1


def key_of(look, settings):
    return (look, settings.get(COLOR_TONE, 0) if look == CROSS else None)


MARKERS = ["red", "green", "blue", "blue-shadow", "cyan", "magenta", "yellow", "warm", "cool"]


def assignment():
    """every colour look (and Cross Process tone) gets its own marker"""
    table, i = {}, 0
    for _, look, st in CASES:
        k = key_of(look, st)
        if k not in table:
            table[k] = marker(MARKERS[i % len(MARKERS)])
            i += 1
    return table


def check_stock(rtos, r):
    print("\n[stock] selector without assignments vs stock firmware")
    for live in (False, True):
        stock, model = Run(rtos), Run(rtos, {})
        for name, look, st in CASES:
            a, ta = stock.make(look, st, live)
            b, tb = model.make(look, st, live)
            runs = diff_runs(a, b)
            r.check(not runs and ta == tb, f"{'live ' if live else 'still'} {name}", fmt_runs(runs))


def check_assigned(rtos, r):
    print("\n[assigned] every colour look with its own curves")
    table = assignment()
    plain = {(CROSS, ct): None for ct in range(3)}
    for live in (False, True):
        stock, bare, model = Run(rtos), Run(rtos, plain), Run(rtos, table)
        for name, look, st in CASES:
            a, _ = stock.make(look, st, live)
            _, ta = bare.make(look, st, live)       # the computed curve, no channel curves
            b, tb = model.make(look, st, live)
            outside = [(x, n) for x, n in diff_runs(a, b)
                       if x < GAMMA_INFO[0] or x + n > GAMMA_INFO[1]]
            curves = table[key_of(look, st)]
            base = ta[3]
            want = [compose(base, c) for c in curves]
            luma = [(77 * R + 150 * G + 29 * B) >> 8 for R, G, B in zip(*want)]
            ok = not outside and tb[:3] == want and tb[3] == luma
            detail = (f"outside gamma {fmt_runs(outside)}" if outside else
                      f"tables {[tb[i] == w for i, w in enumerate(want + [luma])]}")
            r.check(ok, f"{'live ' if live else 'still'} {name}", detail)


def q1_order(rtos, r):
    print("\n[q1] context fill before flag, selection and channel fill in the same Make")
    points = {0x536DD270: "fill", SITE: "variant", 0x536DB160: "flag", SELECT: "select",
              0x536E7B64: "gamma", CHAN_FILL: "chan", 0x536EEDF0: "curves"}
    run = Run(rtos, assignment())
    log = []
    for a, n in points.items():
        prev = run.e.watch.get(a)
        run.e.watch[a] = (lambda n, prev: lambda e: (log.append(n), prev and prev(e)))(n, prev)
    for live in (False, True):
        for name, look, st in (CASES[0], CASES[12]):
            log.clear()
            run.make(look, st, live)
            seq = [x for x in log if x in ("variant", "flag", "select", "chan", "curves")]
            ok = seq == ["variant", "flag", "select", "chan", "curves"]
            r.check(ok, f"{'live ' if live else 'still'} {name}: {' > '.join(log)}")


def q2_sequences(rtos, r):
    print("\n[q2] history: every Make equals a fresh run of the same look")
    table = assignment()
    seqs = [
        ("A > B > A", [CASES[0], CASES[1], CASES[0]]),
        ("curves > Cross > curves", [CASES[2], CASES[10], CASES[2], CASES[12]]),
        ("all looks twice", CASES + CASES[::-1]),
    ]
    unassigned = {k: v for k, v in table.items() if k[0] != 12}   # Vivid stays stock
    for live in (False, True):
        for label, seq, tab in [*[(l, s, table) for l, s in seqs],
                                ("Vivid stock after curves", [CASES[0], CASES[1], CASES[2], CASES[1]], unassigned)]:
            run = Run(rtos, tab)
            bad = []
            for name, look, st in seq:
                got = run.make(look, st, live)
                fresh = Run(rtos, tab).make(look, st, live)
                if diff_runs(got[0], fresh[0]) or got[1] != fresh[1]:
                    bad.append(f"{name} {fmt_runs(diff_runs(got[0], fresh[0]))}")
            r.check(not bad, f"{'live ' if live else 'still'} {label}", "; ".join(bad))
    # the on -> off signal is raised exactly as stock raises it after Cross Process
    stock = Run(rtos)
    stock.make(21, {COLOR_TONE: 0})
    stock_flag = stock.make(11)[0][TRANSITION]
    run = Run(rtos, table)
    run.make(11)
    unassigned_flag = Run(rtos, unassigned)
    unassigned_flag.make(11)
    r.check(stock_flag == 1 and unassigned_flag.make(12)[0][TRANSITION] == 1,
            "curves -> no curves raises info+0x82af like Cross Process -> Standard")
    # GammaTuning caches its curve; a slider change between looks must still land
    run = Run(rtos, table)
    run.make(11, {}, True)
    got = run.make(12, {0x77E: 6}, True)
    r.check(got == Run(rtos, table).make(12, {0x77E: 6}, True), "live slider change between looks")


def q3_select(rtos, r):
    print("\n[q3] what the selection writes for a non-Cross look with variant 4")

    def writes(look, st, table):
        run = Run(rtos, table)
        run.make(look, st)                     # warm up
        log, inside = [], [False]
        e = run.e

        def enter(_):
            inside[0] = True
            ret = e.reg("lr")
            e.watch[ret] = lambda _: inside.__setitem__(0, False)
        e.watch[SELECT] = enter
        e.u.hook_add(UC_HOOK_MEM_WRITE,
                     lambda u, acc, a, size, val, _: inside[0] and a < emu.STACK - 0x100000
                     and log.append((a, size, val)))
        run.make(look, st)
        return run, log

    table = assignment()
    run_cp, cp = writes(21, {COLOR_TONE: 2}, None)
    run_std, std = writes(11, {}, table)
    t = run_std.t
    si, info = t.si, t.info

    def region(a):
        if si <= a < si + emu.SI_SIZE:
            return f"shooting info +{a - si:#x}"
        if info <= a < info + emu.INFO_SIZE:
            return f"info +{a - info:#x}"
        if emu.DATA_VMA <= a < emu.DATA_VMA + 0x1000000:
            return f"data {a:#x}"
        return f"heap {a:#x}"
    rs = sorted({region(a) for a, _, _ in std})
    rc = sorted({region(a) for a, _, _ in cp})
    print("    Standard + variant 4:", ", ".join(rs) or "-")
    print("    Cross Process CT3   :", ", ".join(rc) or "-")
    r.check(not any(x.startswith("shooting info") for x in rs), "no writes to the shooting info")
    r.check(rs == rc, "same write targets as stock Cross Process")


def patch_hdr(rtos):
    img = bytearray(rtos)
    for site, _, new in HDR_OFF:
        o = site - emu.BASE
        if isinstance(new, str):
            target = int(new.split(":")[1], 16)
            old = struct.unpack_from("<I", img, o)[0]
            new = (old & 0xFF000000) | (((target - site - 8) >> 2) & 0xFFFFFF)
        struct.pack_into("<I", img, o, new)
    return bytes(img)


def q4_hdr(rtos, r):
    print("\n[q4] HDR with its effects off vs Standard")
    patched = patch_hdr(rtos)
    for live in (False, True):
        for name, st in (("HDR", {0x788: 0}), ("HDR v1", {0x788: 1}), ("HDR v2", {0x788: 2}),
                         ("HDR v1 contrast +2", {0x788: 1, 0x77E: 6})):
            ref = Run(rtos).make(11, {k: v for k, v in st.items() if k != 0x788}, live)
            hdr = Run(patched).make(20, st, live)
            stock_hdr = Run(rtos).make(20, st, live)
            std = ref
            runs = diff_runs(std[0], hdr[0])
            print(f"    {'live ' if live else 'still'} {name}: stock HDR vs Standard "
                  f"{fmt_runs(diff_runs(std[0], stock_hdr[0])) or '-'}")
            print(f"    {'live ' if live else 'still'} {name}: patched HDR vs Standard {fmt_runs(runs) or '-'}"
                  f"{'  tables equal' if std[1] == hdr[1] else '  tables differ'}")
    r.check(True, "listed; differences must lie in HDR's own data (colour matrix, YCbCr, MultiAxial)")
    # HDR keeps the tone sliders (key 0x780, contrast 0x781/0x782) at neutral
    neutral = Run(rtos).make(11)
    for off in (0x780, 0x781, 0x782):
        hdr = Run(patched).make(20, {0x788: 1, off: 7})
        r.check(not diff_runs(neutral[0], hdr[0]) and neutral[1] == hdr[1],
                f"patched HDR ignores slider {off:#x} (= neutral Standard)")
    # the local processing fill (also used by in-camera RAW development) gets no HDR level
    def fill(img, level):
        e = emu.Emu(img)
        obj, par, si = e.alloc(0x40), e.alloc(0x40), e.alloc(0x1000)
        e.w32(obj + 0x10, par)
        e.w8(si + 0x793, 4); e.w8(si + 0x794, 4); e.w8(si + 0x796, level)
        e.call(LOCAL_FILL, obj, si, 0)
        return struct.unpack("<4H", e.read(par, 8))
    r.check([fill(rtos, v)[3] for v in range(4)] == [0, 1, 2, 3], "stock fill passes the HDR level (param +6)")
    r.check([fill(patched, v)[3] for v in range(4)] == [0, 0, 0, 0], "patched fill: HDR level always 0")
    # the patches must not touch any other look
    for name, look, st in CASES:
        if look == 20:
            continue
        r.check(Run(rtos).make(look, st) == Run(patched).make(look, st), f"patch leaves {name} alone")


# Post-gamma matrix P (docs/plan-post-matrix.md): a look gets its own hue table,
# whose 9 matrices carry H(k) B P B^-1, so the YCbCr stage computes H(k) B P.
TONE_TABLE = 0x55018E4C          # .data, per table index -> set; set word 0 -> hue table
HUE_TABLE = 0x5501988C           # the hue table every colour look shares (9 pointers)
YCC_BASE = 0x53DC1444            # RGB -> YCbCr, 3x3 int32, 256 = 1.0
CSP = (0x12A0, 0x12B2)           # info: the 9 YCbCr registers, 9 bit two's complement
HUE = 0x77E                      # shooting info: hue setting 0..8 (= -4..+4)
LUMA_MIX = [[0.5 + 0.5 * 77 / 256, 0.5 * 150 / 256, 0.5 * 29 / 256],
            [0.5 * 77 / 256, 0.5 + 0.5 * 150 / 256, 0.5 * 29 / 256],
            [0.5 * 77 / 256, 0.5 * 150 / 256, 0.5 + 0.5 * 29 / 256]]
POST_TESTS = [   # name, look, settings, tone table entries, P (rows sum to 1)
    ("Positive Film, R/B swap", 17, {}, [17], [[0, 0, 1], [0, 1, 0], [1, 0, 0]]),
    ("Negative Film, Portra 400 fit", 22, {}, [25],
     [[0.9740, 0.0928, -0.0668], [0.0810, 0.8078, 0.1112], [0.0130, 0.1346, 0.8524]]),
    ("Bleach Bypass v1-v3, luma mix", 18, {0x786: 1}, [18, 22, 23], LUMA_MIX),
    ("Bleach Bypass v2", 18, {0x786: 2}, [18, 22, 23], LUMA_MIX),
    ("Bleach Bypass v3", 18, {0x786: 3}, [18, 22, 23], LUMA_MIX),
]


def mat_mul(a, b):
    return [[sum(a[i][k] * b[k][j] for k in range(3)) for j in range(3)] for i in range(3)]


def mat_inv(m):
    (a, b, c), (d, e, f), (g, h, i) = m
    det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
    return [[(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
            [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
            [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det]]


def i32s(e, a, n=9):
    return list(struct.unpack(f"<{n}i", e.read(a, 4 * n)))


def rows(v):
    return [v[0:3], v[3:6], v[6:9]]


def csp_calc(h, base):
    """the YCbCr calc 0x536e4720: (H x base + 0x1000) >> 13, 9 bit"""
    out = []
    for i in range(3):
        for j in range(3):
            s = (sum(h[i][k] * base[k][j] for k in range(3)) + 0x1000) & 0xFFFFFFFF
            out.append((((s >> 13) & 0x1FF) ^ 0x100) - 0x100)
    return out


def csp_regs(info):
    return [((struct.unpack_from("<H", info, CSP[0] + 2 * k)[0] & 0x1FF) ^ 0x100) - 0x100
            for k in range(9)]


def post_matrices(e, p):
    """registers wanted per hue step (rows exact: 256 / 0 / 0) and the Q13 matrices that give them"""
    base = rows(i32s(e, YCC_BASE))
    inv = mat_inv(base)
    want, stored = [], []
    for k in range(9):
        h = rows(i32s(e, e.r32(HUE_TABLE + 4 * k)))
        ideal = mat_mul(mat_mul([[v / 8192 for v in r] for r in h], base), p)
        reg = [[round(v) for v in r] for r in ideal]
        # rows sum to exactly 256 / 0 / 0: move the entries that rounded the most
        for row, ideal_row, target in zip(reg, ideal, (256, 0, 0)):
            while sum(row) != target:
                step = 1 if sum(row) < target else -1
                j = max(range(3), key=lambda j: (ideal_row[j] - row[j]) * step)
                row[j] += step
        s = [[round(v * 8192) for v in r] for r in mat_mul(reg, inv)]
        want.append(sum(reg, []))
        stored.append(s)
    return base, want, stored


def install_post(e, tables, p):
    """the data patch in the emulator's RAM: set, hue table, 9 matrices; tone table entries"""
    base, want, stored = post_matrices(e, p)
    x = e.alloc(4 + 36 + 9 * 36)
    e.w32(x, x + 4)
    for k, s in enumerate(stored):
        e.w32(x + 4 + 4 * k, x + 0x28 + 36 * k)
        e.u.mem_write(x + 0x28 + 36 * k, struct.pack("<9i", *sum(s, [])))
    for t in tables:
        e.w32(TONE_TABLE + 4 * t, x)
    return base, want, stored


def q5_post(rtos, r):
    print("\n[q5] post-gamma matrix: a look's own hue table (docs/plan-post-matrix.md)")
    probe = Run(rtos)
    base = rows(i32s(probe.e, YCC_BASE))
    stock_h = [rows(i32s(probe.e, probe.e.r32(HUE_TABLE + 4 * k))) for k in range(9)]
    # the model of the calc, on stock: every hue step of Standard
    for live in (False, True):
        got = [csp_regs(probe.make(11, {HUE: k}, live)[0]) for k in range(9)]
        r.check(got == [csp_calc(h, base) for h in stock_h],
                f"{'live ' if live else 'still'} stock Standard, hue -4..+4: registers = (H x base + 0x1000) >> 13")
    for name, look, st, tables, p in POST_TESTS:
        for live in (False, True):
            tag = f"{'live ' if live else 'still'} {name}"
            run = Run(rtos)
            _, want, stored = install_post(run.e, tables, p)
            r.check(all(csp_calc(s, base) == w for s, w in zip(stored, want)),
                    f"{tag}: stored matrices give the wanted registers (model)")
            bad, outside, grey = [], [], True
            for k in range(9):
                got = run.make(look, {**st, HUE: k}, live)
                ref = Run(rtos).make(look, {**st, HUE: k}, live)
                regs = csp_regs(got[0])
                if regs != want[k]:
                    bad.append(f"hue {k - 4:+d}: {regs} vs {want[k]}")
                grey &= [sum(regs[0:3]), sum(regs[3:6]), sum(regs[6:9])] == [256, 0, 0]
                outside += [f"hue {k - 4:+d} {fmt_runs([d])}" for d in diff_runs(ref[0], got[0])
                            if d[0] < CSP[0] or d[0] + d[1] > CSP[1]]
                if ref[1] != got[1]:
                    outside.append(f"hue {k - 4:+d} gamma tables")
            r.check(not bad, f"{tag}: registers = H(k) B P for hue -4..+4", "; ".join(bad[:3]))
            r.check(grey, f"{tag}: rows 256 / 0 / 0 (grey stays grey)")
            r.check(not outside, f"{tag}: nothing else in the info block changes", "; ".join(outside[:3]))
            # every other look, in the same history right after the target, equals stock
            others = []
            for oname, olook, ost in CASES:
                if olook == look:
                    continue
                for k in (4, 8):
                    run.make(look, {**st, HUE: k}, live)
                    got = run.make(olook, {**ost, HUE: k}, live)
                    ref = Run(rtos).make(olook, {**ost, HUE: k}, live)
                    if diff_runs(ref[0], got[0]) or ref[1] != got[1]:
                        others.append(f"{oname} hue {k - 4:+d} {fmt_runs(diff_runs(ref[0], got[0]))}")
            r.check(not others, f"{tag}: every other look unchanged, also right after it", "; ".join(others[:3]))


# menu index -> target name in an export v2 preset
TARGET_NAMES = {11: "Standard", 12: "Vivid", 17: "PositiveFilm", 18: "BleechBypass_GR", 19: "Retro",
                20: "HdrCuisine", 21: "CrossProcess", 22: "NegativeFilm"}


def check_post(path, preset_path, r):
    """an exported image with post matrices (docs/plan-post-matrix.md, Phase 2): every
    target with P against its source in stock, every other look against stock"""
    print(f"\n[post] {path}: post matrices against the preset {preset_path}")
    with open(preset_path) as f:
        targets = json.load(f)["targets"]
    rtos, stock = emu.load_rtos(path), emu.load_rtos(emu.IMAGE)
    probe = Run(stock)
    base = rows(i32s(probe.e, YCC_BASE))
    hue = [rows(i32s(probe.e, probe.e.r32(HUE_TABLE + 4 * k))) for k in range(9)]
    looks = {v: k for k, v in TARGET_NAMES.items()}
    for live in (False, True):
        real, ref = Run(rtos), Run(stock)            # one history each
        for name, look, st in CASES + CASES[::-1]:
            spec = targets.get(TARGET_NAMES[look])
            src_look, src_st = look, st
            source = spec.get("source") if spec else None
            if source and source != TARGET_NAMES[look]:
                if source not in looks:
                    raise SystemExit(f"--post: source {source!r} is not modelled here")
                # a look built from another one computes as that look's first case
                src_look, src_st = next((l, s) for _, l, s in CASES if l == looks[source])
            p = spec.get("post") if spec else None
            bad = []
            for k in (0, 4, 8):
                got = real.make(look, {**st, HUE: k}, live)
                want = ref.make(src_look, {**src_st, HUE: k}, live)
                runs = diff_runs(want[0], got[0])
                outside = [d for d in runs if d[0] < CSP[0] or d[0] + d[1] > CSP[1]]
                if outside or want[1] != got[1]:
                    bad.append(f"hue {k - 4:+d}: {fmt_runs(outside) or 'gamma tables'}")
                regs = csp_regs(got[0])
                if p:
                    ideal = mat_mul(mat_mul([[v / 8192 for v in row] for row in hue[k]], base), rows(p))
                    if any(abs(regs[3 * i + j] - ideal[i][j]) > 1 for i in range(3) for j in range(3)) or \
                            [sum(regs[0:3]), sum(regs[3:6]), sum(regs[6:9])] != [256, 0, 0]:
                        bad.append(f"hue {k - 4:+d}: registers {regs}")
                elif regs != csp_regs(want[0]):
                    bad.append(f"hue {k - 4:+d}: registers changed")
            what = f"P {[round(v, 3) for v in p]}" if p else ("as its source" if spec else "stock")
            r.check(not bad, f"{'live ' if live else 'still'} {name}: {what}", "; ".join(bad[:3]))


# the selector as exported (core/src/curvesel.rs)
STUB, STUB_TABLE, ENTRIES = 0x53003D00, 0x53003D80, 15
SITE_STOCK = 0x159632D0


def stub_pointers(rtos):
    """{(look, tone|None): [R, G, B]} from the exported stub table"""
    out = {}
    for e in range(ENTRIES):
        ptrs = list(struct.unpack_from("<3I", rtos, STUB_TABLE + 12 * e - emu.BASE))
        key = (CROSS, e - 12) if e >= 12 else (11 + e, None)
        out[key] = ptrs if ptrs[0] else None
    return out


def check_patched(path, r):
    print(f"\n[patched] {path}: the real stub vs the model of it")
    rtos = emu.load_rtos(path)
    site = struct.unpack_from("<I", rtos, SITE - emu.BASE)[0]
    if site == SITE_STOCK:
        print("  selector not installed")
        r.check(False, "selector installed")
        return
    ptrs = stub_pointers(rtos)
    for k, v in sorted(ptrs.items(), key=lambda kv: (kv[0][0], kv[0][1] or 0)):
        if v:
            print(f"    look {k[0]}{'' if k[1] is None else ' tone ' + str(k[1])}: {', '.join(hex(p) for p in v)}")
    base = bytearray(rtos)
    struct.pack_into("<I", base, SITE - emu.BASE, SITE_STOCK)
    base = bytes(base)
    for live in (False, True):
        real, model = Run(rtos), Run(base, pointers=ptrs)
        for name, look, st in CASES + CASES[::-1]:       # in one history
            a, b = real.make(look, st, live), model.make(look, st, live)
            runs = diff_runs(a[0], b[0], ignore=())
            r.check(not runs and a[1] == b[1], f"{'live ' if live else 'still'} {name}", fmt_runs(runs))
    # assigned looks: the channel tables are the curves composed onto the computed one
    bare = Run(base, pointers={})
    for name, look, st in CASES:
        p = ptrs.get(key_of(look, st))
        if not p:
            continue
        _, tb = Run(rtos).make(look, st)
        base_curve = bare.make(look, st)[1][3]
        curves = [u16s(rtos[a - emu.BASE:a - emu.BASE + 512]) if a < emu.DATA_VMA
                  else u16s(Run(rtos).e.read(a, 512)) for a in p]
        want = [compose(base_curve, c) for c in curves]
        r.check(tb[:3] == want, f"{name}: R, G, B = compose(tone curve, its curves)")


STEPS = {"stock": check_stock, "assigned": check_assigned, "q1": q1_order,
         "q2": q2_sequences, "q3": q3_select, "q4": q4_hdr, "q5": q5_post}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--image", default=emu.IMAGE, help="unpacked firmware image")
    ap.add_argument("--only", choices=STEPS, action="append")
    ap.add_argument("--patched", help="exported image with the selector: check it against the model")
    ap.add_argument("--post", nargs=2, metavar=("IMAGE", "PRESET"),
                    help="exported image with post matrices and its preset (export v2)")
    args = ap.parse_args()
    r = Report()
    if args.post:
        check_post(*args.post, r)
    elif args.patched:
        check_patched(args.patched, r)
    else:
        rtos = emu.load_rtos(args.image)
        for name in args.only or STEPS:
            STEPS[name](rtos, r)
    print(f"\n{'all passed' if not r.failed else f'{r.failed} failed'}")
    sys.exit(1 if r.failed else 0)


if __name__ == "__main__":
    main()
