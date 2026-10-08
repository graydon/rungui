#!/usr/bin/env bash
# Smoke test for modal windows, prompt, Enter/Escape, multi-select and the calendar: runs
# examples/smoke_dialogs under Xvfb with xdotool (GTK with G_DEBUG=fatal-warnings) and checks its output.
# BACKEND=wine runs the mingw-built Win32 backend under wine instead, BACKEND=gnustep the Cocoa
# backend on GNUstep (feature emulate-mac).
# Usage: scripts/smoke-dialogs.sh (honours CARGO_TARGET_DIR)
set -u
cd "$(dirname "$0")/.."
if [ "${BACKEND:-gtk}" = wine ]; then
  command -v wine >/dev/null || { echo "SKIP: wine not installed"; exit 0; }
  cargo build --target x86_64-pc-windows-gnu --example smoke_dialogs || exit 1
  tdir="$(cd "${CARGO_TARGET_DIR:-target}" && pwd)"
  export WINEPREFIX="$tdir/wineprefix-smoke" WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml="
  exe="$tdir/x86_64-pc-windows-gnu/debug/examples/smoke_dialogs.exe"
  export RUN="timeout 120 wine"
elif [ "${BACKEND:-gtk}" = gnustep ]; then
  cargo build --features emulate-mac --example smoke_dialogs || exit 1
  exe="${CARGO_TARGET_DIR:-target}/debug/examples/smoke_dialogs"
  export RUN=""
else
  cargo build --example smoke_dialogs || exit 1
  exe="${CARGO_TARGET_DIR:-target}/debug/examples/smoke_dialogs"
  export RUN=""
fi
xvfb-run -a -s "-screen 0 1024x768x24" bash -c '
set -u
export LANG=C.UTF-8 LC_ALL=C.UTF-8 G_DEBUG=fatal-warnings NO_AT_BRIDGE=1
exe="$1"; out=$(mktemp)
$RUN "$exe" > "$out" 2>&1 &
pid=$!
for i in $(seq 150); do grep -q READY "$out" && break; sleep 0.2; done
grep -q READY "$out" || { echo "FAIL: no READY"; cat "$out"; kill $pid; exit 1; }
w=$(xdotool search --onlyvisible --name "^smoke-dialogs$" | head -1)
xdotool windowfocus $w 2>/dev/null; sleep 0.3
eval "$(xdotool getwindowgeometry --shell $w)"; ox=$X; oy=$Y
[ "${BACKEND:-gtk}" = gnustep ] && { xdotool mousemove $((ox+3)) $((oy+3)) click 1; sleep 0.3; }  # the first click only activates the window
at() { read -r x y bw bh < <(awk -v n="$1" "\$1==\"BOUNDS\"&&\$2==n{print \$3,\$4,\$5,\$6}" "$out"); echo $((ox+x+bw/2+${2:-0})) $((oy+y+bh/2+${3:-0})); }
click() { read -r cx cy < <(at "$@"); xdotool mousemove $cx $cy click 1; sleep 0.3; }
# Enter in the text field
click field; xdotool type --delay 30 "abc"; xdotool key Return; sleep 0.3
# prompt: type over the selected text? the field starts with "old": append, then Enter
click ask; sleep 0.8; xdotool key End; xdotool type --delay 30 "X"; xdotool key Return; sleep 0.6
# prompt cancelled with Escape
click ask; sleep 0.8; xdotool key Escape; sleep 0.6
# a modal window: the main window must not react while it is up, Escape closes it
click modal; sleep 0.8
click field; xdotool type --delay 30 "zzz"; sleep 0.3
xdotool key Escape; sleep 0.6
xdotool windowfocus $w; sleep 0.3
# multi-select list: click the first row, extend with Shift+Down
read -r x y bw bh < <(awk "\$1==\"BOUNDS\"&&\$2==\"list\"{print \$3,\$4,\$5,\$6}" "$out"); lx=$((ox+x+bw/2)); ly=$((oy+y+9))
xdotool mousemove $lx $ly click 1; sleep 0.2
xdotool keydown shift; sleep 0.2; xdotool key Down; sleep 0.2; xdotool key Down; sleep 0.2; xdotool keyup shift; sleep 0.3
# multi-select table
read -r x y bw bh < <(awk "\$1==\"BOUNDS\"&&\$2==\"table\"{print \$3,\$4,\$5,\$6}" "$out"); tx=$((ox+x+30)); ty=$((oy+y+28))
xdotool mousemove $tx $ty click 1; sleep 0.2
xdotool keydown shift; sleep 0.2; xdotool key Down; sleep 0.2; xdotool keyup shift; sleep 0.3
# calendar: click a day cell (right of centre, lower half)
read -r cx cy < <(at cal 20 25); xdotool mousemove $cx $cy click 1; sleep 0.3
import -window root "${SMOKE_SHOTS:-/tmp}/smoke-dialogs-${BACKEND:-gtk}.png" 2>/dev/null
xdotool key --window $w ctrl+q 2>/dev/null
kill $pid 2>/dev/null; sleep 0.5; kill -9 $pid 2>/dev/null; wait $pid 2>/dev/null
cat "$out"
fail=0
for pat in "^TODAY true" "^ACTIVATE" "^PROMPT Some\(\"oldX\"\)" "^PROMPT None" "^MODAL_OPEN" "^MODAL_CANCEL" "^MODAL_DONE" "^LIST_SEL \[[0-9]+, [0-9]+" "^TABLE_SEL \[[0-9]+, [0-9]+\]" "^DATE 2031-"; do
  grep -Eq "$pat" "$out" && echo "ok   $pat" || { echo "FAIL $pat"; fail=1; }
done
# typing into the main window while the modal was up must not have reached it
grep -q "zzz" "$out" && { echo "FAIL: main window took input while modal"; fail=1; }
exit $fail
' _ "$exe"
