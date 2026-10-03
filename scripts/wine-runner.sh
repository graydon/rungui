#!/usr/bin/env bash
# Cargo runner for x86_64-pc-windows-gnu (see .cargo/config.toml): runs the built .exe under wine.
# Uses a private prefix under the cargo target dir so it never touches ~/.wine. Set WINEDEBUG to see wine logs.
# Without $DISPLAY (e.g. a headless shell or CI) it wraps the run in xvfb-run.
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
export WINEPREFIX="${WINEPREFIX:-${CARGO_TARGET_DIR:-$here/target}/wineprefix}"
export WINEDEBUG="${WINEDEBUG:--all}"
if ! command -v wine >/dev/null; then
  echo "wine-runner: wine not installed (scripts/setup-devcontainer.sh installs it on x86_64 hosts)" >&2
  exit 127
fi
if [ "$(uname -m)" != x86_64 ]; then
  echo "wine-runner: wine cannot run x86_64 PE files on $(uname -m); build only, or run on x86_64" >&2
  exit 126
fi
if [ -z "${DISPLAY:-}" ] && command -v xvfb-run >/dev/null; then
  exec xvfb-run -a -s '-screen 0 1280x800x24' wine "$@"
fi
exec wine "$@"
