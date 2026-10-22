#!/usr/bin/env python3
"""Unicorn check of the video patch group osd2 (core/src/video.rs).

Applies the osd2 patches as written in video.rs to the stock image and runs
both patch sites through their stub and back:

- shutter formatter 0x532aacc4, from the patched `cmp r5, #0` at 0x532aad04:
  hidden (empty string, branch to 0x532aad38) exactly when movie and the
  exposure mode is not M (5); otherwise on to 0x532aad14 with the buffer
  untouched;
- aperture item 0x53173a5c, from the patched `ldrb r2, [r3, #0x940]` at
  0x53173a94: r2 (the formatter's "hide") is the movie flag unless the mode
  is M, and every other register is kept;
- the layout and position tables: items 30/31 on in the movie row, at the
  places of the still layouts.

    .venv/bin/python tools/test-osd2.py [IMAGE]
"""
import os
import re
import sys

from unicorn import Uc, UC_ARCH_ARM, UC_MODE_ARM, UC_HOOK_CODE
from unicorn import arm_const as A

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMAGE = sys.argv[1] if len(sys.argv) > 1 else os.environ.get("GR_IMAGE") or \
    os.path.join(REPO, "..", "gr3x-fw-hack", "assets", "fwdc239b", "unpackedFW")
BIAS = 0x52FEF790
BASE, END = 0x53000000, 0x55000000
DATA, STACK = 0x60000000, 0x61000000


def osd2_patches():
    """(va, stock, new) of the osd2 group, read from video.rs"""
    src = open(os.path.join(REPO, "core", "src", "video.rs")).read()
    stub = "".join(re.findall(r'"([0-9a-f]{8})", //',
                              src.split("const OSD2_STUB")[1].split(");")[0]))
    group = src.split("\"osd2\",\n")[1].split("\n    ),")[0]
    out = []
    for va, stock, new in re.findall(
            r'va: 0x([0-9A-F_]+),\s*stock: "([0-9a-f]+)",\s*new: ([A-Z0-9_]+|"[0-9a-f]+")', group):
        new = stub if new == "OSD2_STUB" else new.strip('"')
        out.append((int(va.replace("_", ""), 16), bytes.fromhex(stock), bytes.fromhex(new)))
    assert len(out) == 6, out
    return out


def image():
    img = bytearray(open(IMAGE, "rb").read()[BASE - BIAS:END - BIAS])
    for va, stock, new in osd2_patches():
        o = va - BASE
        assert img[o:o + len(stock)] == stock, hex(va)
        assert len(new) == len(stock)
        img[o:o + len(new)] = new
    return bytes(img)


def emu(img):
    uc = Uc(UC_ARCH_ARM, UC_MODE_ARM)
    uc.mem_map(BASE, END - BASE)
    uc.mem_write(BASE, img)
    uc.mem_map(DATA, 0x10000)
    uc.mem_map(STACK - 0x10000, 0x10000)
    return uc


def run(uc, start, stops, regs):
    """run from start until one of stops; returns the stop address"""
    for r in range(13):
        uc.reg_write(A.UC_ARM_REG_R0 + r, 0x1000 + r)
    uc.reg_write(A.UC_ARM_REG_SP, STACK - 0x100)
    uc.reg_write(A.UC_ARM_REG_LR, 0xDEAD0000)
    for k, v in regs.items():
        uc.reg_write(k, v)
    hit = []

    def hook(uc, addr, size, _):
        if addr in stops:
            hit.append(addr)
            uc.emu_stop()
    h = uc.hook_add(UC_HOOK_CODE, hook)
    uc.emu_start(start, 0, count=64)
    uc.hook_del(h)
    assert hit, f"no stop reached from {start:#x}"
    return hit[0]


def main():
    img = image()
    uc = emu(img)
    fails = 0

    # shutter formatter
    buf = DATA + 0x100
    for movie in (0, 1):
        for mode in (0, 1, 2, 4, 5, 7, 8):
            uc.mem_write(buf, b"\xaa" * 4)
            stop = run(uc, 0x532AAD04, {0x532AAD14, 0x532AAD38},
                       {A.UC_ARM_REG_R5: movie, A.UC_ARM_REG_R7: mode, A.UC_ARM_REG_R6: buf})
            hidden = stop == 0x532AAD38
            want = bool(movie and mode != 5)
            first = uc.mem_read(buf, 1)[0]
            ok = hidden == want and (first == 0 if hidden else first == 0xAA)
            # registers the formatter goes on with
            ok &= uc.reg_read(A.UC_ARM_REG_R5) == movie and uc.reg_read(A.UC_ARM_REG_R7) == mode
            ok &= uc.reg_read(A.UC_ARM_REG_R6) == buf and uc.reg_read(A.UC_ARM_REG_R4) == 0x1004
            fails += not ok
            print(f"shutter  movie={movie} mode={mode}: {'hidden' if hidden else 'shown '}"
                  f" {'ok' if ok else 'FAIL'}")

    # aperture item
    model = DATA + 0x1000
    for movie in (0, 1):
        for mode in (0, 2, 4, 5):
            uc.mem_write(model + 0x940, bytes([movie]))
            uc.mem_write(model + 0xB98, bytes([mode]))
            before = {r: 0x1000 + (r - A.UC_ARM_REG_R0) for r in range(A.UC_ARM_REG_R0, A.UC_ARM_REG_R12)}
            before[A.UC_ARM_REG_R3] = model
            stop = run(uc, 0x53173A94, {0x53173A98}, {A.UC_ARM_REG_R3: model})
            r2 = uc.reg_read(A.UC_ARM_REG_R2)
            want = movie if mode != 5 else 0
            ok = r2 == want
            for r, v in before.items():
                if r != A.UC_ARM_REG_R2:
                    ok &= uc.reg_read(r) == v
            ok &= uc.reg_read(A.UC_ARM_REG_SP) == STACK - 0x100
            fails += not ok
            print(f"aperture movie={movie} mode={mode}: hide={r2} {'ok' if ok else 'FAIL'}")

    # the layout table: movie row, items 30 and 31 on, the rest unchanged
    stock = open(IMAGE, "rb").read()[BASE - BIAS:END - BIAS]
    row = 0x53B30B48 + 0x2F - BASE
    on = [k for k in range(1, 47) if img[row + k]]
    changed = [k for k in range(1, 47) if img[row + k] != stock[row + k]]
    ok = changed == [30, 31]
    fails += not ok
    print(f"layout   movie row changed items {changed}, on: {len(on)} of 46 {'ok' if ok else 'FAIL'}")

    # positions: in the movie layouts (1 = LCD, 3 = HDMI) items 30/31 get
    # the places they have in the still layouts (0, 2)
    import struct
    def pos(layout, item):
        return struct.unpack_from("<ii", img, 0x53B2FC5C + 0x178 * layout + 8 * item - BASE)
    for movie, still in ((1, 0), (3, 2)):
        for item in (30, 31):
            ok = pos(movie, item) == pos(still, item) != (0, 0)
            fails += not ok
            print(f"position layout {movie} item {item}: {pos(movie, item)} {'ok' if ok else 'FAIL'}")

    print("FAILED" if fails else "passed", f"({fails} failures)")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
