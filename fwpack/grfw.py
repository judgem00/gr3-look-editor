"""The GR III firmware container (FWDC*.BIN): parse, decode, verify, rebuild.

Layout, as checked by FwDecoder in the camera (RTOS 0x5362bd40 ff.):

    0x00   header, 0x80 bytes
             +0x30  project number (0x24c on the GR III)
             +0x78  project number twice (0x024c024c)
             +0x7c  0xa55a5aa5
    0x80   compressed stream: frames, then a 0x0000 end marker
           padding to a multiple of 4
    end-0x18 footer
             +0x00  project number twice   +0x04  0xa55a5aa5
             +0x08  firmware type          +0x0c  stream length incl. end marker
             +0x10  decoded length         +0x14  checksum

Two checksums, both "sum of little-endian 32-bit words == 0":
  - over the whole file, balanced by footer +0x14 (0x5362bcc8)
  - over the decoded image, balanced by its last word (0x5362bea8)
The camera skips the first if the field is 0xffffffff and the second if the
last word is 0, 0xfffffffe or 0xffffffff.

Frame format (documented by gr_unpack): a big-endian u16, top bit set for a
literal frame, low 15 bits the frame's size in bytes. A compressed frame is a
sequence of blocks: a big-endian u16 of flags, then 16 tokens, MSB first --
0 a literal byte, 1 a copy:

    ooooolll oooooooo [llllllll ...]
    offset = ((b0 & 0xf8) << 5) | b1, length = b0 & 7 (+ extension bytes)
    copies length + 3 bytes from offset bytes back in the output;
    offset 0 ends the frame.
"""
import struct

import numpy as np

MAGIC = 0xA55A5AA5
HEADER_SIZE = 0x80
FOOTER_SIZE = 0x18
LITERAL_MAX = 0x7FFF
# every frame of the original decodes to 0x6000 bytes; new literal frames
# keep to that grid so the camera's decoder sees nothing unusual
FRAME_OUTPUT = 0x6000


class FormatError(Exception):
    pass


def word_sum(data):
    """sum of little-endian u32 words, mod 2^32; len(data) must be a multiple of 4"""
    if len(data) % 4:
        raise FormatError("length is not a multiple of 4")
    return int(np.frombuffer(data, dtype="<u4").sum(dtype=np.uint64)) & 0xFFFFFFFF


class Frame:
    __slots__ = ("pos", "size", "literal", "out_start", "out_end", "src_min")

    def __init__(self, pos, size, literal):
        self.pos, self.size, self.literal = pos, size, literal
        self.out_start = self.out_end = 0
        self.src_min = None     # lowest output position any copy reads from

    def raw(self, data):
        return data[self.pos:self.pos + 2 + self.size]


class Firmware:
    def __init__(self, data):
        self.data = bytes(data)
        if len(self.data) < HEADER_SIZE + FOOTER_SIZE or self.data[:5] != b"RICOH":
            raise FormatError("not a RICOH firmware file")
        self.header = self.data[:HEADER_SIZE]
        self.footer = struct.unpack_from("<6I", self.data, len(self.data) - FOOTER_SIZE)
        self.project = struct.unpack_from("<I", self.header, 0x30)[0]
        self.fw_type = self.footer[2]
        self.stream_len = self.footer[3]
        self.decoded_len = self.footer[4]
        self.frames = self._parse_frames()

    @classmethod
    def load(cls, path):
        with open(path, "rb") as f:
            return cls(f.read())

    def _parse_frames(self):
        frames, p = [], HEADER_SIZE
        end = HEADER_SIZE + self.stream_len - 2
        while True:
            prefix = int.from_bytes(self.data[p:p + 2], "big")
            if prefix == 0:
                break
            fr = Frame(p, prefix & 0x7FFF, bool(prefix & 0x8000))
            frames.append(fr)
            p += 2 + fr.size
            if p > end:
                raise FormatError(f"frame at {fr.pos:#x} runs past the stream")
        if p != end:
            raise FormatError(f"end marker at {p:#x}, footer says {end:#x}")
        return frames

    # ---- checks, in the order FwDecoder makes them --------------------------

    def check_container(self, expected_project=None, expected_type=None):
        """problems with header, footer and file checksum (empty list = ok)"""
        problems = []
        h, f = self.header, self.footer
        proj = expected_project if expected_project is not None else self.project
        if self.project != proj:
            problems.append(f"header project number {self.project:#x} != {proj:#x}")
        twice = proj | (proj << 16)
        if struct.unpack_from("<I", h, 0x78)[0] != twice:
            problems.append("header +0x78 is not the project number twice")
        if struct.unpack_from("<I", h, 0x7C)[0] != MAGIC:
            problems.append("header magic missing")
        if f[0] != twice:
            problems.append("footer project number mismatch")
        if f[1] != MAGIC:
            problems.append("footer magic missing")
        if expected_type is not None and f[2] != expected_type:
            problems.append(f"footer firmware type {f[2]} != {expected_type}")
        if f[5] != 0xFFFFFFFF and word_sum(self.data) != 0:
            problems.append(f"file checksum wrong (word sum {word_sum(self.data):#010x})")
        return problems

    # ---- decoding ----------------------------------------------------------

    def decode(self):
        """the decoded image; also fills in each frame's output range and
        the lowest position its copies read from"""
        d = self.data
        out = bytearray()
        for fr in self.frames:
            fr.out_start = len(out)
            p, end = fr.pos + 2, fr.pos + 2 + fr.size
            if fr.literal:
                out += d[p:end]
            else:
                src_min = None
                done = False
                while p < end and not done:
                    flags = (d[p] << 8) | d[p + 1]
                    p += 2
                    for bit in range(15, -1, -1):
                        if p >= end:
                            break
                        if not (flags >> bit) & 1:
                            out.append(d[p])
                            p += 1
                            continue
                        b0, b1 = d[p], d[p + 1]
                        p += 2
                        length = b0 & 7
                        offset = ((b0 & 0xF8) << 5) | b1
                        if length == 7:
                            length += d[p]
                            p += 1
                            if length == 7 + 0xFF:
                                nxt = 0xFF
                                while nxt == 0xFF:
                                    nxt = d[p]
                                    p += 1
                                    length += nxt
                        if offset == 0:
                            done = True
                            break
                        start = len(out) - offset
                        if start < 0:
                            raise FormatError(f"copy before start of output in frame at {fr.pos:#x}")
                        n = length + 3
                        if src_min is None or start < src_min:
                            src_min = start
                        if offset >= n:
                            out += out[start:start + n]
                        else:            # overlapping copy repeats the pattern
                            for k in range(n):
                                out.append(out[start + k])
                if p != end:
                    raise FormatError(f"frame at {fr.pos:#x} decoded {p - fr.pos - 2} of {fr.size} bytes")
                fr.src_min = src_min
            fr.out_end = len(out)
        return bytes(out)


def fix_image_checksum(image):
    """the image with its last word set so its word sum is zero"""
    img = bytearray(image)
    if len(img) % 4:
        raise FormatError("image length is not a multiple of 4")
    img[-4:] = b"\0\0\0\0"
    struct.pack_into("<I", img, len(img) - 4, (-word_sum(bytes(img))) & 0xFFFFFFFF)
    return bytes(img)


def literal_frames(chunk):
    out = bytearray()
    step = len(chunk) if len(chunk) <= LITERAL_MAX else FRAME_OUTPUT
    for i in range(0, len(chunk), step):
        part = chunk[i:i + step]
        out += (0x8000 | len(part)).to_bytes(2, "big") + part
    return bytes(out)


def repack(original, image):
    """a firmware file for `image`, built from `original` with as little change
    as possible: frames are copied byte for byte unless their output differs,
    or they copy from bytes that differ -- those are re-emitted as literal
    frames. Returns (bytes, list of re-encoded frame indices)."""
    if len(image) != original.decoded_len:
        raise FormatError(f"image is {len(image)} bytes, the container holds "
                          f"{original.decoded_len}; size changes are not supported")
    base = original.decode()
    if len(base) != original.decoded_len:
        raise FormatError("original does not decode to the length its footer states")

    a = np.frombuffer(base, dtype=np.uint8)
    b = np.frombuffer(image, dtype=np.uint8)
    changed = np.flatnonzero(a != b)
    # prefix count of changed bytes, to test any output range in O(1)
    cum = np.zeros(len(a) + 1, dtype=np.int64)
    if len(changed):
        np.add.at(cum, changed + 1, 1)
        cum = np.cumsum(cum)

    def dirty(lo, hi):
        return len(changed) and cum[hi] - cum[lo] > 0

    redo = []
    for i, fr in enumerate(original.frames):
        if dirty(fr.out_start, fr.out_end):
            redo.append(i)
        elif fr.src_min is not None and dirty(fr.src_min, fr.out_end):
            # conservative: everything between its lowest source and its end
            redo.append(i)

    stream = bytearray()
    redo_set = set(redo)
    for i, fr in enumerate(original.frames):
        if i in redo_set:
            stream += literal_frames(image[fr.out_start:fr.out_end])
        else:
            stream += fr.raw(original.data)
    stream += b"\0\0"
    stream_len = len(stream)
    stream += b"\0" * (-(HEADER_SIZE + len(stream)) % 4)

    proj2, magic, fw_type = original.footer[:3]
    body = original.header + bytes(stream) + struct.pack("<5I", proj2, magic, fw_type,
                                                         stream_len, len(image))
    checksum = (-word_sum(body + b"\0\0\0\0")) & 0xFFFFFFFF
    return body + struct.pack("<I", checksum), redo
