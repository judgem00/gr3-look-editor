"""Run NDMON.BIN in Unicorn against a simulated camera memory, as a test
before the camera (docs/firmware/nd-monitor.md).

    .venv/bin/python tools/ramload/test-ndmon.py OUTDIR [IMAGE]

The memory is that of test-dump.py (image, .data, BSS) plus the object heap up to
0x5a000000 (and the UserData page), with a ND driver object, an AE context and a LiveViewProcess
object put in where the monitor looks for them. Time is simulated: each
nanosleep moves the clock on, and at fixed times the test changes fields
the way an ND press would. The monitor must log exactly those changes, find
the LiveViewProcess object and never write memory.
"""
import importlib.util
import re
import os
import struct
import sys

from unicorn import Uc, UC_ARCH_ARM, UC_MODE_ARM, UC_HOOK_INTR, UC_HOOK_MEM_WRITE, UC_PROT_READ
from unicorn import arm_const as A

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
spec = importlib.util.spec_from_file_location("test_dump", os.path.join(HERE, "test-dump.py"))
td = importlib.util.module_from_spec(spec)
spec.loader.exec_module(td)

MAP_START, MAP_END = 0x55000000, 0x5A000000
ND_OBJ, AE_SERVICE, LVP_VTABLE = 0x5536DC58, 0x55085104, 0x53C1D000
AE, LVP, ND_COUNTERS = 0x597CDCF0, 0x59900000, 0x59A00000
# fake heap objects: a decoy with the vtable but another AE context first
DECOY = 0x59600000
UD = 0x5D520C80

# (ms, address, word): what an ND press changes
EVENTS = [
    (5000, LVP + 0x9F0C, 0x00000100),   # ND change pending
    (5040, ND_OBJ + 0x38, 0x00000001),  # filter in
    (5040, ND_COUNTERS + 0x54, 1),      # one move
    (5060, LVP + 0x9F0C, 0),
    (20000, AE + 0x5694, 0x20),         # AE request bit
    (21000, 0x55400000, 0x41414141),    # the RTOS text log ring: left out
    (25000, UD + 0x530, 0x01),          # MovieNdFilter on
]
EXPECT = [
    "lvp+9f0c 0x00000000 -> 0x00000100",
    "nd_state 0x00000000 -> 0x00000001",
    "nd_moves 0x00000000 -> 0x00000001",
    "lvp+9f0c 0x00000100 -> 0x00000000",
    "ae+5694 0x00000000 -> 0x00000020",
    "ud+530 0x00000002 -> 0x00000001",
]


def build_ram(image):
    ram = td.physical(image)
    data = ram[td.DATA_VMA]
    data.extend(bytes(MAP_END - td.DATA_VMA - len(data)))
    ram[UD & ~0xFFF] = bytearray(0x2000)  # the UserData page, far above

    def w(addr, word):
        for base, buf in ram.items():
            if base <= addr < base + len(buf):
                struct.pack_into("<I", buf, addr - base, word)

    w(AE_SERVICE + 0x10, AE)
    w(DECOY, LVP_VTABLE)
    w(DECOY + 0xAA0, AE + 0x1000)
    w(LVP, LVP_VTABLE)
    w(LVP + 0xAA0, AE)
    w(LVP + 0xA94, LVP + 0x8000)
    w(LVP + 0x728, 0x10)
    w(ND_OBJ + 0x28, ND_COUNTERS - 0x100)
    w(ND_COUNTERS - 0x100 + 4, ND_COUNTERS)
    w(0x55063F34, UD)
    w(UD + 0x44, 1)
    w(UD + 0x530, 2)
    return ram, w


def run(binary, ram, poke, outdir):
    with open(binary, "rb") as f:
        elf = f.read()
    entry, phoff = struct.unpack_from("<II", elf, 0x18)
    phentsize, phnum = struct.unpack_from("<HH", elf, 0x2A)
    uc = Uc(UC_ARCH_ARM, UC_MODE_ARM)
    for i in range(phnum):
        p_type, off, vaddr, _, filesz, memsz, _, _ = struct.unpack_from("<8I", elf, phoff + i * phentsize)
        if p_type != 1:
            continue
        lo, hi = vaddr & ~0xFFF, (vaddr + memsz + 0xFFF) & ~0xFFF
        uc.mem_map(lo, hi - lo)
        uc.mem_write(vaddr, elf[off:off + filesz])
    uc.mem_map(td.STACK - 0x100000, 0x100000)
    uc.reg_write(A.UC_ARM_REG_SP, td.STACK - 0x100)

    fds, maps = {}, {}
    st = {"fd": 3, "map": td.MMAP_BASE, "exit": None, "ms": 1_000_000, "phys": {}, "next_event": 0}

    def cstr(addr):
        s = b""
        while (c := bytes(uc.mem_read(addr, 1))) != b"\0":
            s += c
            addr += 1
        return s.decode()

    def apply_events():
        t = st["ms"] - 1_000_000
        while st["next_event"] < len(EVENTS) and EVENTS[st["next_event"]][0] <= t:
            _, addr, word = EVENTS[st["next_event"]]
            poke(addr, word)
            for va, (pa, size) in st["phys"].items():
                if pa <= addr < pa + size:
                    uc.mem_write(va + addr - pa, struct.pack("<I", word))
            st["next_event"] += 1

    def svc(uc, intno, _):
        r = [uc.reg_read(getattr(A, f"UC_ARM_REG_R{i}")) for i in range(8)]
        n, ret = r[7], 0
        if n == 1:
            st["exit"] = r[0]
            uc.emu_stop()
            return
        if n == 5:
            path, flags = cstr(r[0]), r[1]
            if path == "/dev/mem":
                assert flags & 3 == 0, "monitor must open /dev/mem read-only"
                ret, fds[st["fd"]] = st["fd"], ("mem", None)
                st["fd"] += 1
            elif path.startswith(td.PREFIX):
                name = os.path.join(outdir, path[len(td.PREFIX):])
                if flags & 0o200 and os.path.exists(name):
                    ret = -17
                else:
                    ret, fds[st["fd"]] = st["fd"], ("file", open(name, "wb"))
                    st["fd"] += 1
            else:
                ret = -2
        elif n == 4:
            fds[r[0]][1].write(bytes(uc.mem_read(r[1], r[2])))
            ret = r[2]
        elif n == 6:
            _, f = fds.pop(r[0])
            if f:
                f.close()
        elif n in (118, 66, 174):  # fsync, setsid, rt_sigaction
            ret = 0
        elif n == 2:  # fork: carry on as the child
            ret = 0
        elif n == 192:
            length, fd, pgoff = r[1], r[4], r[5]
            assert fds[fd][0] == "mem" and r[2] == 1, "monitor must map /dev/mem read-only"
            va, size = st["map"], (length + 0xFFF) & ~0xFFF
            uc.mem_map(va, size, UC_PROT_READ)
            uc.mem_write(va, td.read_phys(ram, pgoff << 12, size))
            st["phys"][va] = (pgoff << 12, size)
            st["map"] += size + 0x1000
            maps[va] = size
            ret = va
        elif n == 91:
            uc.mem_unmap(r[0], maps.pop(r[0]))
        elif n == 263:  # clock_gettime(clock, ts)
            ms = st["ms"]
            uc.mem_write(r[1], struct.pack("<II", ms // 1000, (ms % 1000) * 1_000_000))
        elif n == 162:  # nanosleep(req, rem)
            sec, nsec = struct.unpack("<II", uc.mem_read(r[0], 8))
            st["ms"] += sec * 1000 + nsec // 1_000_000
            apply_events()
        else:
            raise RuntimeError(f"unexpected syscall {n}")
        uc.reg_write(A.UC_ARM_REG_R0, ret & 0xFFFFFFFF)

    def write_hook(uc, access, addr, size, value, _):
        if addr >= td.MMAP_BASE and addr < st["map"]:
            raise RuntimeError(f"write to mapped camera memory at {addr:#x}")

    uc.hook_add(UC_HOOK_INTR, svc)
    uc.hook_add(UC_HOOK_MEM_WRITE, write_hook)
    uc.emu_start(entry, 0xFFFFFFFF)
    return st["exit"]


def main():
    outdir = sys.argv[1]
    image_path = sys.argv[2] if len(sys.argv) > 2 else os.path.join(REPO, "work", "fwdc239b", "unpackedFW")
    binary = os.path.join(REPO, "work", "ramload", "SD_NDMON", "SCRIPT", "RAMLOAD", "NDMON.BIN")
    with open(image_path, "rb") as f:
        image = f.read()
    if image[:5] == b"RICOH":
        image = bytes(td.grfw.Firmware(image).decode())
    ram, poke = build_ram(image)
    os.makedirs(outdir, exist_ok=True)
    for name in ("NDMON.LCK", "NDMON.OK", "NDMON.ERR"):
        if os.path.exists(os.path.join(outdir, name)):
            os.remove(os.path.join(outdir, name))
    code = run(binary, ram, poke, outdir)
    print(f"exit {code}")
    for name in sorted(os.listdir(outdir)):
        print(f"  {os.path.getsize(os.path.join(outdir, name)):10d}  {name}")
    log = open(os.path.join(outdir, "NDMON.LOG")).read()
    timed = [l.split(" ", 1)[1] for l in log.splitlines()[1:] if re.match(r"\d{6}\.\d{3} ", l)]
    changes = [l for l in timed if not l.startswith(("all", "lvp 0x", "d "))]
    diffs = [l for l in timed if l.startswith("d ")]
    print("diff lines:", *diffs, sep="\n  ")
    # the ND object lies in BSS: its word shows once from the object area
    # and once from the .data/BSS pass
    ok_diffs = diffs == [f"d {LVP + 0x9F0C:#010x} 0x00000000 -> 0x00000100",
                         f"d {ND_OBJ + 0x38:#010x} 0x00000000 -> 0x00000001",
                         f"d {LVP + 0x9F0C:#010x} 0x00000100 -> 0x00000000",
                         f"d {ND_OBJ + 0x38:#010x} 0x00000000 -> 0x00000001",
                         f"d {AE + 0x5694:#010x} 0x00000000 -> 0x00000020"]
    notes = [l for l in log.splitlines()[1:] if not re.match(r"\d{6}\.\d{3} ", l)]
    print("notes:", *notes, sep="\n  ")
    ok_notes = any(l.startswith("scan done") for l in notes) and any(l.startswith("first snapshot") for l in notes)
    ok = code == 0 and os.path.exists(os.path.join(outdir, "NDMON.OK"))
    ok &= f"lvp {LVP:#010x}" in log
    ok &= changes == EXPECT and ok_notes and ok_diffs
    if changes != EXPECT:
        print("changes logged:", *changes, sep="\n  ")
    snaps = []
    with open(os.path.join(outdir, "SNAP.BIN"), "rb") as f:
        while head := f.read(16):
            magic, t, addr, n = struct.unpack("<4I", head)
            assert magic == 0x50414E53
            f.seek(n, 1)
            snaps.append((t, addr, n))
    print(f"{len(snaps)} snapshots, {sum(n for *_, n in snaps)} bytes; "
          f"data snapshots at {[t for t, a, n in snaps if a == MAP_START]}")
    print(log.splitlines()[0])
    print("\n".join(l[:150] for l in log.splitlines()[1:16] if re.match(r"\d{6}", l)))
    print("PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
