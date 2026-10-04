#!/usr/bin/env bash
# GTK backend smoke test: runs examples/smoke under Xvfb, clicks/types with xdotool and checks the
# callbacks fired; then examples/smoke2 (splitter drags, monospace, window position, shrinking,
# table/tree in splitters) under G_DEBUG=fatal-warnings. Usage: scripts/gtk-smoke.sh (honours CARGO_TARGET_DIR)
set -u
cd "$(dirname "$0")/.."
cargo build --example smoke --example smoke2 || exit 1
exe="${CARGO_TARGET_DIR:-target}/debug/examples/smoke"
xvfb-run -a -s "-screen 0 1024x768x24" bash -c '
set -u
export LANG=C.UTF-8 LC_ALL=C.UTF-8
exe="$1"; out=$(mktemp)
"$exe" > "$out" 2>"$out.err" &
pid=$!
for i in $(seq 50); do grep -q READY "$out" && break; sleep 0.2; done
grep -q READY "$out" || { echo "FAIL: no READY"; cat "$out" "$out.err"; kill $pid; exit 1; }
wa=$(xdotool search --name "^smoke-a$" | head -1)
wb=$(xdotool search --name "^smoke-b$" | head -1)
xdotool windowmove $wb 500 0; sleep 0.3
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
xdotool mousemove $((ox+x+40)) $((oy+y+12)) click 1; sleep 0.3; xdotool key plus; sleep 0.5; xdotool key Down Down; sleep 0.3
import -window root /tmp/smoke-a.png
click dlg; sleep 0.7; xdotool key Return; sleep 0.5
# right click -> popup menu, then choose the item with the keyboard
read -r x y w h < <(awk "\$1==\"BOUNDS\"&&\$2==\"btn\"{print \$3,\$4,\$5,\$6}" "$out")
xdotool mousemove $((ox+x+w/2)) $((oy+y+h/2)) click 3; sleep 0.7; xdotool key Down Return; sleep 0.5
xdotool windowfocus $wb; sleep 0.3; xdotool key ctrl+q; sleep 1
kill $pid 2>/dev/null; wait $pid 2>/dev/null
cat "$out"; cat "$out.err"
fail=0
for pat in "^CLICK" "TEXT héllo" "TOGGLE true" "^SELECT Some" "^VALUE" "^COMBO Some" "MENU_QUIT" "^BYE" "HANDLE true" "^RB true" "^TREE_EXPAND true" "^TABLE_SEL Some" "^TREE_SEL true" "^DIALOG Yes" "^CTXMENU"; do
  grep -Eq "$pat" "$out" && echo "ok   $pat" || { echo "FAIL $pat"; fail=1; }
done
exit $fail
' _ "$exe"
rc1=$?

exe2="${CARGO_TARGET_DIR:-target}/debug/examples/smoke2"
xvfb-run -a -s "-screen 0 1024x768x24" bash -c '
set -u
export LANG=C.UTF-8 LC_ALL=C.UTF-8 G_DEBUG=fatal-warnings NO_AT_BRIDGE=1
exe="$1"; out=$(mktemp)
"$exe" > "$out" 2>&1 &
pid=$!
for i in $(seq 50); do grep -q READY "$out" && break; sleep 0.2; done
grep -q READY "$out" || { echo "FAIL: no READY"; cat "$out"; kill $pid; exit 1; }
w=$(xdotool search --onlyvisible --name "^smoke2$" | head -1)
bounds() { awk -v n="$1" "\$1==\"BOUNDS\"&&\$2==n{print \$3,\$4,\$5,\$6}" "$out"; }
read -r px py < <(sed -n "s/^POS Some((\([0-9-]*\), \([0-9-]*\)))/\1 \2/p" "$out")
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
read -r x y tw th < <(bounds tree); xdotool mousemove $((px+x+40)) $((py+y+12)) click 1; sleep 0.3; xdotool key plus; sleep 0.4
# keyboard on the sashes: a click focuses one; arrows step 10px (Shift: 50px), Home/End jump to the limits
last() { sed -n "s/^$1 //p" "$out" | tail -1; }
hnow=$(last HSPLIT)
xdotool mousemove $((px+hx+hnow+3)) $((py+hy+hh/2)) click 1; sleep 0.3
xdotool key Left Left shift+Left; sleep 0.4
hkeys=$(last HSPLIT)
xdotool key Home; sleep 0.3; hhome=$(last HSPLIT)
xdotool key End; sleep 0.3; hend=$(last HSPLIT)
xdotool key Up; sleep 0.2; hup=$(last HSPLIT)          # Up is not an arrow for a side-by-side split (GTK moves the focus)
vnow=$(last VSPLIT)
xdotool mousemove $((px+vx+vw/2)) $((py+vy+vnow+3)) click 1; sleep 0.3
xdotool key Down shift+Down; sleep 0.4
vkeys=$(last VSPLIT)
xdotool key Home; sleep 0.3; vhome=$(last VSPLIT)
xdotool key Left; sleep 0.2; vleft=$(last VSPLIT)       # Left is not an arrow for a stacked split
# window position (user move) and shrinking below the natural size
xdotool windowmove $w 300 200; sleep 0.5
xdotool windowsize $w 100 100; sleep 0.7
eval "$(xdotool getwindowgeometry --shell $w)"; echo "SHRUNK $WIDTH $HEIGHT" >> "$out"
kill -0 $pid 2>/dev/null || echo "FAIL: died" >> "$out"
kill $pid 2>/dev/null; wait $pid 2>/dev/null
cat "$out"
fail=0
hmax=$(sed -n "s/^HSPLIT //p" "$out" | sort -n | tail -1); vmin=$(sed -n "s/^VSPLIT //p" "$out" | sort -n | head -1)
[ "${hmax:-0}" -ge $((hpos+50)) ] && echo "ok   splitter drag right ($hpos -> $hmax)" || { echo "FAIL horizontal sash drag"; fail=1; }
[ -n "$vmin" ] && [ "$vmin" -le $((vpos-20)) ] && echo "ok   splitter drag up ($vpos -> $vmin)" || { echo "FAIL vertical sash drag"; fail=1; }
[ "${hkeys:-0}" = $((hnow-70)) ] && echo "ok   sash keys: Left x2 + Shift+Left ($hnow -> $hkeys)" || { echo "FAIL sash arrow keys ($hnow -> ${hkeys:-none})"; fail=1; }
[ "${hup:-}" = "${hend:-x}" ] && echo "ok   sash ignores the cross-axis arrow" || { echo "FAIL cross-axis arrow moved the sash ($hend -> ${hup:-none})"; fail=1; }
[ "${hhome:-}" = 60 ] && [ "${hend:-0}" -gt 60 ] && echo "ok   sash Home/End ($hhome, $hend)" || { echo "FAIL sash Home/End (${hhome:-none}, ${hend:-none})"; fail=1; }
[ "${vkeys:-0}" = $((vnow+60)) ] && echo "ok   vertical sash keys: Down + Shift+Down ($vnow -> $vkeys)" || { echo "FAIL vertical sash keys ($vnow -> ${vkeys:-none})"; fail=1; }
[ -n "${vhome:-}" ] && [ "$vhome" -lt "${vkeys:-0}" ] && [ "${vleft:-}" = "$vhome" ] && echo "ok   vertical sash Home ($vhome), Left ignored" || { echo "FAIL vertical sash Home/Left (${vhome:-none}, ${vleft:-none})"; fail=1; }
for pat in "^MONO true true WRAP false" "^MOVED 300 200" "^TABLE_SEL Some" "^TREE_SEL true" "^TREE_EXPAND true" "^RESIZED 100 100" "^SHRUNK ([0-9]|[1-9][0-9]|1[01][0-9]) "; do
  grep -Eq "$pat" "$out" && echo "ok   $pat" || { echo "FAIL $pat"; fail=1; }
done
grep -Eq "CRITICAL|WARNING|panicked|Segmentation|^FAIL" "$out" && { echo "FAIL: warnings or crash"; fail=1; }
exit $fail
' _ "$exe2"
rc2=$?
exit $((rc1 | rc2))
