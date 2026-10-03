#!/usr/bin/env bash
# Cocoa backend (on GNUstep, feature emulate-mac) smoke test under Xvfb + xdotool.
# Phase 1 drives examples/smoke_cocoa: typing (unicode), buttons, spin/slider/combo, tabs, table
# (select + header sort), tree expand, context menu, menu accelerators, splitter sash drags
# (both orientations), monospace, window move/resize. Phase 2 soaks examples/kitchen_sink.
# Prints ok/FAIL per check, exits nonzero on any FAIL. Screenshots land in
# $CARGO_TARGET_DIR/smoke-gnustep/ (default target/smoke-gnustep). Usage: scripts/smoke-gnustep.sh
set -u
cd "$(dirname "$0")/.."
cargo build --features emulate-mac --example smoke_cocoa --example kitchen_sink || exit 1
tdir="${CARGO_TARGET_DIR:-target}"
export SHOTS="$tdir/smoke-gnustep"; mkdir -p "$SHOTS"
export EXE_SMOKE="$tdir/debug/examples/smoke_cocoa" EXE_KS="$tdir/debug/examples/kitchen_sink"
exec xvfb-run -a -s "-screen 0 1280x1024x24" bash -c '
set -u
export LANG=C.UTF-8 LC_ALL=C.UTF-8
fail=0
pass() { echo "ok   $1"; }
bad()  { echo "FAIL $1"; fail=1; }
check() { grep -Eq "$2" "$out" && pass "$1" || bad "$1 (wanted /$2/)"; }
shot() { import -window root "$SHOTS/$1.png" 2>/dev/null; }
# wait_for FILE PATTERN [tries]: poll a file for a pattern
wait_for() { for i in $(seq ${3:-50}); do grep -Eq "$2" "$1" && return 0; sleep 0.2; done; return 1; }

out=$(mktemp); err="$out.err"
"$EXE_SMOKE" >"$out" 2>"$err" &
pid=$!
wait_for "$out" READY || { echo "FAIL: no READY"; cat "$out" "$err"; kill $pid; exit 1; }
alive() { kill -0 $pid 2>/dev/null || { echo "FAIL: died during $1"; cat "$out" "$err"; exit 1; }; }
win=$(xdotool search --name "^cc-a$" | head -1)
[ -n "$win" ] || { echo "FAIL: no cc-a window"; kill $pid; exit 1; }
xdotool windowfocus $win 2>/dev/null; sleep 0.3
eval "$(xdotool getwindowgeometry --shell $win)"; ox=$X; oy=$Y
[ "$ox,$oy" = "220,100" ] && pass "Window::set_position places the window ($ox,$oy)" || bad "set_position: window at $ox,$oy, wanted 220,100"
b() { awk -v n="$1" "\$1==\"BOUNDS\"&&\$2==n{print \$3,\$4,\$5,\$6}" "$out"; }
click() { read -r x y bw bh < <(b $1); xdotool mousemove $((ox+x+${2:-bw/2})) $((oy+y+${3:-bh/2})) click ${4:-1}; sleep 0.3; alive "click $1"; }
# click at an offset inside a widget: clickat NAME DX DY [BUTTON]
clickat() { read -r x y bw bh < <(b $1); xdotool mousemove $((ox+x+$2)) $((oy+y+$3)) click ${4:-1}; sleep 0.3; alive "clickat $1"; }
shot initial

check "READY / native handles" "^HANDLE true"
xdotool mousemove $((ox+5)) $((oy+690)) click 1; sleep 0.3   # first click only activates the window
click btn; check "button click" "^CLICK"
read -r x y bw bh < <(b spin); clickat spin $((bw-9)) $((bh/4)); check "spin up arrow" "^SPIN 1"
click txt; xdotool type --delay 120 "héllo ☃"; sleep 0.4; check "unicode typing in TextInput" "^TEXT héllo ☃"
click chk; check "checkbox -> monospace on" "^MONO true"
clickat hs 40 40; xdotool type --delay 20 "ab"; xdotool key Return; xdotool type --delay 20 "cd"; sleep 0.3; check "multi-line typing in TextArea" "^AREA ab\|cd"
shot typed

# horizontal splitter: sash at hs.x + position (default 200), thickness 6 -> drag +80 px
read -r hx hy hw hh < <(b hs)
sx=$((ox+hx+200+3)); sy=$((oy+hy+hh/2))
xdotool mousemove $sx $sy; sleep 0.2
xdotool mousedown 1; for d in 10 25 40 60 80; do xdotool mousemove $((sx+d)) $sy; sleep 0.08; done; xdotool mouseup 1; sleep 0.4
alive hsplit
shot hsplit
last=$(grep "^HSPLIT" "$out" | tail -1 | cut -d" " -f2)
[ -n "$last" ] && [ "$last" -ge 265 ] && [ "$last" -le 290 ] && pass "horizontal sash drag (position $last)" || bad "horizontal sash drag (HSPLIT=${last:-none})"
# vertical splitter: drag up by 40 from vs.y + 100 + 3, then check clamping at the minimum
read -r vx vy vw vh < <(b vs)
sx=$((ox+vx+vw/2)); sy=$((oy+vy+100+3))
xdotool mousemove $sx $sy; sleep 0.2
xdotool mousedown 1; for d in 10 20 30 40; do xdotool mousemove $sx $((sy-d)); sleep 0.08; done; xdotool mouseup 1; sleep 0.4
alive vsplit
last=$(grep "^VSPLIT" "$out" | tail -1 | cut -d" " -f2)
[ -n "$last" ] && [ "$last" -ge 50 ] && [ "$last" -le 70 ] && pass "vertical sash drag (position $last)" || bad "vertical sash drag (VSPLIT=${last:-none})"
# drag back down (+110) so the table is usable again
sy=$((sy-40))
xdotool mousemove $sx $sy; sleep 0.2
xdotool mousedown 1; for d in 20 50 80 110; do xdotool mousemove $sx $((sy+d)); sleep 0.08; done; xdotool mouseup 1; sleep 0.4
last=$(grep "^VSPLIT" "$out" | tail -1 | cut -d" " -f2)
[ -n "$last" ] && [ "$last" -ge 160 ] && [ "$last" -le 180 ] && pass "vertical sash drag back down (position $last)" || bad "vertical sash drag down (VSPLIT=${last:-none})"

click slider; check "slider click" "^VALUE"
click combo; sleep 0.4; xdotool key Down Return; sleep 0.4; check "combo selection" "^COMBO Some"
shot combo
# tabs: header "Second" sits ~74px right of the tab view origin, ~8px down
clickat tabs 74 8; check "tab switch by clicking the header" "^TAB Some\(1\)"
shot tabs
# table: click the 2nd row (header ~20px, rows ~16px), then header sort twice
clickat table 60 47; check "table row select" "^TABLE_SEL Some"
clickat table 40 8; check "table column click sorts (asc)" "^TABLE_COL 0 asc a"
clickat table 40 8; check "table column click sorts (desc)" "^TABLE_COL 0 desc c"
# tree: expand root by its disclosure triangle (first row)
tree_y=$((vy+last+6)); xdotool mousemove $((ox+vx+30)) $((oy+tree_y+9)) click 1; sleep 0.3; check "tree click" "^TREE_SEL true|^TREE_EXPAND true"
xdotool key Right; sleep 0.3; xdotool key Down; sleep 0.3
check "tree expand" "^TREE_EXPAND true"
shot tree
# context menu on the button: right click, then click the first item (just below the pointer)
click btn "" "" 3; sleep 0.8; shot ctxmenu
xdotool mousemove_relative 12 22; sleep 0.3; xdotool click 1; sleep 0.5
check "popup (context) menu" "^CTXMENU"
# accelerators (Command is Alt on GNUstep)
xdotool windowfocus $win 2>/dev/null; sleep 0.2
xdotool key alt+k; sleep 0.4; check "accelerator toggles CheckMenuItem" "^MENUTOGGLE true"
xdotool key alt+shift+g; sleep 0.4; check "accelerator with shift" "^MENU_GO"
# window move and resize notifications
xdotool windowmove $win 300 150; sleep 0.6; check "Event::Moved on user move" "^MOVED 300 150"
xdotool windowsize $win 500 600; sleep 0.6; check "resize event" "^RESIZED 500 600"
xdotool windowsize $win 200 200; sleep 0.6; alive resize-small
shot resized
check "min size honoured (client >= 300x400)" "^RESIZED (3[0-9][0-9]|[4-9][0-9][0-9]) [4-9][0-9][0-9]$"
xdotool key alt+q
for i in $(seq 25); do kill -0 $pid 2>/dev/null || break; sleep 0.2; done
if kill -0 $pid 2>/dev/null; then bad "still running after Quit accelerator"; kill $pid; fi
wait $pid 2>/dev/null
check "clean exit (BYE)" "^BYE"
check "quit accelerator" "^MENU_QUIT"
grep -Eq "panicked|Segmentation|Assertion|Unhandled exception|Uncaught exception" "$err" && { bad "errors in stderr"; grep -E "panicked|Segmentation|Assertion|exception" "$err" | head; } || pass "no crash output on stderr"
echo "--- smoke_cocoa events"; cat "$out"

# ---- phase 2: kitchen_sink soak (no crash while clicking around, resizing, dialogs)
out=$(mktemp); err="$out.err"
"$EXE_KS" >"$out" 2>"$err" &
pid=$!
for i in $(seq 50); do w=$(xdotool search --name "^rungui kitchen sink$" | head -1); [ -n "$w" ] && break; sleep 0.2; done
[ -n "$w" ] || { bad "kitchen_sink window"; kill $pid; exit 1; }
sleep 0.6; eval "$(xdotool getwindowgeometry --shell $w)"; kx=$X; ky=$Y
ksc() { xdotool mousemove $((kx+$1)) $((ky+$2)) click ${3:-1}; sleep 0.25; alive "ks click $1,$2"; }
for tx in 38 90 143 187; do ksc $tx 41; xdotool key Down Up; sleep 0.2; done
ksc 143 41; ksc 200 100; xdotool type --delay 10 "line one ☃"; xdotool key Return; xdotool type --delay 10 "line two 日本"; sleep 0.3; alive notes
shot ks-notes
ksc 187 41; shot ks-data
for s in "300 250" "900 700" "480 400" "200 150" "700 300"; do xdotool windowsize $w $s; sleep 0.3; alive resize; done
shot ks-resized
xdotool windowsize $w 480 400; sleep 0.3
xdotool key alt+q
for i in $(seq 25); do kill -0 $pid 2>/dev/null || break; sleep 0.2; done
if kill -0 $pid 2>/dev/null; then bad "kitchen_sink still running after Quit"; kill $pid; else pass "kitchen_sink soak survived and quit"; fi
wait $pid 2>/dev/null
grep -Eq "panicked|Segmentation|Assertion|Unhandled exception|Uncaught exception" "$err" && { bad "kitchen_sink errors on stderr"; head "$err"; } || pass "kitchen_sink: no crash output"
[ $fail -eq 0 ] && echo PASS
exit $fail
'
