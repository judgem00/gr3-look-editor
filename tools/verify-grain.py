#!/usr/bin/env python3
"""Grain for every colour look (docs/grain.md): run the camera's own gate and
parameter fill of the DigitalFilterProcess in Unicorn.

    .venv/bin/python tools/verify-grain.py                    # stock: how grain works
    .venv/bin/python tools/verify-grain.py --patched work/grain-test/unpackedFW

Stock: the gate (0x535131c0) runs the filter for the monochrome looks and
Negative Film when their grain (SI+0x795) is set, for the other colour looks
only for shading or clarity; the fill (0x5350304c) passes the grain 0..3 into
the filter's parameters (+4) for every look.

Patched: looks with a grain of their own (the table after the stubs) run the
filter and get that grain whatever SI+0x795 says; every other look, and every
input with no grain of ours, behaves exactly as stock.
"""
import argparse
import struct
import sys

import emu

GATE = 0x535131C0
FILL = 0x5350304C
FILL_SITE = 0x535031B0
LOOKS = range(11, 23)
NAMES = {11: "Standard", 12: "Vivid", 13: "MonotoneGR", 14: "SoftMonotone", 15: "HardMonotone",
         16: "HighContrastBW", 17: "PositiveFilm", 18: "BleechBypass_GR", 19: "Retro", 20: "HdrCuisine",
         21: "CrossProcess", 22: "NegativeFilm"}
# (shading 0x793, clarity 0x794, grain 0x795, HDR level 0x796); 4 = 0 on the menu's -4..+4
INPUTS = [(4, 4, g, 0) for g in range(5)] + [(5, 4, 0, 0), (4, 2, 1, 0), (4, 4, 0, 2), (4, 4, 2, 1)]


class Report:
    def __init__(self):
        self.failed = 0

    def check(self, ok, what):
        print(f"  {'ok  ' if ok else 'FAIL'} {what}")
        if not ok:
            self.failed += 1


def settings(e, look, inp):
    si = e.alloc(0x1000)
    e.w8(si + 0x77C, look)
    for off, v in zip((0x793, 0x794, 0x795, 0x796), inp):
        e.w8(si + off, v)
    return si


def gate(rtos, look, inp):
    """obj+0x24: does the DigitalFilterProcess run?"""
    e = emu.Emu(rtos)
    obj = e.alloc(0x40)
    e.w8(obj + 0x24, 0x55)
    e.call(GATE, obj, settings(e, look, inp))
    return e.read(obj + 0x24, 1)[0]


def fill(rtos, look, inp):
    """the filter's parameters: shading, clarity, grain, HDR level"""
    e = emu.Emu(rtos)
    obj, par = e.alloc(0x40), e.alloc(0x40)
    e.w32(obj + 0x10, par)
    e.call(FILL, obj, settings(e, look, inp), 0)
    return struct.unpack("<4h", e.read(par, 8))


def stock_checks(rtos, r):
    print("\n[stock] who gets grain")
    grain_looks = {13, 15, 16, 22}
    for look in LOOKS:
        runs = [gate(rtos, look, (4, 4, g, 0)) for g in range(4)]
        if look in grain_looks:
            r.check(runs == [0, 1, 1, 1], f"{NAMES[look]}: the filter runs for grain 1..3 {runs}")
        elif look == 14:
            r.check(runs == [1, 1, 1, 1], f"{NAMES[look]}: the filter always runs {runs}")
        else:
            r.check(runs == [0, 0, 0, 0], f"{NAMES[look]}: grain alone does not run the filter {runs}")
    print("\n[stock] the fill passes the grain for every look")
    for look in LOOKS:
        got = [fill(rtos, look, (4, 4, g, 0))[2] for g in range(5)]
        r.check(got == [0, 1, 2, 3, 0], f"{NAMES[look]}: SI+0x795 0..4 -> parameter +4 {got}")


def table_of(rtos):
    site = struct.unpack_from("<I", rtos, FILL_SITE - emu.BASE)[0]
    if site >> 24 != 0xEB:
        return None
    off = ((site & 0xFFFFFF) << 8)
    off = (off - (1 << 32) if off & (1 << 31) else off) >> 6
    stub = FILL_SITE + 8 + off
    t = stub + 10 * 4
    return list(rtos[t - emu.BASE:t - emu.BASE + 12])


def patched_checks(stock, rtos, r):
    table = table_of(rtos)
    print(f"\n[patched] grain table (looks 11..22): {table}")
    if table is None:
        r.check(False, "grain stubs installed")
        return
    for look in LOOKS:
        ours = table[look - 11]
        for inp in INPUTS:
            g0, g1 = gate(stock, look, inp), gate(rtos, look, inp)
            f0, f1 = fill(stock, look, inp), fill(rtos, look, inp)
            if ours:
                ok = g1 == 1 and f1 == (f0[0], f0[1], ours, f0[3])
            else:
                ok = g1 == g0 and f1 == f0
            if not ok:
                r.check(False, f"{NAMES[look]} {inp}: gate {g0}->{g1}, fill {f0}->{f1}")
        r.check(True, f"{NAMES[look]}: " + (f"grain {ours}, filter on, shading/clarity/HDR as stock"
                                             if ours else "as stock") + f" ({len(INPUTS)} inputs)")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--image", default=emu.IMAGE, help="unpacked stock firmware image")
    ap.add_argument("--patched", help="exported image with grain: check it against stock")
    args = ap.parse_args()
    r = Report()
    stock = emu.load_rtos(args.image)
    if args.patched:
        patched_checks(stock, emu.load_rtos(args.patched), r)
    else:
        stock_checks(stock, r)
    print(f"\n{'all passed' if not r.failed else f'{r.failed} failed'}")
    sys.exit(1 if r.failed else 0)


if __name__ == "__main__":
    main()
