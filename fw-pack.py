#!/usr/bin/env python3
"""Check and rebuild GR III firmware update files (FWDC*.BIN).

    ./fw-pack.py check fwdc239b.bin
    ./fw-pack.py pack fwdc239b.bin work/fwdc239b/unpackedFW-looks \\
        -o work/fwdc239b/FWDC239B.BIN

`check` runs every test the camera's FwDecoder makes (header, footer, file
checksum, decoded length, image checksum) and reports what it finds.

`pack` builds an update file for a modified unpacked image. It starts from
the original file and changes as little as possible: frames are copied byte for
byte unless their output differs, or they copy from bytes that differ; those
are re-emitted as literal frames. The image's last word is set so its checksum
holds, the footer and file checksum are recomputed, and the result is decoded
again and compared before anything is written. Packing an unmodified image
reproduces the original file exactly.

Format and checks are documented in fwpack/grfw.py and
docs/firmware-packing.md. A file that passes here passes the container checks
the camera makes; whether the camera then boots the modified image is a
different question.
"""
import argparse, os, sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "fwpack"))
import grfw  # noqa: E402

GR3_PROJECT, USER_FW_TYPE = 0x24C, 2


def check(path):
    fw = grfw.Firmware.load(path)
    print(f"{path}: {len(fw.data)} bytes, {len(fw.frames)} frames "
          f"({sum(f.literal for f in fw.frames)} literal)")
    print(f"  project {fw.project:#x}, type {fw.fw_type}, "
          f"build {fw.header[0x58:0x70].split(b'\0')[0].decode(errors='replace')}")
    problems = fw.check_container(GR3_PROJECT, USER_FW_TYPE)
    image = fw.decode()
    if len(image) != fw.decoded_len:
        problems.append(f"decodes to {len(image)} bytes, footer says {fw.decoded_len}")
    last = int.from_bytes(image[-4:], "little")
    if last not in (0, 0xFFFFFFFE, 0xFFFFFFFF) and grfw.word_sum(image) != 0:
        problems.append(f"image checksum wrong (word sum {grfw.word_sum(image):#010x})")
    for p in problems:
        print(f"  FAIL {p}")
    if not problems:
        print("  ok: header, footer, file checksum, decoded length, image checksum")
    return 1 if problems else 0


def pack(original_path, image_path, out_path):
    original = grfw.Firmware.load(original_path)
    if problems := original.check_container(GR3_PROJECT, USER_FW_TYPE):
        sys.exit(f"the original does not pass its own checks: {problems}")
    with open(image_path, "rb") as f:
        image = grfw.fix_image_checksum(f.read())
    data, redo = grfw.repack(original, image)

    rebuilt = grfw.Firmware(data)
    if rebuilt.check_container(GR3_PROJECT, USER_FW_TYPE) or rebuilt.decode() != image:
        sys.exit("internal error: the rebuilt file does not verify; nothing written")
    if os.path.abspath(out_path) == os.path.abspath(original_path):
        sys.exit("refusing to overwrite the original")
    with open(out_path, "wb") as f:
        f.write(data)

    print(f"{out_path}: {len(data)} bytes ({len(data) - len(original.data):+d})")
    for i in redo:
        fr = original.frames[i]
        print(f"  frame {i:4} re-encoded, image {fr.out_start:#010x}-{fr.out_end:#010x}")
    print("  verified: container checks pass, decodes to the given image")


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check", help="verify an update file")
    c.add_argument("file")
    p = sub.add_parser("pack", help="build an update file for a modified image")
    p.add_argument("original", help="the original update file, e.g. firmware/fwdc239b.bin")
    p.add_argument("image", help="the modified unpacked image")
    p.add_argument("-o", "--output", required=True)
    args = ap.parse_args()
    if args.cmd == "check":
        sys.exit(check(args.file))
    pack(args.original, args.image, args.output)


if __name__ == "__main__":
    main()
