"""The RAM view of a firmware file, as rampatch.rs reads it: 0x53000000 up
to the end of the .data copy source (the same bytes as RTOS.BIN of a dump,
without the rounding). For test-load.sh.

    python3 tools/ramload/view.py FIRMWARE OUT
"""
import os
import sys

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(REPO, "fwpack"))
import grfw  # noqa: E402

BIAS = 0x52FEF790
RTOS, VIEW_END = 0x53000000, 0x5407B540 + 0x3A080

with open(sys.argv[1], "rb") as f:
    image = f.read()
if image[:5] == b"RICOH":
    image = bytes(grfw.Firmware(image).decode())
with open(sys.argv[2], "wb") as f:
    f.write(image[RTOS - BIAS:VIEW_END - BIAS])
