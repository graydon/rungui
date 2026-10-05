#!/usr/bin/env bash
# Build/check every host mode. Usage: scripts/check-all.sh [--clean] [extra cargo args]
# Honors CARGO_TARGET_DIR (use a private one when other builds run concurrently).
# --clean wipes the target dir first. SKIP_SMOKE=1 skips the Xvfb smoke test.
set -u
clean=0
if [ "${1:-}" = "--clean" ]; then clean=1; shift; fi
fail=0
run() { echo "== $*"; "$@" || { echo "FAILED: $*"; fail=1; }; }
cd "$(dirname "$0")/.."
[ $clean = 1 ] && rm -rf "${CARGO_TARGET_DIR:-target}"
run cargo test "$@"                                             # core logic vs mock backend
run cargo test --features mock "$@"
run cargo check --features mock --examples "$@"
run cargo build --examples "$@"                                 # linux/GTK + examples
run cargo build --features emulate-mac --example hello --example kitchen_sink --example smoke_cocoa "$@"  # GNUstep (cocoa backend)
run cargo build --target x86_64-pc-windows-gnu --examples "$@"  # win32 via mingw
run cargo check --target aarch64-apple-darwin "$@"              # real cocoa cfg, type-check only
run cargo check --target x86_64-apple-darwin "$@"
run cargo build --release "$@"
if cargo clippy --version >/dev/null 2>&1; then
  # warnings are errors in every mode (the real backends are separate code per target)
  run cargo clippy --all-targets "$@" -- -D warnings
  run cargo clippy --all-targets --features mock "$@" -- -D warnings
  run cargo clippy --all-targets --features emulate-mac "$@" -- -D warnings
  run cargo clippy --all-targets --target x86_64-pc-windows-gnu "$@" -- -D warnings
  run cargo clippy --all-targets --target aarch64-apple-darwin "$@" -- -D warnings
  run cargo clippy --all-targets --target x86_64-apple-darwin "$@" -- -D warnings
  run cargo clippy --manifest-path fuzz/Cargo.toml --all-targets --features mock "$@" -- -D warnings
fi
# the core's own bookkeeping must scale (no operation quadratic in what the app holds)
run cargo run --release --features mock --example bench_mock "$@" -- --max-exponent 1.75
[ "${SKIP_SMOKE:-0}" = 1 ] || run scripts/smoke-gtk.sh          # GTK backend under Xvfb + xdotool
[ "${SKIP_SMOKE:-0}" = 1 ] || run scripts/smoke-gtk-soak.sh          # kitchen_sink soak: unicode, resize, dialogs, fatal GLib warnings
[ "${SKIP_SMOKE:-0}" = 1 ] || run scripts/smoke-filemanager.sh      # file manager example: navigate, preview, copy, rename, delete
[ "${SKIP_SMOKE:-0}" = 1 ] || run scripts/smoke-gnustep.sh      # Cocoa backend on GNUstep: sash drags, clicks, menus, move/resize
[ "${SKIP_SMOKE:-0}" = 1 ] || ! command -v wine >/dev/null || run scripts/smoke-win32.sh   # Win32 backend under wine + Xvfb
# random-operation fuzzing and leak soaks of the real toolkits (fuzz/, see doc/BUILDING.md)
[ "${SKIP_SMOKE:-0}" = 1 ] || run scripts/fuzz-native.sh 150
[ "${SKIP_SMOKE:-0}" = 1 ] || SOAK=1 run scripts/fuzz-native.sh 400
[ "${SKIP_SMOKE:-0}" = 1 ] || BACKEND=gnustep run scripts/fuzz-native.sh 60
[ "${SKIP_SMOKE:-0}" = 1 ] || BACKEND=gnustep SOAK=1 run scripts/fuzz-native.sh 200
exit $fail
