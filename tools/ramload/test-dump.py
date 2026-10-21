"""Run DUMP.BIN in Unicorn against a simulated camera memory, as a test
before the camera (docs/firmware/ram-loader.md).

    python3 tools/ramload/test-dump.py OUTDIR [IMAGE] [--poke ADDR=WORD ...]

The memory is built from IMAGE (default: stock work/fwdc239b/unpackedFW; a
FWDC239B.BIN is decoded) as the RTOS loads it: the image at 0x53000000, .data
copied to 0x55000000, BSS zero. --poke changes words in it first, to stand in
for run-time writes. The Linux syscalls the program makes are emulated;
/dev/mem reads that memory, files under /media/storage1/SCRIPT/RAMLOAD/ go to
OUTDIR.
"""
import argparse
import os
import struct
import sys

from unicorn import Uc, UC_ARCH_ARM, UC_MODE_ARM, UC_HOOK_INTR
from unicorn import arm_const as A

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(REPO, "fwpack"))
import grfw  # noqa: E402

BIAS = 0x52FEF790
RTOS, DATA_VMA, DATA_LMA, DATA_SIZE = 0x53000000, 0x55000000, 0x5407B540, 0x3A080
BSS_END = 0x55804374
PREFIX = "/media/storage1/SCRIPT/RAMLOAD/"
MMAP_BASE, STACK = 0x20000000, 0x7F000000


def physical(image):
    """{base: bytearray} of the simulated RAM"""
    text = bytearray(image[RTOS - BIAS:DATA_LMA + DATA_SIZE - BIAS])
    data = bytearray(BSS_END - DATA_VMA)
    data[:DATA_SIZE] = image[DATA_LMA - BIAS:DATA_LMA - BIAS + DATA_SIZE]
    return {RTOS: text, DATA_VMA: data}


def read_phys(ram, addr, n):
    for base, buf in ram.items():
        if base <= addr and addr + n <= base + len(buf) + 0x1000:
            o = addr - base
            chunk = bytes(buf[o:o + n])
            return chunk + bytes(n - len(chunk))
    raise ValueError(f"no simulated RAM at {addr:#x}")


def run(binary, ram, outdir):
    with open(binary, "rb") as f:
        elf = f.read()
    assert elf[:4] == b"\x7fELF"
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
    uc.mem_map(STACK - 0x100000, 0x100000)
    uc.reg_write(A.UC_ARM_REG_SP, STACK - 0x100)

    files, fds, maps = {}, {}, {}
    state = {"next_fd": 3, "next_map": MMAP_BASE, "exit": None}

    def cstr(addr):
        s = b""
        while True:
            c = uc.mem_read(addr, 1)
            if c == b"\0":
                return s.decode()
            s += bytes(c)
            addr += 1

    def svc(uc, intno, _):
        r = [uc.reg_read(getattr(A, f"UC_ARM_REG_R{i}")) for i in range(8)]
        n, ret = r[7], 0
        if n == 1:  # exit
            state["exit"] = r[0]
            uc.emu_stop()
            return
        if n == 5:  # open
            path, flags = cstr(r[0]), r[1]
            if path == "/dev/mem":
                ret = state["next_fd"]
                fds[ret] = ("mem", None)
                state["next_fd"] += 1
            elif path.startswith(PREFIX):
                name = os.path.join(outdir, path[len(PREFIX):])
                if flags & 0o200 and os.path.exists(name):
                    ret = -17  # EEXIST
                else:
                    ret = state["next_fd"]
                    state["next_fd"] += 1
                    fds[ret] = ("file", open(name, "wb"))
            else:
                ret = -2
        elif n == 4:  # write
            kind, f = fds[r[0]]
            f.write(bytes(uc.mem_read(r[1], r[2])))
            ret = r[2]
        elif n == 6:  # close
            kind, f = fds.pop(r[0])
            if f:
                f.close()
        elif n == 118:  # fsync
            ret = 0
        elif n == 192:  # mmap2(addr, len, prot, flags, fd, pgoff)
            length, fd, pgoff = r[1], r[4], r[5]
            assert fds[fd][0] == "mem" and r[2] == 1, "dump must map /dev/mem read-only"
            va = state["next_map"]
            size = (length + 0xFFF) & ~0xFFF
            uc.mem_map(va, size)
            uc.mem_write(va, read_phys(ram, pgoff << 12, size))
            state["next_map"] += size + 0x1000
            maps[va] = size
            ret = va
        elif n == 91:  # munmap
            uc.mem_unmap(r[0], maps.pop(r[0]))
        else:
            raise RuntimeError(f"unexpected syscall {n}")
        uc.reg_write(A.UC_ARM_REG_R0, ret & 0xFFFFFFFF)

    uc.hook_add(UC_HOOK_INTR, svc)
    uc.emu_start(entry, 0xFFFFFFFF)
    return state["exit"]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("outdir")
    ap.add_argument("image", nargs="?", default=os.path.join(REPO, "work", "fwdc239b", "unpackedFW"))
    ap.add_argument("--binary", default=os.path.join(REPO, "work", "ramload", "SD_ROOT", "SCRIPT",
                                                      "RAMLOAD", "DUMP.BIN"))
    ap.add_argument("--poke", action="append", default=[], metavar="ADDR=WORD")
    a = ap.parse_args()
    with open(a.image, "rb") as f:
        image = f.read()
    if image[:5] == b"RICOH":
        image = bytes(grfw.Firmware(image).decode())
    ram = physical(image)
    for p in a.poke:
        addr, word = (int(x, 0) for x in p.split("="))
        for base, buf in ram.items():
            if base <= addr < base + len(buf):
                struct.pack_into("<I", buf, addr - base, word)
    os.makedirs(a.outdir, exist_ok=True)
    for name in ("DUMP.LCK", "DUMP.OK", "DUMP.ERR"):
        if os.path.exists(os.path.join(a.outdir, name)):
            os.remove(os.path.join(a.outdir, name))
    code = run(a.binary, ram, a.outdir)
    print(f"exit {code}")
    for name in sorted(os.listdir(a.outdir)):
        print(f"  {os.path.getsize(os.path.join(a.outdir, name)):9d}  {name}")
    for name in ("INFO.TXT", "DUMP.OK", "DUMP.ERR"):
        path = os.path.join(a.outdir, name)
        if os.path.exists(path):
            print(f"--- {name}\n{open(path).read()}", end="")
    sys.exit(0 if code == 0 and os.path.exists(os.path.join(a.outdir, "DUMP.OK")) else 1)


if __name__ == "__main__":
    main()
