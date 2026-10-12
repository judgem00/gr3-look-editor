#!/bin/sh
# Start the look editor. Creates its virtualenv on first run.
#   ./run.sh --update fwdc239b.bin [--dng photo.DNG]
set -e
here="$(cd "$(dirname "$0")" && pwd)"
if [ ! -x "$here/.venv/bin/python" ]; then
    python3 -m venv "$here/.venv"
    "$here/.venv/bin/pip" install -q -r "$here/editor/requirements.txt"
fi
exec "$here/.venv/bin/python" "$here/editor/server.py" "$@"
