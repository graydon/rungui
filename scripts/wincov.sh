#!/usr/bin/env bash
# Line coverage of the Win32 backend, measured under wine (LLVM's source-based coverage has no
# runtime for x86_64-pc-windows-gnu). Builds with block-level sanitizer coverage and a tiny runtime
# (scripts/wincov/cov.c) that records the addresses reached and writes them to $RUNGUI_COV_OUT at
# exit; scripts/wincov/report.py turns them into per-file line coverage.
#
#   scripts/wincov.sh build [cargo args...]      build everything with coverage into $COV_TARGET
#   scripts/wincov.sh run <exe> [args...]        run one built exe under wine + Xvfb, appending to its hits file
#   scripts/wincov.sh report [--list] [--src substr]
#   scripts/wincov.sh all                        build, run the tests, drivers and smoke scripts' exes, report
#
# Needs nightly (sanitizer-coverage flags), the x86_64-pc-windows-gnu target, mingw-w64 and wine.
set -eu
cd "$(dirname "$0")/.."
COV_TARGET="${COV_TARGET:-${CARGO_TARGET_DIR:-$PWD/target}/wincov}"
HITS="$COV_TARGET/hits"   # one file per executable: addresses differ between them
OBJ="$COV_TARGET/cov.o"
triple=x86_64-pc-windows-gnu
flags="-C passes=sancov-module -C llvm-args=-sanitizer-coverage-level=2 -C llvm-args=-sanitizer-coverage-trace-pc -C link-arg=$OBJ -C link-arg=-lkernel32"
dbg="$COV_TARGET/$triple/debug"
mkdir -p "$COV_TARGET"
wine_env() {
  export WINEPREFIX="$COV_TARGET/wineprefix" WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml=" LANG=C.UTF-8
  export RUNGUI_COV_OUT="Z:$HITS/$(basename "$1" .exe).txt" RUNGUI_FUZZ_ASCII_TEXT=1
}
build() {
  x86_64-w64-mingw32-gcc -O2 -c scripts/wincov/cov.c -o "$OBJ"
  export CARGO_TARGET_DIR="$COV_TARGET"
  RUSTFLAGS="$flags" cargo +nightly build --target $triple --examples "$@"
  RUSTFLAGS="$flags" cargo +nightly test --target $triple --no-run "$@"
  RUSTFLAGS="$flags" cargo +nightly build --manifest-path fuzz/Cargo.toml --target $triple --bins "$@"
}
run() {
  mkdir -p "$HITS"
  wine_env "$1"
  xvfb-run -a -s "-screen 0 1280x1024x24" wine "$@" < /dev/null
}
report() {
  python3 scripts/wincov/report.py "$HITS" "$@"
}
case "${1:-}" in
  build) shift; build "$@" ;;
  run) shift; run "$@" ;;
  report) shift; report "$@" ;;
  all)
    build
    rm -rf "$HITS"
    for t in $(find "$dbg" -name '*.exe' \( -name 'rungui-*' -o -name 'win32_native-*' -o -name 'file_manager-*' \) | grep '/out/'); do run "$t"; done
    run "$dbg/native.exe" 400 1
    run "$dbg/soak.exe" 300
    run "$dbg/bench_native.exe" 1
    report --src src/backend/win32
    ;;
  *) sed -n '2,12p' "$0"; exit 2 ;;
esac
