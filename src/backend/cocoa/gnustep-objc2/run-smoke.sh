#!/bin/bash
# Run scripts/smoke-gnustep.sh (xdotool-driven smoke test of the Cocoa backend) against the private
# libobjc2 GNUstep stack built by build-gnustep-libobjc2.sh. Requires the emulate-mac Cargo patch
# (emulate-mac.Cargo.toml.snippet) to be applied. Usage: run-smoke.sh PREFIX
set -euo pipefail
P=$(realpath "${1:?usage: run-smoke.sh PREFIX}")
export RUNGUI_GNUSTEP_PREFIX=$P
# shellcheck disable=SC1091
set +u # GNUstep.sh uses unset variables
. "$P/GS/share/GNUstep/Makefiles/GNUstep.sh"
set -u
export LD_LIBRARY_PATH="$P/lib:$P/GS/local/lib:${LD_LIBRARY_PATH:-}"
cd "$(dirname "$0")/../../../.."
exec scripts/smoke-gnustep.sh
