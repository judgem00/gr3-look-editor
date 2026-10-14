"""Run the camera's image-tuning code in Unicorn.

Adapted from idontknownihil/gr3-film-mods (emu.py, run_tuning.py; MIT,
Copyright (c) idontknownihil). Changes: reads this repo's unpacked image
(RTOS = file offset + BIAS), live view Make, watch hooks.
"""
import os
import struct

from unicorn import (Uc, UcError, UC_ARCH_ARM, UC_MODE_ARM, UC_HOOK_CODE,
                     UC_HOOK_INTR)
from unicorn import arm_const as A

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMAGE = os.path.join(REPO, "work", "fwdc239b", "unpackedFW")

BIAS = 0x52FEF790
BASE = 0x53000000
DATA_VMA, DATA_LMA, DATA_SIZE = 0x55000000, 0x5407B540, 0x3A080
HEAP, STACK, STOP = 0x60000000, 0x5F000000, 0x5E000000

MALLOC = (0x536AF2A8, 0x536AF31C)
FREE = (0x536AF2E4, 0x536AF358)
ASSERT = 0x536B93D0
RET0 = {0x536B82D4, 0x5313FBB8}

# ImageTuning object setup and the two Make entry points
VTABLE = 0x53DBD8A0
INIT = (0x536DA1FC, 0x536DA62C)
MAKE_STILL = 0x536DD8F0
MAKE_LIVE = 0x536DDECC
# calls inside Make that touch hardware or OS state; they return 0
MAKE_STUBS = (0x536DECB4, 0x536E4634, 0x536E4DE4, 0x536E5070, 0x536E7348,
              0x536EAF80, 0x536EC3E0, 0x536EB0B0, 0x536E9D1C,
              0x536E9BD8, 0x536E91A8)   # live view: sub-tuning at obj+0x180 and its copier

INFO_SIZE, SI_SIZE = 0x9000, 0x80000
STAGE_OFF = 0x71248          # shooting-info block of the still stage
# image-control settings at their neutral menu value (4 = 0 on the -4..+4 scale)
DEFAULTS = {**{o: 4 for o in (0x77D, 0x77E, 0x77F, 0x780, 0x781, 0x782, 0x784,
                              0x787, 0x792, 0x793, 0x794)},
            0x783: 1, 0x786: 1, 0x798: 1}


def load_rtos(path=IMAGE):
    with open(path, "rb") as f:
        img = f.read()
    # the RTOS image ends before .data's run-time address
    return img[BASE - BIAS:DATA_VMA - BIAS]


class Emu:
    def __init__(self, rtos):
        u = self.u = Uc(UC_ARCH_ARM, UC_MODE_ARM)
        u.mem_map(BASE, (len(rtos) + 0xFFFF) & ~0xFFFF)
        u.mem_write(BASE, bytes(rtos))
        u.mem_map(DATA_VMA, 0x01000000)
        src = DATA_LMA - BASE
        u.mem_write(DATA_VMA, bytes(rtos[src:src + DATA_SIZE]))
        u.mem_map(HEAP, 0x04000000)
        u.mem_map(STACK - 0x100000, 0x100000)
        u.mem_map(STOP, 0x1000)
        u.reg_write(A.UC_ARM_REG_C1_C0_2, 0xF00000)
        u.reg_write(A.UC_ARM_REG_FPEXC, 0x40000000)
        self.brk = HEAP
        self.asserts = []
        self.svcs = []
        self.stubs = {}
        self.watch = {}          # pc -> callback(emu), runs before the instruction
        u.hook_add(UC_HOOK_CODE, self._code)
        u.hook_add(UC_HOOK_INTR, self._intr)

    def alloc(self, n):
        a = self.brk
        self.brk = (self.brk + n + 0x3F) & ~0x3F
        self.u.mem_write(a, bytes(n))
        return a

    def _ret(self, v=None):
        if v is not None:
            self.u.reg_write(A.UC_ARM_REG_R0, v)
        self.u.reg_write(A.UC_ARM_REG_PC, self.u.reg_read(A.UC_ARM_REG_LR))

    def _code(self, u, at, size, _):
        if at in self.watch:
            self.watch[at](self)
        if at in MALLOC:
            self._ret(self.alloc(u.reg_read(A.UC_ARM_REG_R0)))
        elif at in FREE or at in RET0:
            self._ret(0)
        elif at == ASSERT:
            if not u.reg_read(A.UC_ARM_REG_R0):
                self.asserts.append((u.reg_read(A.UC_ARM_REG_LR), u.reg_read(A.UC_ARM_REG_R3)))
            self._ret(0)
        elif at in self.stubs:
            v = self.stubs[at]
            self._ret(v(self) if callable(v) else v)

    def _intr(self, u, intno, _):
        self.svcs.append(intno)
        u.reg_write(A.UC_ARM_REG_R0, 0)

    def call(self, at, *args, limit=20_000_000):
        u = self.u
        for i in range(13):
            u.reg_write(getattr(A, f"UC_ARM_REG_R{i}"), 0)
        for i, v in enumerate(args):
            u.reg_write(getattr(A, f"UC_ARM_REG_R{i}"), v)
        u.reg_write(A.UC_ARM_REG_SP, STACK - 0x100)
        u.reg_write(A.UC_ARM_REG_LR, STOP)
        u.reg_write(A.UC_ARM_REG_CPSR, 0x13)
        try:
            u.emu_start(at, STOP, count=limit)
        except UcError as e:
            pc = u.reg_read(A.UC_ARM_REG_PC)
            raise RuntimeError(f"{e} at pc={pc:#x} lr={u.reg_read(A.UC_ARM_REG_LR):#x}")
        if u.reg_read(A.UC_ARM_REG_PC) != STOP:
            raise RuntimeError(f"did not return, pc={u.reg_read(A.UC_ARM_REG_PC):#x}")
        return u.reg_read(A.UC_ARM_REG_R0)

    def reg(self, n):
        return self.u.reg_read(getattr(A, f"UC_ARM_REG_{n.upper()}"))

    def read(self, a, n):
        return bytes(self.u.mem_read(a, n))

    def r32(self, a):
        return struct.unpack("<I", self.u.mem_read(a, 4))[0]

    def w32(self, a, v):
        self.u.mem_write(a, struct.pack("<I", v))

    def w8(self, a, v):
        self.u.mem_write(a, bytes([v]))


class Tuning:
    """one ImageTuning object with its info and shooting-info blocks"""

    def __init__(self, rtos):
        e = self.e = Emu(rtos)
        for f in MAKE_STUBS:
            e.stubs[f] = 0
        self.obj = e.alloc(0x400)
        cfg = e.alloc(0x1000)
        e.w32(self.obj, VTABLE)
        for f in INIT:
            e.call(f, self.obj, cfg)
        e.w32(self.obj, VTABLE)
        self.info = e.alloc(INFO_SIZE)
        self.si = e.alloc(SI_SIZE)
        e.w32(self.info + 0x82D0, self.si)

    def make(self, look, live=False, settings=None):
        """run Make for `look` (shooting-info tone index); returns the info block"""
        e = self.e
        stage = 0 if live else 1
        e.w8(self.info + 0x82AC, stage)
        st = self.si + (STAGE_OFF if stage else 0)
        for off, v in {**DEFAULTS, **(settings or {})}.items():
            e.w8(st + off, v)
        e.w8(st + 0x77C, look)
        e.call(MAKE_LIVE if live else MAKE_STILL, self.obj, self.info)
        return e.read(self.info, INFO_SIZE)
