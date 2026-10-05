#!/usr/bin/env bash
# Find the first operation count at which the native fuzz driver fails for one seed, then print the
# trace of the last few operations.
#
#   scripts/fuzz-bisect.sh <seed> [max_ops=4000] [pattern] [first_seed=<seed>]
#
# `first_seed` < `seed` replays the earlier seeds in full first (some failures need the state they
# leave behind) and applies the limit to `seed` only. Needs a built driver:
#   cargo build --manifest-path fuzz/Cargo.toml --bin native     (add --features emulate-mac for GNUstep)
# and honours CARGO_TARGET_DIR (default: target/debug/native). EXTRA_ENV="NSZombieEnabled=YES" adds
# environment variables to every run.
set -u
seed="$1"; hi="${2:-4000}"
pat="${3:-CRITICAL|WARNING|BUG|Trace/breakpoint|Segmentation|panicked|stack overflow|foreign exception|Aborted|deallocated}"
first="${4:-$seed}"
exe="${CARGO_TARGET_DIR:-target}/debug/native"
run() {
  timeout 300 xvfb-run -a env RUNGUI_FUZZ_LIMIT="$1" RUNGUI_FUZZ_LIMIT_SEED="$seed" ${TRACE:+RUNGUI_FUZZ_TRACE=1} \
    G_DEBUG=fatal-warnings NO_AT_BRIDGE=1 LANG=C.UTF-8 ${EXTRA_ENV:-} "$exe" "$((seed - first + 1))" "$first" 2>&1
}
lo=1
while [ "$lo" -lt "$hi" ]; do
  mid=$(( (lo + hi) / 2 ))
  if run "$mid" | grep -Eq "$pat"; then hi=$mid; else lo=$((mid + 1)); fi
done
echo "seed $seed first fails with RUNGUI_FUZZ_LIMIT=$lo; last operations:"
TRACE=1 run "$lo" | grep -E '^(op|  )' | tail -n 6
