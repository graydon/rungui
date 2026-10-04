#!/usr/bin/env bash
# Win32 backend smoke test: runs the mingw-built examples/smoke_controls.exe and smoke_layout.exe under wine on a
# private Xvfb, clicks/types with xdotool (wine windows are ordinary X windows) and checks the
# callbacks fired (same checks as smoke-gtk.sh, plus table/tree/popup menu/splitter/monospace).
# Needs wine, Xvfb, xdotool, ImageMagick. Usage: scripts/smoke-win32.sh (honours CARGO_TARGET_DIR)
set -u
cd "$(dirname "$0")/.."
command -v wine >/dev/null || { echo "SKIP: wine not installed"; exit 0; }
[ "$(uname -m)" = x86_64 ] || { echo "SKIP: wine cannot run x86_64 PE files on $(uname -m)"; exit 0; }
cargo build --target x86_64-pc-windows-gnu --example smoke_controls --example smoke_layout || exit 1
tdir="$(cd "${CARGO_TARGET_DIR:-target}" && pwd)"
export WINEPREFIX="$tdir/wineprefix-smoke" WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml="
exe="$tdir/x86_64-pc-windows-gnu/debug/examples/smoke_controls.exe"
xvfb-run -a -s "-screen 0 1024x768x24" bash -c '
set -u
export LANG=C.UTF-8 LC_ALL=C.UTF-8
exe="$1"; out=$(mktemp)
timeout 120 wine "$exe" > "$out" 2>"$out.err" &
pid=$!
for i in $(seq 150); do grep -q READY "$out" && break; sleep 0.2; done
grep -q READY "$out" || { echo "FAIL: no READY"; cat "$out" "$out.err"; kill $pid; exit 1; }
wa=$(xdotool search --name "^smoke-a$" | head -1)
wb=$(xdotool search --name "^smoke-b$" | head -1)
eval "$(xdotool getwindowgeometry --shell $wa)"; ox=$X; oy=$Y
click() {
  read -r x y w h < <(awk -v n="$1" "\$1==\"BOUNDS\"&&\$2==n{print \$3,\$4,\$5,\$6}" "$out")
  xdotool mousemove $((ox+x+w/2)) $((oy+y+h/2)) click 1; sleep 0.3
}
click btn
click txt; xdotool type --delay 30 "héllo"; sleep 0.3
click chk
click slider
click list
click combo; sleep 0.3; xdotool key Down Return; sleep 0.3
click rb
click table 2>/dev/null; xdotool key Down; sleep 0.3
read -r x y w h < <(awk "\$1==\"BOUNDS\"&&\$2==\"tree\"{print \$3,\$4,\$5,\$6}" "$out")
xdotool mousemove $((ox+x+40)) $((oy+y+12)) click 1; sleep 0.3; xdotool key Right; sleep 0.5; xdotool key Down Down; sleep 0.3
import -window root "${SMOKE_SHOTS:-/tmp}/smoke-win32-a.png" 2>/dev/null
click dlg; sleep 0.7; xdotool key Return; sleep 0.5
# right click -> popup menu, then choose the item with the keyboard
read -r x y w h < <(awk "\$1==\"BOUNDS\"&&\$2==\"btn\"{print \$3,\$4,\$5,\$6}" "$out")
xdotool mousemove $((ox+x+w/2)) $((oy+y+h/2)) click 3; sleep 0.7; xdotool key Down Return; sleep 0.5
eval "$(xdotool getwindowgeometry --shell $wb)"
xdotool mousemove $((X+WIDTH/2)) $((Y+WIDTH/4+10)) click 1; sleep 0.5; xdotool key ctrl+q; sleep 1
kill $pid 2>/dev/null; wait $pid 2>/dev/null
cat "$out"; cat "$out.err"
fail=0
for pat in "^CLICK" "TEXT héllo" "TOGGLE true" "^SELECT Some" "^VALUE" "^COMBO Some" "MENU_QUIT" "^BYE" "HANDLE true" "^RB true" "^TREE_EXPAND true" "^TABLE_SEL Some" "^TREE_SEL true" "^DIALOG Yes" "^CTXMENU"; do
  grep -Eq "$pat" "$out" && echo "ok   $pat" || { echo "FAIL $pat"; fail=1; }
done
exit $fail
' _ "$exe"
rc1=$?

exe2="$tdir/x86_64-pc-windows-gnu/debug/examples/smoke_layout.exe"
xvfb-run -a -s "-screen 0 1024x768x24" bash -c '
set -u
export LANG=C.UTF-8 LC_ALL=C.UTF-8
exe="$1"; out=$(mktemp)
timeout 120 wine "$exe" > "$out" 2>&1 &
pid=$!
for i in $(seq 150); do grep -q READY "$out" && break; sleep 0.2; done
grep -q READY "$out" || { echo "FAIL: no READY"; cat "$out"; kill $pid; exit 1; }
w=$(xdotool search --onlyvisible --name "^smoke-layout$" | head -1)
bounds() { awk -v n="$1" "\$1==\"BOUNDS\"&&\$2==n{print \$3,\$4,\$5,\$6}" "$out"; }
eval "$(xdotool getwindowgeometry --shell $w)"; px=$X; py=$Y
# the client area starts at the window origin (no menu bar); sash = strip after the first pane
read -r hx hy hw hh < <(bounds hs); read -r vx vy vw vh < <(bounds vs)
hpos=$(sed -n "s/^HPOS \([0-9]*\) .*/\1/p" "$out"); vpos=$(sed -n "s/^HPOS [0-9]* VPOS \([0-9]*\)/\1/p" "$out")
# drag the horizontal splitter sash right by 60px, the vertical one up by 30px
sx=$((px+hx+hpos+3)); sy=$((py+hy+hh/2))
xdotool mousemove $sx $sy mousedown 1; sleep 0.2
for d in 15 30 45 60; do xdotool mousemove $((sx+d)) $sy; sleep 0.1; done
xdotool mouseup 1; sleep 0.3
sx=$((px+vx+vw/2)); sy=$((py+vy+vpos+3))
xdotool mousemove $sx $sy mousedown 1; sleep 0.2
for d in -10 -20 -30; do xdotool mousemove $sx $((sy+d)); sleep 0.1; done
xdotool mouseup 1; sleep 0.3
# table and tree clicks in their splitter panes
read -r x y tw th < <(bounds table); xdotool mousemove $((px+x+150)) $((py+y+35)) click 1; sleep 0.3
read -r x y tw th < <(bounds tree); xdotool mousemove $((px+x+40)) $((py+y+12)) click 1; sleep 0.3; xdotool key Right; sleep 0.4
# no window manager runs under Xvfb, so wine ignores external moves/resizes: Moved, min size and
# shrinking are not exercised here (see doc/STATUS.md)
kill -0 $pid 2>/dev/null || echo "FAIL: died" >> "$out"
kill $pid 2>/dev/null; wait $pid 2>/dev/null
cat "$out"
fail=0
hmax=$(sed -n "s/^HSPLIT //p" "$out" | sort -n | tail -1); vmin=$(sed -n "s/^VSPLIT //p" "$out" | sort -n | head -1)
[ "${hmax:-0}" -ge $((hpos+50)) ] && echo "ok   splitter drag right ($hpos -> $hmax)" || { echo "FAIL horizontal sash drag"; fail=1; }
[ -n "$vmin" ] && [ "$vmin" -le $((vpos-20)) ] && echo "ok   splitter drag up ($vpos -> $vmin)" || { echo "FAIL vertical sash drag"; fail=1; }
for pat in "^MONO true true WRAP false" "^POS Some\(\(120, 90\)\)" "^TABLE_SEL Some" "^TREE_SEL true" "^TREE_EXPAND true"; do
  grep -Eq "$pat" "$out" && echo "ok   $pat" || { echo "FAIL $pat"; fail=1; }
done
grep -Eq "panicked|Segmentation|^FAIL" "$out" && { echo "FAIL: warnings or crash"; fail=1; }
exit $fail
' _ "$exe2"
rc2=$?
wineserver -k 2>/dev/null
exit $((rc1 | rc2))
