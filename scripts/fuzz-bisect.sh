#!/usr/bin/env bash
# Find the first operation count at which the native fuzz driver fails for one seed, then print the
# trace of the last few operations.   Usage: scripts/fuzz-bisect.sh <seed> [max_ops=4000] [pattern]
# Needs a built driver: cargo build --manifest-path fuzz/Cargo.toml --bin native
set -u
seed="$1"; hi="${2:-4000}"; pat="${3:-CRITICAL|WARNING|BUG|Trace/breakpoint|Segmentation|panicked|stack overflow|foreign exception|Aborted}"
exe="${CARGO_TARGET_DIR:-fuzz/target}/debug/native"
run() {
  timeout 40 xvfb-run -a env RUNGUI_FUZZ_LIMIT="$1" ${TRACE:+RUNGUI_FUZZ_TRACE=1} \
    G_DEBUG=fatal-warnings NO_AT_BRIDGE=1 LANG=C.UTF-8 "$exe" 1 "$seed" 2>&1
}
lo=1
while [ "$lo" -lt "$hi" ]; do
  mid=$(( (lo + hi) / 2 ))
  if run "$mid" | grep -Eq "$pat"; then hi=$mid; else lo=$((mid + 1)); fi
done
echo "seed $seed first fails with RUNGUI_FUZZ_LIMIT=$lo; last operations:"
TRACE=1 run "$lo" | grep -E '^(op|  )' | tail -n 6
