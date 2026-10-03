#!/usr/bin/env bash
# Drives examples/file_manager under Xvfb with xdotool: navigation, preview, sorting, copy, rename,
# new folder, delete, hidden files; verifies results on disk and in the RUNGUI_FM_TRACE output and
# leaves screenshots in $SHOTS (default: $CARGO_TARGET_DIR/fm-shots).
#
#   scripts/smoke-filemanager.sh                 GTK (the real test)
#   BACKEND=wine scripts/smoke-filemanager.sh    win32 exe under wine (informational)
#   BACKEND=gnustep scripts/smoke-filemanager.sh emulate-mac build (informational)
# Honours CARGO_TARGET_DIR. Non-GTK backends only check start-up, a screenshot and a clean quit.
set -u
cd "$(dirname "$0")/.."
T="${CARGO_TARGET_DIR:-target}"
BACKEND="${BACKEND:-gtk}"
SHOTS="${SHOTS:-$T/fm-shots}"
mkdir -p "$SHOTS"
case "$BACKEND" in
  gtk) cargo build --example file_manager || exit 1; cmd="$T/debug/examples/file_manager" ;;
  gnustep) cargo build --features emulate-mac --example file_manager || exit 1; cmd="$T/debug/examples/file_manager" ;;
  wine) cargo build --target x86_64-pc-windows-gnu --example file_manager || exit 1
        export WINEPREFIX="${WINEPREFIX:-$T/wine-prefix}" WINEDEBUG=-all
        cmd="wine $T/x86_64-pc-windows-gnu/debug/examples/file_manager.exe" ;;
  *) echo "unknown BACKEND $BACKEND"; exit 2 ;;
esac
export BACKEND SHOTS cmd
exec xvfb-run -a -s "-screen 0 1280x800x24" bash -c '
set -u
export LANG=C.UTF-8 LC_ALL=C.UTF-8 NO_AT_BRIDGE=1 RUNGUI_FM_TRACE=1
root=$(mktemp -d); out="$root/trace.txt"
A="$root/a"; B="$root/b"
mkdir -p "$A/docs" "$A/Ünïcode dir" "$B"
printf "hello from alpha\nsecond line\n" > "$A/alpha.txt"
head -c 600 /dev/urandom > "$A/blob.bin"
printf "inner\n" > "$A/docs/inner.txt"
printf "unicode\n" > "$A/Ünïcode dir/日本語.txt"
printf "hidden\n" > "$A/.hidden"
printf "P6\n8 8\n255\n" > "$A/pic.ppm"; head -c 192 /dev/zero | tr "\0" "\200" >> "$A/pic.ppm"
printf "readme\n" > "$A/readme.txt"
ln -s docs "$A/linkdir"; ln -s nowhere "$A/dangling"
$cmd "$A" "$B" > "$out" 2>"$out.err" &
pid=$!
for i in $(seq 100); do grep -q "^READY" "$out" 2>/dev/null && break; sleep 0.2; done
fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
grep -q "^READY" "$out" || { bad "no READY"; cat "$out" "$out.err"; [ -n "$pid" ] && kill $pid; exit 1; }
shot() { import -window root "$SHOTS/fm-$BACKEND-$1.png" 2>/dev/null; }
if [ "$BACKEND" != gtk ]; then
  sleep 1; shot start
  xdotool key ctrl+q; sleep 2
  if kill -0 $pid 2>/dev/null; then echo "INFO: still running after ctrl+q"; kill $pid; else echo "INFO: exited cleanly"; fi
  grep -E "^(NAV|BOUNDS|READY)" "$out" | head -20; cat "$out.err" | head
  exit 0
fi
w=$(xdotool search --name "File Manager" | head -1)
eval "$(xdotool getwindowgeometry --shell $w)"; WX=$X; WY=$Y
xdotool windowfocus $w; sleep 0.3
# client area origin: the menu bar sits above the layout origin
ch=$(grep RESIZE "$out" | tail -1 | awk "{print \$3}"); MB=$((HEIGHT-ch)); [ $MB -ge 0 ] || MB=0
bounds() { awk -v n="$1" "\$1==\"BOUNDS\"&&\$2==n{print \$3,\$4,\$5,\$6}" "$out" | tail -1; }
click() { read -r x y bw bh < <(bounds "$1"); xdotool mousemove $((WX+x+${2:-bw/2})) $((WY+MB+y+${3:-bh/2})) click ${4:-1}; sleep 0.4; }
# click row N (0 = "..") of table A or B
row() { read -r x y bw bh < <(bounds "table$1"); xdotool mousemove $((WX+x+60)) $((WY+MB+y+25+$2*23+11)) click 1; sleep 0.4; }
expect() { for i in $(seq 25); do grep -Eq -- "$1" "$out" && { ok "$2"; return; }; sleep 0.2; done; bad "$2 (wanted: $1)"; }
alive() { kill -0 $pid 2>/dev/null || { bad "died during $1"; cat "$out.err"; exit 1; }; }
dlgwin() { for i in $(seq 25); do d=$(xdotool search --name "^$1\$" | head -1); [ -n "$d" ] && break; sleep 0.2; done; echo "$d"; }
# type into the open dialog and press OK (dialog bounds are relative to the dialog window)
dialog() {
  d=$(dlgwin "$1"); [ -n "$d" ] || { bad "dialog $1 did not open"; return; }
  sleep 0.6; xdotool windowfocus $d; sleep 0.3
  xdotool key ctrl+a; xdotool type --delay 20 "$2"; sleep 0.3
  shot "dialog-$1"
  read -r x y bw bh < <(awk "\$1==\"BOUNDS\"&&\$2==\"dlg_ok\"{print \$3,\$4,\$5,\$6}" "$out" | tail -1)
  eval "$(xdotool getwindowgeometry --shell $d)"
  xdotool mousemove $((X+x+bw/2)) $((Y+y+bh/2)) click 1; sleep 0.6
  xdotool windowfocus $w; sleep 0.3
}

shot start
grep -q "^NAV A $A " "$out" && ok "pane A shows start dir" || bad "pane A start"
grep -q "^NAV B $B " "$out" && ok "pane B shows start dir" || bad "pane B start"
grep -q "^BOUNDS tableA" "$out" && ok "bounds traced" || bad "bounds"

# select rows with the keyboard: order is .. docs linkdir Ünïcode dir | alpha blob dangling pic readme
row A 0; expect "^SELECT A \.\." "select .. row"
xdotool key Down; expect "^SELECT A docs" "Down selects docs"; expect "^PREVIEW folder .* docs" "folder preview"
row A 4; expect "^SELECT A alpha.txt" "select alpha.txt"; expect "^PREVIEW text 29 alpha.txt" "text preview of 29 bytes"
xdotool key Down; expect "^SELECT A blob.bin" "select blob.bin"; expect "^PREVIEW binary [0-9]+ blob.bin" "hex preview of binary"
row A 7; expect "^SELECT A pic.ppm" "select ppm"; expect "^PREVIEW image" "image preview"
shot preview-image

# sort by Size (click the header), twice
read -r x y bw bh < <(bounds tableA)
xdotool mousemove $((WX+x+125+31)) $((WY+MB+y+12)) click 1; sleep 0.5; expect "^SORT A 1 asc" "sort by size ascending"
xdotool mousemove $((WX+x+125+31)) $((WY+MB+y+12)) click 1; sleep 0.5; expect "^SORT A 1 desc" "sort by size descending"
xdotool mousemove $((WX+x+60)) $((WY+MB+y+12)) click 1; sleep 0.5; expect "^SORT A 0 asc" "sort by name"

# open a folder by double click, then go back with Alt+Up
row A 1; xdotool click --repeat 2 --delay 80 1; sleep 0.6
expect "^NAV A $A/docs " "double click opens docs"
xdotool key alt+Up; sleep 0.6; expect "^NAV A $A " "Alt+Up goes to parent"

# copy alpha.txt to pane B with F5
row A 4; expect "^SELECT A alpha.txt" "select before copy"; xdotool key F5; sleep 1
[ "$(cat "$B/alpha.txt" 2>/dev/null)" = "hello from alpha
second line" ] && ok "F5 copied alpha.txt to B" || bad "F5 copy"
[ -e "$A/alpha.txt" ] && ok "source kept after copy" || bad "source missing after copy"

# rename (F2) readme.txt -> notes ünï.txt
row A 8; expect "^SELECT A readme.txt" "select readme"
xdotool key F2; dialog Rename "notes ünï.txt"
expect "^RENAME readme.txt" "rename traced"
[ -e "$A/notes ünï.txt" ] && [ ! -e "$A/readme.txt" ] && ok "rename on disk" || bad "rename on disk"

# new folder (F7)
xdotool key F7; dialog "New Folder" "Fresh"
[ -d "$A/Fresh" ] && ok "F7 created folder" || bad "F7 mkdir"

# move (F6) the new folder to B, then delete it there (F8)
expect "^STATUS .*Fresh \\(folder\\)" "new folder is selected"
xdotool key F6; sleep 1
[ -d "$B/Fresh" ] && [ ! -e "$A/Fresh" ] && ok "F6 moved folder" || bad "F6 move"
xdotool key F9; sleep 0.5; expect "^ACTIVE B" "F9 switches to pane B"
xdotool key F8; sleep 1; shot confirm
d=$(xdotool search --name "^Delete\$" | head -1); [ -n "$d" ] && ok "delete asks for confirmation" || bad "no confirmation dialog"
xdotool key Return; sleep 1
[ ! -e "$B/Fresh" ] && ok "F8 deleted (after confirming)" || { bad "delete on disk"; xdotool key Escape; }

# hidden files
xdotool key ctrl+h; expect "^HIDDEN true" "Ctrl+H shows hidden files"
sleep 0.5; shot hidden
xdotool key ctrl+h; expect "^HIDDEN false" "Ctrl+H hides them again"

# path box: type a directory
click path; xdotool key ctrl+a; xdotool type --delay 15 "$A/docs"; sleep 0.8
expect "^NAV A $A/docs " "typing a path navigates"
xdotool key alt+Up; sleep 0.5
alive "path"
shot final

xdotool key ctrl+q; for i in $(seq 20); do kill -0 $pid 2>/dev/null || break; sleep 0.2; done
if kill -0 $pid 2>/dev/null; then bad "did not exit on Ctrl+Q"; kill $pid; else ok "Ctrl+Q quits"; fi
grep -E "ERROR|panick" "$out" "$out.err" | head -5
echo "trace: $out"
[ $fail = 0 ] && rm -rf "$root"
exit $fail
'
