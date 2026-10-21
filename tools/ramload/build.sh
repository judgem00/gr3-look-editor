#!/bin/sh
# Builds the RAM dump program and puts together the files for the SD card
# (docs/firmware/ram-loader.md). Needs clang with the ARM target and ld.lld.
#
#   tools/ramload/build.sh [out]      default out: work/ramload
#
# out/SD_ROOT          copy its contents to the card root
# out/SCRIPT_ON        ModeSet files that turn SD scripts on (MENU held at power-on)
# out/SCRIPT_OFF       the same, to turn them off again
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
out=${1:-$repo/work/ramload}
lld=$(command -v ld.lld)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

clang --target=armv7-linux-gnueabi -march=armv7-a -marm -O2 -ffreestanding \
    -fno-builtin -mfpu=none -mfloat-abi=soft -fno-stack-protector -fomit-frame-pointer -fno-pic \
    -c "$here/dump.c" -o "$tmp/dump.o"
"$lld" -m armelf_linux_eabi -static -e _start --build-id=none \
    -o "$tmp/DUMP.BIN" "$tmp/dump.o"

rm -rf "$out"
mkdir -p "$out/SD_ROOT/SCRIPT/RAMLOAD" "$out/SCRIPT_ON" "$out/SCRIPT_OFF"
cp "$here/card/STARTUP.TTL" "$out/SD_ROOT/SCRIPT/STARTUP.TTL"
cp "$here/card/DUMP.SH" "$tmp/DUMP.BIN" "$out/SD_ROOT/SCRIPT/RAMLOAD/"

# GR III: the ModeSet file is named after the RTOS major and minor numbers
# (GR IIIx: 00078490.609). Command and DEVELOP.MOD as grkit writes them
# (crates/grkit-core/src/runtime.rs, MIT).
develop='\007\001\054\037\020\003\036\026\005\055'
printf '[SCRIPT_EN_MODE EN]\r\n' > "$out/SCRIPT_ON/00078350.588"
printf '[SCRIPT_EN_MODE DIS]\r\n' > "$out/SCRIPT_OFF/00078350.588"
printf "$develop" > "$out/SCRIPT_ON/DEVELOP.MOD"
printf "$develop" > "$out/SCRIPT_OFF/DEVELOP.MOD"

echo "$out"
find "$out" -type f | sort | while read -r f; do
    printf '  %8d  %s\n' "$(wc -c < "$f")" "${f#"$out"/}"
done
