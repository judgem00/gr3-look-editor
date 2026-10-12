"""Read and patch the colour data of the image controls in an unpacked GR III
firmware image (work/<fw>/unpackedFW, decoded from the update file).

How a look reaches its data is documented in docs/look-mapping.md; the short
version:

    look -> internal index -> 0x55018d94[index] -> ColorMatrix set
         set+0x00 sensor CCM (shared)   set+0x04 look matrix   set+0x10 luma

Two address mappings are in play. RTOS code and read-only data sit at
RAM = file offset + BIAS. The startup code copies .data from the image to
0x55000000, so .data addresses have to be moved back to where they are stored.
"""
import struct

BIAS = 0x52FEF790
DATA_VMA, DATA_LMA, DATA_SIZE = 0x55000000, 0x5407b540, 0x3a080
CM_TABLE = 0x55018d94        # one ColorMatrix set per internal index
# the three per-look tables the setting filler (0x536dc0f4) indexes with the
# look: parameter set (setting+8), +0xc block, tone set (setting+0x18)
LOOK_TABLES = (0x55018d94, 0x55018cf0, 0x55018e4c)
SCALE = 8192                 # int16 fixed point, 8192 = 1.0
TONE_TABLE = 0x55018e4c      # per look -> YCbCr set; its [0] -> 9 hue matrices
YCC_BASE = 0x53dc1444        # RGB -> YCbCr, int32 /256 (77 150 29 / -43 -85 128 / 128 -107 -21)

# (look index, table index, name). The look index is what the camera stores
# at setting+0x77c; 14 and 20 are confirmed by assert comparisons in the UI
# code, 13/14/16 by the monochrome switch in ColorMatrixTuning, the rest follow
# the order of the name pool. The table index selects the parameter set: the
# setting filler (0x536dc0f4) maps NegativeFilm to 25 and sends Bleach Bypass
# and HDR to other sets depending on a look option (+0x786 / +0x788), so those
# variants have matrices of their own. Which menu option is which is open.
LOOKS = [
    (11, 11, "Standard"), (12, 12, "Vivid"), (13, 13, "MonotoneGR"),
    (14, 14, "SoftMonotone"), (15, 15, "HardMonotone"), (16, 16, "HighContrastBW"),
    (17, 17, "PositiveFilm"), (18, 18, "BleechBypass_GR"),
    (18, 22, "BleechBypass_GR · Var. 2"), (18, 23, "BleechBypass_GR · Var. 3"),
    (19, 19, "Retro"), (20, 20, "HdrCuisine"), (20, 24, "HdrCuisine · Var."),
    (21, 21, "CrossProcess"), (22, 25, "NegativeFilm"),
]
# looks the tuning code renders through an equal-weight mix (0x536dc4c8)
MONO_MIX = {13, 14, 16}

# Pentax makernote "ImageTone" as exiftool prints it -> firmware name
IMAGE_TONE_NAMES = {
    "standard": "Standard", "vivid": "Vivid", "monotone": "MonotoneGR",
    "soft monotone": "SoftMonotone", "hard monotone": "HardMonotone",
    "hi-contrast b&w": "HighContrastBW", "high contrast b&w": "HighContrastBW",
    "positive film": "PositiveFilm", "bleach bypass": "BleechBypass_GR",
    "bleach bypass 2": "BleechBypass_GR", "retro": "Retro",
    "hdr tone": "HdrCuisine", "cross processing": "CrossProcess",
    "cross processing 2": "CrossProcess", "negative film": "NegativeFilm",
}


def file_offset(va):
    if DATA_VMA <= va < DATA_VMA + DATA_SIZE:
        return va - DATA_VMA + DATA_LMA - BIAS
    return va - BIAS


def inherit(data, target_table, source_table):
    """image bytes where look table entry `target_table` points at the same
    look data as `source_table` -- all three per-look tables, in .data"""
    img = bytearray(data)
    for t in LOOK_TABLES:
        src = struct.unpack_from("<I", img, file_offset(t + 4 * source_table))[0]
        struct.pack_into("<I", img, file_offset(t + 4 * target_table), src)
    return bytes(img)


class Firmware:
    def __init__(self, path, data=None):
        self.path = path
        if data is None:
            with open(path, "rb") as f:
                data = f.read()
        self.data = data
        if self.data[:8] != b"BOOTPARA":
            raise ValueError(f"{path} does not look like an unpacked GR III image")

    def u32(self, va):
        return struct.unpack_from("<I", self.data, file_offset(va))[0]

    def matrix(self, va):
        return [v / SCALE for v in struct.unpack_from("<9h", self.data, file_offset(va))]

    def cm_set(self, table):
        return self.u32(CM_TABLE + 4 * table)

    def table_of(self, name):
        return next(t for _, t, n in LOOKS if n == name)

    def with_inheritance(self, mapping):
        """a Firmware whose looks in {target: source} use the source's look data"""
        data = self.data
        for target, source in mapping.items():
            data = inherit(data, self.table_of(target), self.table_of(source))
        return Firmware(self.path, data)

    def look_matrix_address(self, table):
        return self.u32(self.cm_set(table) + 4)

    def sensor_ccm(self):
        # the same for every look; read it from Standard's set
        return self.matrix(self.u32(self.cm_set(11)))

    def luma_weights(self):
        w = struct.unpack_from("<3i", self.data, file_offset(self.u32(self.cm_set(11) + 0x10)))
        return [v / 256 for v in w]

    def sat_matrices(self, table):
        """the nine matrices of the saturation setting -4..+4 (set+8, index
        0x53dc1400[setting]); ColorMatrixTuning::Calc (0x536deecc) computes
        A[sat] x look x sensor. A[4] is identity."""
        t = self.u32(self.cm_set(table) + 8)
        return [self.matrix(self.u32(t + 4 * k)) for k in range(9)]

    def hue_matrices(self, table):
        """the nine matrices of the hue setting -4..+4: a rotation in the Cb/Cr
        plane, applied as H x YCC_BASE by YCbCrMatrixTuning (0x536e4720), i.e.
        after the tone curve"""
        t = self.u32(self.u32(TONE_TABLE + 4 * table))
        return [[v / SCALE for v in struct.unpack_from("<9i", self.data, file_offset(self.u32(t + 4 * k)))]
                for k in range(9)]

    def ycc_base(self):
        return [v / 256 for v in struct.unpack_from("<9i", self.data, file_offset(YCC_BASE))]

    def looks(self):
        owners = {}
        for _, table, name in LOOKS:
            owners.setdefault(self.look_matrix_address(table), []).append(name)
        out = []
        for index, table, name in LOOKS:
            va = self.look_matrix_address(table)
            out.append({
                "name": name, "index": index, "table": table, "address": va,
                "file_offset": file_offset(va), "matrix": self.matrix(va),
                "mono_mix": index in MONO_MIX,
                "sat_matrices": self.sat_matrices(table),
                "hue_matrices": self.hue_matrices(table),
                "shared_with": [n for n in owners[va] if n != name],
            })
        return out

    def patched(self, matrices):
        """bytes of the image with {look name: 9 floats} written in.

        Raises ValueError for unknown looks, out-of-range values, or two
        edits that land on one shared matrix with different values."""
        by_name = {name: table for _, table, name in LOOKS}
        writes = {}
        for name, values in matrices.items():
            if name not in by_name:
                raise ValueError(f"unknown look {name!r}")
            if len(values) != 9:
                raise ValueError(f"{name}: a matrix needs nine values")
            fixed = [round(v * SCALE) for v in values]
            if not all(-32768 <= v <= 32767 for v in fixed):
                raise ValueError(f"{name}: values must stay within about -4.0 .. +4.0")
            va = self.look_matrix_address(by_name[name])
            if va in writes and writes[va][1] != fixed:
                raise ValueError(f"{name} and {writes[va][0]} share one matrix "
                                 "but were given different values")
            writes[va] = (name, fixed)
        data = bytearray(self.data)
        for va, (_, fixed) in writes.items():
            struct.pack_into("<9h", data, file_offset(va), *fixed)
        return bytes(data)
