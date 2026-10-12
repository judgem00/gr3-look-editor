"""The camera's tone curve, computed by the camera's own code.

GammaTuning (RTOS 0x536e7b64) builds the 256-entry, 14-bit tone curve from
four image-control settings -- key, contrast, contrast highlight, contrast
shadow -- and a gain, by blending 15 base curves stored in .data:

    0x550204d4[family] -> 5 curve pointers, one per white-level band
    family 0 = high contrast, 1 = normal, 2 = low contrast

Rather than re-implement that function (it blends by region, white level and a
smoothing table), this runs the original ARM code in Unicorn on the firmware
image. Edited base curves go through exactly the arithmetic the camera uses.

    curve(image, key=0, contrast=3, highlight=-4, shadow=-1, gain=0x2000)

Settings are the camera's -4..+4 values. The curve maps a 12-bit linear
input (index i covers input i*16) to 0..0x4000.
"""
import struct

import unicorn
from unicorn import arm_const as A

BIAS = 0x52FEF790
DATA_VMA, DATA_LMA, DATA_SIZE = 0x55000000, 0x5407b540, 0x3a080
GAMMA_FN = 0x536e7b64
FAMILIES = 0x550204d4                 # 3 pointers to 5-curve sets
CURVE_LEN = 256
CODE_START, CODE_END = 0x53000000, 0x540c6000
STACK, STACK_SIZE = 0x60000000, 0x10000
SCRATCH = 0x61000000
STOP = 0x62000000


def file_offset(va):
    if DATA_VMA <= va < DATA_VMA + DATA_SIZE:
        return va - DATA_VMA + DATA_LMA - BIAS
    return va - BIAS


def base_curves(image):
    """{(family, band): (address, [256 values])} for the 15 base curves"""
    out = {}
    for fam in range(3):
        cset = struct.unpack_from("<I", image, file_offset(FAMILIES + 4 * fam))[0]
        for band in range(5):
            va = struct.unpack_from("<I", image, file_offset(cset + 4 * band))[0]
            vals = struct.unpack_from(f"<{CURVE_LEN}H", image, file_offset(va))
            out[(fam, band)] = (va, list(vals))
    return out


def with_curves(image, curves):
    """image bytes with {(family, band): [256 values]} written in"""
    img = bytearray(image)
    addrs = base_curves(image)
    for key, vals in curves.items():
        va = addrs[key][0]
        vals = [min(0xFFFF, max(0, int(round(v)))) for v in vals]
        struct.pack_into(f"<{CURVE_LEN}H", img, file_offset(va), *vals)
    return bytes(img)


class GammaEngine:
    """runs GammaTuning's curve builder on a given firmware image"""

    def __init__(self, image):
        self.base = base_curves(image)
        self.load(image)

    def set_curves(self, curves):
        """replace base curves in the emulator's memory; {} restores the originals"""
        for key, (va, vals) in self.base.items():
            new = curves.get(key, vals)
            new = [min(0xFFFF, max(0, int(v))) for v in new]
            self.mu.mem_write(va, struct.pack(f"<{CURVE_LEN}H", *new))

    def load(self, image):
        mu = unicorn.Uc(unicorn.UC_ARCH_ARM, unicorn.UC_MODE_ARM)
        size = CODE_END - CODE_START
        mu.mem_map(CODE_START, size)
        mu.mem_write(CODE_START, bytes(image[CODE_START - BIAS:CODE_END - BIAS]))
        mu.mem_map(DATA_VMA, 0x40000)
        mu.mem_write(DATA_VMA, bytes(image[file_offset(DATA_VMA):file_offset(DATA_VMA) + DATA_SIZE]))
        mu.mem_map(STACK, STACK_SIZE)
        mu.mem_map(SCRATCH, 0x1000)
        mu.mem_map(STOP, 0x1000)
        # enable VFP in case a helper on the path touches it
        mu.reg_write(A.UC_ARM_REG_C1_C0_2, mu.reg_read(A.UC_ARM_REG_C1_C0_2) | (0xF << 20))
        mu.reg_write(A.UC_ARM_REG_FPEXC, 0x40000000)
        self.mu = mu

    def curve(self, key=0, contrast=0, highlight=0, shadow=0, gain=0x2000, flag=0):
        mu = self.mu
        params = struct.pack("<iiiiiBxxxi", 0, key, contrast, -shadow, highlight, flag, gain)
        out = SCRATCH
        par = SCRATCH + 0x800
        mu.mem_write(par, params)
        mu.mem_write(out, b"\0" * (2 * CURVE_LEN))
        mu.reg_write(A.UC_ARM_REG_R0, out)
        mu.reg_write(A.UC_ARM_REG_R1, out)
        mu.reg_write(A.UC_ARM_REG_R2, par)
        mu.reg_write(A.UC_ARM_REG_SP, STACK + STACK_SIZE - 0x100)
        mu.reg_write(A.UC_ARM_REG_LR, STOP)
        mu.emu_start(GAMMA_FN, STOP, count=5_000_000)
        return list(struct.unpack(f"<{CURVE_LEN}H", mu.mem_read(out, 2 * CURVE_LEN)))
