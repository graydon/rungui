#!/usr/bin/env bash
# Random-operation fuzzing of a REAL backend (see fuzz/src/ops.rs, fuzz/src/bin/native.rs): every
# seed is a deterministic program of hostile API calls run inside the toolkit's event loop under
# Xvfb. On GTK any GLib warning/critical is fatal and prints the Rust stack plus the GTK widget tree.
#
#   scripts/fuzz-native.sh [seeds=300] [first_seed=1]     BACKEND=gtk|gnustep (default gtk)
#   ASAN=1 scripts/fuzz-native.sh ...                      AddressSanitizer build (needs nightly)
#   SOAK=1 scripts/fuzz-native.sh [iterations=1500]        leak soak instead: create/destroy loop
#   MODAL=0|messages|files|all ...                         dialogs the driver may open (default: none; GTK trips over its own key handling when they are dismissed this way)
#
# GNUstep runs get NSZombieEnabled=YES (messages to freed objects are reported). Reproduce a failing
# seed with `scripts/fuzz-native.sh 1 <seed>` and narrow it with scripts/fuzz-bisect.sh.
set -u
cd "$(dirname "$0")/.."
seeds="${1:-300}"; first="${2:-1}"
BACKEND="${BACKEND:-gtk}"
T="${CARGO_TARGET_DIR:-target}"
case "$BACKEND" in
  # (on GTK the driver aborts on GLib warnings itself and skips known toolkit ones)
  gtk) feat=(); env_extra=(NO_AT_BRIDGE=1) ;;
  gnustep) feat=(--features emulate-mac); env_extra=(NSZombieEnabled=YES) ;;
  *) echo "unknown BACKEND $BACKEND"; exit 2 ;;
esac
bin=native; args=("$seeds" "$first")
[ "${SOAK:-0}" = 1 ] && { bin=soak; args=("$seeds"); }
if [ "${ASAN:-0}" = 1 ]; then
  T="$T/asan"
  export CARGO_TARGET_DIR="$T"
  triple="$(rustc -vV | sed -n 's/^host: //p')"
  RUSTFLAGS="-Zsanitizer=address" cargo +nightly build --manifest-path fuzz/Cargo.toml "${feat[@]}" \
    --bin "$bin" --target "$triple" || exit 1
  exe="$T/$triple/debug/$bin"
  env_extra+=(ASAN_OPTIONS=detect_leaks=0)   # GTK and fontconfig own caches LeakSanitizer cannot tell from leaks
else
  export CARGO_TARGET_DIR="$T"
  cargo build --manifest-path fuzz/Cargo.toml "${feat[@]}" --bin "$bin" || exit 1
  exe="$T/debug/$bin"
fi
out="$(mktemp)"
# MODAL=messages lets the driver open message boxes, MODAL=files file dialogs too and
# MODAL=all popup menus as well; MODAL=0 none. They block the toolkit's loop, so a background loop
# presses Escape to dismiss whatever is open. The last two are opt-in because GTK 3 itself trips
# over them now and then (assertions inside its file chooser, submenu arrows drawn with negative
# sizes in popup menus).
modal="${MODAL:-0}"
[ "$bin" = native ] && [ "$BACKEND" = gtk ] || modal=0   # the dismissing loop below knows GTK dialogs only
[ "$modal" != 0 ] && env_extra+=(RUNGUI_FUZZ_MODAL="$modal")
export MODAL_PUMP=0
[ "$modal" != 0 ] && MODAL_PUMP=1
xvfb-run -a -s "-screen 0 1280x1024x24" bash -c '
  if [ "$MODAL_PUMP" = 1 ]; then
    # no window manager runs here: focus each visible DIALOG window (the toolkit marks them with a
    # window type hint) and press Escape; ordinary windows are left alone
    ( while sleep 0.15; do
        for w in $(xdotool search --onlyvisible --name "" 2>/dev/null); do
          if xprop -id "$w" _NET_WM_WINDOW_TYPE 2>/dev/null | grep -q DIALOG; then
            xdotool windowfocus "$w" key --clearmodifiers Escape 2>/dev/null
          fi
        done
      done ) &
    pump=$!
  fi
  env LANG=C.UTF-8 "$@"
  rc=$?
  [ -n "${pump:-}" ] && kill "$pump" 2>/dev/null
  exit $rc
' _ "${env_extra[@]}" "$exe" "${args[@]}" > "$out" 2>&1
rc=$?
# GNUstep logs harmless NSAssert chatter about negative view sizes and font offsets
grep -vE '^seed [0-9]+$|Failed to determine offsets|given negative (width|height)' "$out" | tail -n 40
if [ $rc -eq 0 ] && grep -qE '^done|^iteration' "$out"; then echo "PASS ($bin, $BACKEND)"; exit 0; fi
keep="$T/fuzz-native-failure.log"; cp "$out" "$keep"
echo "FAIL ($bin, $BACKEND, exit $rc); last seed: $(grep -E '^seed [0-9]+$' "$out" | tail -n 1); full log: $keep"
exit 1
