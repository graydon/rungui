#!/bin/bash
# Build libobjc2 + GNUstep (make, base, gui, back/cairo) with clang into a private prefix, so that the
# objc2-based Cocoa backend can run under GNUstep on linux ("emulate-mac"). objc2 supports GNUstep only
# through libobjc2 (Debian only packages GCC's libobjc, which has no objc_msgSend/objc_msg_lookup_sender
# of the form objc2 needs), and GNUstep itself must be built against the same runtime.
#
# Verified on aarch64 Debian trixie (clang 19, ~6 minutes with -j8): libobjc2 master, tools-make 2.9.3,
# libs-base 1.31.1, libs-gui 0.32.0, libs-back 0.32.0 (cairo). Needs cmake, clang, git, and the -dev
# packages that Debian's libgnustep-base-dev / libgnustep-gui-dev pull in (libffi, libxml2, libicu,
# gnutls, libtiff, libpng, libjpeg, libcairo2, libx11 ...).
#
# Usage: build-gnustep-libobjc2.sh [PREFIX]      (default: $PWD/gnustep-libobjc2)
# Afterwards:  export RUNGUI_GNUSTEP_PREFIX=PREFIX  and see run-smoke.sh
set -euo pipefail
P=$(realpath -m "${1:-$PWD/gnustep-libobjc2}")
SRC=$P/src
mkdir -p "$P/lib" "$P/include" "$SRC"
export CC=clang CXX=clang++ OBJC=clang

# ---- libobjc2 (installs with "LOCAL" layout regardless of CMAKE_INSTALL_PREFIX: stage via DESTDIR)
[ -d "$SRC/libobjc2" ] || git clone --depth 1 --recurse-submodules https://github.com/gnustep/libobjc2 "$SRC/libobjc2"
mkdir -p "$SRC/libobjc2/build"
(cd "$SRC/libobjc2/build" &&
  cmake .. -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ -DCMAKE_ASM_COMPILER=clang \
        -DCMAKE_BUILD_TYPE=Release -DTESTS=OFF &&
  make -j"$(nproc)" &&
  make install DESTDIR="$SRC/stage")
cp -a "$SRC"/stage/usr/local/lib/libobjc.so* "$P/lib/"
rm -rf "$P/include/objc"
cp -a "$SRC/stage/usr/local/include/GNUstep/objc" "$P/include/objc"
cp "$SRC"/stage/usr/local/include/GNUstep/Block*.h "$P/include/"

# ---- GNUstep against it
export CPPFLAGS="-I$P/include" LDFLAGS="-L$P/lib -Wl,-rpath,$P/lib" LD_LIBRARY_PATH="$P/lib"
clone() { [ -d "$SRC/$1" ] || git clone --depth 1 --branch "$2" "https://github.com/gnustep/$1" "$SRC/$1"; }
clone tools-make make-2_9_3
clone libs-base base-1_31_1
clone libs-gui gui-0_32_0
clone libs-back back-0_32_0
if [ ! -f "$P/GS/share/GNUstep/Makefiles/GNUstep.sh" ]; then
  (cd "$SRC/tools-make" &&
    ./configure --prefix="$P/GS" --with-layout=fhs-system --with-library-combo=ng-gnu-gnu \
      --disable-importing-config-file --with-config-file="$P/GS/GNUstep.conf" &&
    make -j"$(nproc)" install)
fi
# shellcheck disable=SC1091
set +u # GNUstep.sh uses unset variables
. "$P/GS/share/GNUstep/Makefiles/GNUstep.sh"
set -u
(cd "$SRC/libs-base" && ./configure && make -j"$(nproc)" && make install)
(cd "$SRC/libs-gui" && ./configure && make -j"$(nproc)" && make install)
(cd "$SRC/libs-back" && ./configure --enable-graphics=cairo --with-name=cairo && make -j"$(nproc)" && make install)
# GNUstep looks for libgnustep-back-<ver>.bundle by default
(cd "$P/GS/local/lib/GNUstep/Bundles" && ln -sf libgnustep-cairo-032.bundle libgnustep-back-032.bundle)
echo "done: export RUNGUI_GNUSTEP_PREFIX=$P"
