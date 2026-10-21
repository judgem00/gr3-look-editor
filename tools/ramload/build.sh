#!/bin/sh
# Builds the RAM dump and the RAM loader and puts together the files for the
# SD card (docs/firmware/ram-loader.md). Needs clang with the ARM target and
# ld.lld.
#
#   tools/ramload/build.sh [out]      default out: work/ramload
#
# out/SD_DUMP      copy its contents to the card root: dump the RAM (phase 0)
# out/SD_LOAD      copy its contents to the card root, and LOOK.PAT from the
#                  editor into SCRIPT/RAMLOAD: load a look into RAM
# out/SCRIPT_ON    ModeSet files that turn SD scripts on (MENU held at power-on)
# out/SCRIPT_OFF   the same, to turn them off again
#
# Only these four folders are replaced; everything else in out (dumps,
# firmware backups) stays.
#
# It also refreshes bin/LOAD.BIN, the loader the editor puts into its card
# package (core/src/cardzip.rs): commit it whenever load.c, loadcore.h,
# common.h or core.h change, so git diff shows a stale binary.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
out=${1:-$repo/work/ramload}
lld=$(command -v ld.lld)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for prog in dump load; do
    clang --target=armv7-linux-gnueabi -march=armv7-a -marm -O2 -ffreestanding \
        -fno-builtin -mfpu=none -mfloat-abi=soft -fno-stack-protector -fomit-frame-pointer -fno-pic \
        -c "$here/$prog.c" -o "$tmp/$prog.o"
    "$lld" -m armelf_linux_eabi -static -e _start --build-id=none \
        -o "$tmp/$(echo "$prog" | tr a-z A-Z).BIN" "$tmp/$prog.o"
done

mkdir -p "$out"
for d in SD_DUMP SD_LOAD SCRIPT_ON SCRIPT_OFF SD_ROOT; do
    rm -rf "${out:?}/$d"
done
mkdir -p "$out/SD_DUMP/SCRIPT/RAMLOAD" "$out/SD_LOAD/SCRIPT/RAMLOAD" "$out/SCRIPT_ON" "$out/SCRIPT_OFF"
cp "$here/card/dump.ttl" "$out/SD_DUMP/SCRIPT/STARTUP.TTL"
cp "$here/card/DUMP.SH" "$tmp/DUMP.BIN" "$out/SD_DUMP/SCRIPT/RAMLOAD/"
cp "$here/card/load.ttl" "$out/SD_LOAD/SCRIPT/STARTUP.TTL"
cp "$here/card/LOAD.SH" "$tmp/LOAD.BIN" "$out/SD_LOAD/SCRIPT/RAMLOAD/"
mkdir -p "$here/bin"
cp "$tmp/LOAD.BIN" "$here/bin/LOAD.BIN"

# GR III: the ModeSet file is named after the RTOS major and minor numbers
# (GR IIIx: 00078490.609). Command and DEVELOP.MOD as grkit writes them
# (crates/grkit-core/src/runtime.rs; MIT, see LICENSE-grkit.txt).
develop='\007\001\054\037\020\003\036\026\005\055'
printf '[SCRIPT_EN_MODE EN]\r\n' > "$out/SCRIPT_ON/00078350.588"
printf '[SCRIPT_EN_MODE DIS]\r\n' > "$out/SCRIPT_OFF/00078350.588"
printf "$develop" > "$out/SCRIPT_ON/DEVELOP.MOD"
printf "$develop" > "$out/SCRIPT_OFF/DEVELOP.MOD"

echo "$out"
for d in SD_DUMP SD_LOAD SCRIPT_ON SCRIPT_OFF; do
    find "$out/$d" -type f | sort | while read -r f; do
        printf '  %8d  %s\n' "$(wc -c < "$f")" "${f#"$out"/}"
    done
done
