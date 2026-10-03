#!/usr/bin/env bash
# Drives examples/kitchen_sink under Xvfb: tabs, typing (unicode), menus, dialogs, resize, close.
# Fails on crash, GLib/Gtk warnings (G_DEBUG=fatal-warnings), or unclean exit. Usage: scripts/smoke-gtk.sh
set -u
cd "$(dirname "$0")/.."
cargo build --example kitchen_sink || exit 1
exe="${CARGO_TARGET_DIR:-target}/debug/examples/kitchen_sink"
exec xvfb-run -a -s "-screen 0 1024x768x24" bash -c '
export LANG=C.UTF-8 LC_ALL=C.UTF-8 G_DEBUG=fatal-warnings NO_AT_BRIDGE=1
exe="$1"; err=$(mktemp); fail=0
"$exe" 2>"$err" & pid=$!
for i in $(seq 50); do w=$(xdotool search --name "^rungui kitchen sink$" | head -1); [ -n "$w" ] && break; sleep 0.2; done
[ -n "$w" ] || { echo "FAIL: no window"; cat "$err"; exit 1; }
sleep 0.5
eval "$(xdotool getwindowgeometry --shell $w)"
alive() { kill -0 $pid 2>/dev/null || { echo "FAIL: died during $1"; cat "$err"; exit 1; }; }
clickat() { xdotool mousemove $((X+$1)) $((Y+$2)) click ${3:-1}; sleep 0.25; alive "click $1,$2"; }
shot() { import -window root "/tmp/smoke-gtk-$1.png" 2>/dev/null; }
xdotool windowfocus $w
# Form page: type into Name, unicode
clickat 150 112; xdotool type --delay 20 "héllo 日本語 😀 עברית"; sleep 0.3; alive typing
xdotool key Tab Tab Down; sleep 0.2
shot form
xdotool key ctrl+a BackSpace; sleep 0.2
# tabs: click each tab header (approx positions), then keyboard nav
for tx in 47 121 197 264; do clickat $tx 76; xdotool key Down Up Tab shift+Tab; sleep 0.2; alive "tab $tx"; done
# Notes page: multi-line unicode text, selection, deletion
clickat 197 76; clickat 250 200; xdotool type --delay 10 "line one ☃"; xdotool key Return; xdotool type --delay 10 "line two 日本"; xdotool key Return; xdotool key ctrl+a Delete ctrl+z; sleep 0.2; alive textarea
# Form page again: password, spin keys, slider keys
clickat 47 76; clickat 250 162; xdotool type "s3crét"; clickat 250 242; xdotool key Up Up Down; clickat 250 282; xdotool key Right Right Home End; sleep 0.2; alive formkeys
# Data page: right click in table & tree, open popup, escape
shot data
clickat 100 166 3; sleep 0.4; xdotool key Escape; sleep 0.2; alive ctxmenu
clickat 380 164 3; sleep 0.4; xdotool key Down Return; sleep 0.2; alive ctxmenu2
clickat 100 166; xdotool key Down Up Return; sleep 0.2; alive tablekeys
clickat 279 187; sleep 0.3; xdotool key Down Right Left plus minus; sleep 0.3; clickat 279 187; alive tree
# column header click (sort) several times
clickat 60 118; clickat 60 118; clickat 200 118; alive sort
# menus + file dialog
xdotool key alt+f; sleep 0.3; xdotool key Escape; sleep 0.2
# (typing "/" trips a GTK-internal critical in the chooser completion popup under Xvfb, so avoid it)
xdotool key ctrl+o; sleep 1; alive filedlg
d=$(xdotool search --name "^Open$" | head -1)
[ -n "$d" ] || { echo "FAIL: file dialog did not appear"; fail=1; }
xdotool type "x.txt"; sleep 0.2
for n in 1 2 3; do xdotool search --name "^Open$" >/dev/null || break; xdotool key Escape; sleep 0.4; done
alive filedlg2
xdotool search --name "^Open$" >/dev/null && { echo "FAIL: file dialog did not close"; fail=1; xdotool windowunmap $d; }
# About dialog, dismissed with Enter
clickat 385 359; sleep 0.7; xdotool key Return; sleep 0.5; alive about
shot data2
# resize stress
for s in "300 250" "900 700" "480 400" "200 150" "700 300"; do xdotool windowsize $w $s; sleep 0.3; alive resize; done
shot resized
# close via the Close button (window may have been resized; re-place it first)
xdotool windowsize $w 480 400; sleep 0.4
xdotool mousemove $((X+463)) $((Y+359)) click 1; sleep 1
if kill -0 $pid 2>/dev/null; then echo "FAIL: still running after Close"; fail=1; kill $pid; fi
wait $pid; rc=$?
echo "exit code $rc"; cat "$err"
[ $rc -eq 0 ] || [ $rc -eq 143 ] || { echo "FAIL exit"; fail=1; }
grep -Eq "CRITICAL|WARNING|panicked|Segmentation" "$err" && { echo "FAIL: warnings in stderr"; fail=1; }
[ $fail -eq 0 ] && echo "PASS"
exit $fail
' _ "$exe"
