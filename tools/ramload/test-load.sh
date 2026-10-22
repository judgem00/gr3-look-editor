#!/bin/sh
# Tests the loader's checks and writes (loadcore.h) on the Mac
# (docs/firmware/ram-loader.md):
#
#   tools/ramload/test-load.sh [BASE [PRESET]]
#
# BASE is what the camera runs (default: work/ramload/card-fw/FWDC239B.BIN,
# the build of T1), PRESET the look to load (default: Vivid from Negative
# Film with grain, Standard with grain). Stock (work/fwdc239b.bin) is the
# other firmware. Needs python3 and cargo.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
base=${1:-$repo/work/ramload/card-fw/FWDC239B.BIN}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if [ $# -ge 2 ]; then
    preset=$2
else
    preset=$tmp/look.json
    echo '{"targets": {"Vivid": {"source": "NegativeFilm", "grain": 2}, "Standard": {"grain": 1}}}' > "$preset"
fi

# the RAM views: 0x53000000 up to the end of the .data copy source
python3 "$here/view.py" "$base" "$tmp/base.view"
python3 "$here/view.py" "$repo/work/fwdc239b.bin" "$tmp/other.view"

cargo run --release -q --manifest-path "$repo/core/Cargo.toml" --example ram_patch -- \
    "$preset" "$tmp/LOOK.PAT" "$base" > /dev/null

clang -O2 -Wall -Wextra -o "$tmp/test-load" "$here/test-load.c"
"$tmp/test-load" "$tmp/base.view" "$tmp/other.view" "$tmp/LOOK.PAT"
