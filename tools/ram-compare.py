"""Compare a RAM dump of the camera with firmware images (phase 0 of
docs/plan-ram-loader.md; the dump comes from tools/ramload, see
docs/firmware/ram-loader.md).

    python3 tools/ram-compare.py DUMPDIR [IMAGE ...] [--builds]

DUMPDIR holds RTOS.BIN, DATA.BIN and INFO.TXT from the card
(SCRIPT/RAMLOAD). Each IMAGE is a firmware file (FWDC239B.BIN) or an unpacked
image. The stock image (work/fwdc239b/unpackedFW) is always a candidate;
--builds adds every FWDC239B*.BIN under work/.

It answers three questions:

1. Which candidate does the camera run? The one whose code and read-only
   data (0x53000000 up to the .data copy source) differ least from the dump.
2. Is the editor's free space (0x53000d50..0x53004000, core/src/space.rs)
   unchanged at run time?
3. Do the export sites still hold their file values at run time? The export
   sites are the bytes in which any candidate differs from stock, in the
   image and in .data. If the firmware writes them at startup, a RAM patch
   has to know.
"""
import argparse
import glob
import os
import struct
import sys
import zlib

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(REPO, "fwpack"))
import grfw  # noqa: E402

STOCK = os.path.join(REPO, "work", "fwdc239b", "unpackedFW")

# core/src/addr.rs
BIAS = 0x52FEF790
RTOS = 0x53000000
DATA_VMA, DATA_LMA, DATA_SIZE = 0x55000000, 0x5407B540, 0x3A080
# core/src/space.rs: PADDING and RESERVED
POOL = [(0x53000D50, 0x53002000), (0x53002000, 0x53002100),
        (0x53002100, 0x53003D00), (0x53003D00, 0x53004000)]
# core/src/looks.rs: the per-look pointer tables (ColorMatrix, MultiAxial,
# YCbCr), 4-byte entries
TABLES = {0x55018D94: "ColorMatrix table", 0x55018CF0: "MultiAxial table",
          0x55018E4C: "YCbCr table"}
TABLE_ENTRIES = 0x29  # 0x55018cf0 + 41 * 4 = 0x55018d94: generous for each


def load_image(path):
    with open(path, "rb") as f:
        data = f.read()
    if data[:5] == b"RICOH":
        return bytes(grfw.Firmware(data).decode())
    return data


def info(dumpdir):
    values = {}
    with open(os.path.join(dumpdir, "INFO.TXT")) as f:
        for line in f:
            parts = line.split()
            if len(parts) == 2 and parts[1].startswith("0x"):
                values[parts[0]] = int(parts[1], 16)
    return values


def crc(data):
    return zlib.crc32(data) & 0xFFFFFFFF


def runs(a, b, base, gap=16):
    """[(start, end)] in RTOS addresses where a and b differ, joined across
    gaps shorter than `gap`"""
    out = []
    n = min(len(a), len(b))
    i = 0
    # compare in 4 KiB blocks first, most are equal
    for blk in range(0, n, 4096):
        if a[blk:blk + 4096] == b[blk:blk + 4096]:
            continue
        for i in range(blk, min(blk + 4096, n)):
            if a[i] != b[i]:
                addr = base + i
                if out and addr - out[-1][1] < gap:
                    out[-1][1] = addr + 1
                else:
                    out.append([addr, addr + 1])
    return [tuple(r) for r in out]


def count(rs):
    return sum(e - s for s, e in rs)


def where(addr):
    for lo, hi in POOL:
        if lo <= addr < hi:
            return f"free space {lo:#x}..{hi:#x}"
    for t, name in TABLES.items():
        if t <= addr < t + TABLE_ENTRIES * 4:
            return f"{name} [{(addr - t) // 4}]"
    if RTOS <= addr < DATA_LMA:
        return "code/rodata"
    if DATA_LMA <= addr < DATA_LMA + DATA_SIZE:
        return f".data copy source (= {addr - DATA_LMA + DATA_VMA:#x})"
    if DATA_LMA + DATA_SIZE <= addr < DATA_VMA:
        return "after the image (up to the 4 KiB rounding)"
    if DATA_VMA <= addr < DATA_VMA + DATA_SIZE:
        return ".data"
    if addr >= DATA_VMA + DATA_SIZE:
        return "BSS"
    return "?"


class Candidate:
    def __init__(self, name, image, rtos_end):
        self.name = name
        self.rtos = image[RTOS - BIAS:rtos_end - BIAS]
        off = DATA_LMA - BIAS
        self.data = image[off:off + DATA_SIZE]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("dump")
    ap.add_argument("images", nargs="*")
    ap.add_argument("--builds", action="store_true",
                    help="add every FWDC239B*.BIN under work/")
    ap.add_argument("--stock", default=STOCK)
    ap.add_argument("--max-runs", type=int, default=40)
    a = ap.parse_args()

    meta = info(a.dump)
    with open(os.path.join(a.dump, "RTOS.BIN"), "rb") as f:
        rtos = f.read()
    with open(os.path.join(a.dump, "DATA.BIN"), "rb") as f:
        data = f.read()
    print(f"dump {a.dump}: RTOS {len(rtos):#x} bytes, DATA {len(data):#x} bytes")
    for key, buf in (("rtos_crc32", rtos), ("data_crc32", data)):
        if key in meta and meta[key] != crc(buf):
            print(f"  WARNING: {key} {meta[key]:#010x} != file {crc(buf):#010x} (copy incomplete?)")
    lits = [meta.get(k) for k in ("literal_data_source", "literal_data_start", "literal_data_size")]
    if lits != [DATA_LMA, DATA_VMA, DATA_SIZE]:
        print(f"  WARNING: startup literals {lits} differ from addr.rs; not 2.10 / fwdc239b?")
    rtos_end = RTOS + len(rtos)

    paths = list(a.images)
    if a.builds:
        paths += sorted(glob.glob(os.path.join(REPO, "work", "*", "FWDC239B*.BIN")))
    cands = [Candidate("stock", load_image(a.stock), rtos_end)]
    for p in paths:
        try:
            cands.append(Candidate(os.path.relpath(p, REPO), load_image(p), rtos_end))
        except (OSError, grfw.FormatError) as e:
            print(f"  skipped {p}: {e}")
    stock = cands[0]

    # 1. which firmware
    code_len = DATA_LMA - RTOS
    print("\n1. Code and read-only data (0x53000000..0x5407b540) against each candidate:")
    scored = []
    for c in cands:
        r = runs(rtos[:code_len], c.rtos[:code_len], RTOS)
        scored.append((count(r), len(r), c))
    scored.sort(key=lambda s: s[0])
    for n, k, c in scored:
        print(f"   {n:9d} bytes in {k:5d} runs   {c.name}")
    best = scored[0][2]
    print(f"   -> closest: {best.name}")

    # differences of the dump against the closest candidate
    print(f"\n   Runs in which the dump differs from {best.name} (RTOS image):")
    diff = runs(rtos, best.rtos, RTOS)
    for s, e in diff[:a.max_runs]:
        print(f"     {s:#010x}..{e:#010x} {e - s:6d} B  {where(s)}")
    if len(diff) > a.max_runs:
        print(f"     ... {len(diff) - a.max_runs} more")

    # 2. free space
    print("\n2. Editor free space at run time:")
    for lo, hi in POOL:
        r = runs(rtos[lo - RTOS:hi - RTOS], best.rtos[lo - RTOS:hi - RTOS], lo)
        state = "unchanged" if not r else f"CHANGED, {count(r)} bytes: " + \
            ", ".join(f"{s:#x}..{e:#x}" for s, e in r[:8])
        print(f"   {lo:#x}..{hi:#x}  {state}")

    # 3. export sites
    sites = []
    for c in cands[1:]:
        sites += [(s, e, "image") for s, e in runs(stock.rtos, c.rtos, RTOS)]
        sites += [(s, e, "data") for s, e in runs(stock.data, c.data, DATA_VMA)]
    for t in TABLES:
        sites.append((t, t + TABLE_ENTRIES * 4, "data"))
    sites = sorted(set(sites))
    print(f"\n3. Export sites ({len(sites)} ranges from {len(cands) - 1} builds and the look tables), "
          f"dump against {best.name}:")
    changed = 0
    for s, e, kind in sites:
        if kind == "image":
            got, want = rtos[s - RTOS:e - RTOS], best.rtos[s - RTOS:e - RTOS]
        else:
            got, want = data[s - DATA_VMA:e - DATA_VMA], best.data[s - DATA_VMA:e - DATA_VMA]
        if got != want:
            changed += 1
            r = runs(got, want, s, gap=1)
            print(f"   {s:#010x}..{e:#010x}  {where(s)}: {count(r)} bytes differ at run time")
            for rs, re_ in r[:4]:
                o = rs - s
                print(f"       {rs:#x}: file {want[o:o + min(re_ - rs, 16)].hex()} "
                      f"ram {got[o:o + min(re_ - rs, 16)].hex()}")
    if not changed:
        print("   all unchanged")

    # .data overall, for context: run-time variables change, that is expected
    d = runs(data[:DATA_SIZE], best.data, DATA_VMA)
    print(f"\n.data overall: {count(d)} bytes in {len(d)} runs differ from the file "
          f"(run-time variables; only the export sites above matter)")


if __name__ == "__main__":
    main()
