#!/usr/bin/env bash
# Idempotent dev environment setup (Debian trixie devcontainer, x86_64 or aarch64).
# Run as the normal user (uses sudo). Everything installed is listed below.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
SUDO=""; [ "$(id -u)" -ne 0 ] && SUDO="sudo -E"

PKGS=(
  # build basics
  pkg-config clang lld llvm
  # linux backend: GTK3 (+ AT-SPI headers for accesskit/atspi debugging)
  libgtk-3-dev libatspi2.0-dev at-spi2-core dbus-x11
  # headless GUI tests
  xvfb xauth xdotool x11-utils imagemagick
  # win32 emulation: mingw-w64 cross toolchain (x86_64-pc-windows-gnu)
  gcc-mingw-w64-x86-64 binutils-mingw-w64-x86-64
  # mac emulation: GNUstep base+gui(+back) headers/libs, GCC libobjc runtime, gobjc
  gnustep-devel libgnustep-gui-dev libobjc-14-dev gobjc
)
# wine can only run x86_64 PE binaries on an x86_64 host (not on aarch64).
[ "$(uname -m)" = x86_64 ] && PKGS+=(wine wine64)

$SUDO apt-get update -qq
$SUDO apt-get install -y --no-install-recommends "${PKGS[@]}"

if command -v rustup >/dev/null; then
  rustup target add x86_64-pc-windows-gnu aarch64-apple-darwin x86_64-apple-darwin
fi
echo "rungui dev environment ready ($(uname -m))"
