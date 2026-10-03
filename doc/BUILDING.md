# Building rungui

One crate, three native backends, selected by `build.rs` (which also emits all native
link directives; backend code needs no `#[link]` attributes).

| cfg            | backend                      | selected when                                              |
|----------------|------------------------------|------------------------------------------------------------|
| `rungui_gtk`     | GTK 3                        | linux / other unix (default)                               |
| `rungui_win32`   | Win32                        | `target_os = "windows"` (also `x86_64-pc-windows-gnu`)     |
| `rungui_cocoa`   | Cocoa / AppKit               | `target_os = "macos"`, or linux + feature `emulate-mac`    |
| `rungui_gnustep` | (modifier of `rungui_cocoa`)   | linux + `emulate-mac`: GNUstep instead of Apple frameworks |

Exactly one of `rungui_gtk` / `rungui_win32` / `rungui_cocoa` is set. Use these cfgs (not
`target_os`) in backend code, e.g. `#[cfg(rungui_gtk)] mod gtk;`. Use `#[cfg(rungui_gnustep)]`
for the few places where GNUstep differs from Apple's runtime.

Cargo features: `emulate-mac` (only one).

## Setup

`scripts/setup-devcontainer.sh` (idempotent; wired to `postCreateCommand`) installs GTK3
dev, AT-SPI, xvfb/xdotool/imagemagick, mingw-w64, clang/lld, GNUstep (base, gui, back),
gobjc/libobjc, wine64 (x86_64 hosts only), and the rustup targets
`x86_64-pc-windows-gnu`, `aarch64-apple-darwin`, `x86_64-apple-darwin`.

`scripts/check-all.sh` builds/checks every mode below. Set a private `CARGO_TARGET_DIR`
if other builds run concurrently.

## Mode 1: native hosted

* Linux: `cargo build` / `cargo test`. GUI tests headless: `xvfb-run -a cargo test`
  (needs a session bus only for AT-SPI; `dbus-run-session xvfb-run -a ...`).
* Windows: `cargo build` with the MSVC or gnu toolchain. macOS: `cargo build`.

## Mode 2: win32 from linux (mingw-w64)

    cargo build --target x86_64-pc-windows-gnu      # linker set in .cargo/config.toml

Produces a real PE32+ executable, verified to link user32/gdi32/comctl32/ole32/etc.
Running it (x86_64 hosts only; on aarch64 wine cannot execute x86_64 PE files, so the backend is
compile+link verified there): `.cargo/config.toml` sets `scripts/wine-runner.sh` as the cargo
runner for this target, so these work on Linux:

    cargo run-win --example kitchen_sink    # alias for: cargo run --target x86_64-pc-windows-gnu ...
    cargo test-win                          # the test suite as a Windows exe under wine

The runner uses a private wine prefix under `$CARGO_TARGET_DIR` (never `~/.wine`), wraps the run in
`xvfb-run` when `$DISPLAY` is unset, and honours `WINEDEBUG`. `cargo run-mac` /
`cargo build-mac` are aliases for `--features emulate-mac` (Mode 3).

## Mode 3: mac from linux (decision: GNUstep emulation, plus real-target type-check)

What "build the mac backend on linux" can mean for a pure-Rust crate:

1. **Real target type-check** (always available): rustup has std for the darwin targets.
   `cargo check --target aarch64-apple-darwin` (or `x86_64-apple-darwin`) compiles the
   real `rungui_cocoa` code with real cfgs. It cannot *link* (no Apple SDK/frameworks), so
   it catches type/cfg errors only.
2. **GNUstep emulation, runnable** (decision): `cargo build --features emulate-mac` on
   linux compiles the same Cocoa backend (`rungui_cocoa` + `rungui_gnustep`) and links
   `libgnustep-gui`, `libgnustep-base`, `libobjc`. This was verified to actually run
   under Xvfb: creating `NSString`, `NSNumber`, `NSApplication`, and an `NSWindow` from
   Rust through hand-written externs works. GNUstep's AppKit API is close enough to
   Cocoa for the common widgets (NSWindow, NSButton, NSTextField, NSMenu, ...), though not
   identical; treat it as a smoke-test target, not a fidelity guarantee.
   No clang/ObjC compilation is involved: the backend is pure Rust calling the ObjC
   runtime C API (`objc_getClass`, `sel_registerName`, `objc_msgSend`...).
   (clang+GNUstep headers also work for C-side experiments:
   `clang -fobjc-runtime=gnustep-2.0` needs libobjc2, which Debian does not package; the
   packaged runtime is GCC's libobjc, hence the points below.)

### Runtime differences the Cocoa backend must handle (`#[cfg(rungui_gnustep)]`)

* **No `objc_msgSend`** in GCC libobjc. Use `objc_msg_lookup(receiver, sel) -> IMP`,
  transmute the IMP to the right `extern "C" fn(Id, Sel, ...) -> R` and call it. On real
  macOS use `objc_msgSend` (cast the same way). On x86_64 macOS, struct returns larger
  than 16 bytes (e.g. `NSRect`) need `objc_msgSend_stret`; not on aarch64; not needed
  with `objc_msg_lookup`. A single `msg_send` helper hides this (one macro in the
  backend is justified).
* **Linking**: rustc links with `--as-needed`, and ObjC classes are found by name at
  runtime, so nothing references `libgnustep-base/gui` and the linker drops them (symptom:
  `objc_getClass` returns null, segfault). `src/link_keepalive.rs` references `NSLog` and
  `NSApplicationMain` to prevent this; keep `mod link_keepalive;` in `lib.rs`.
* **Downstream crates**: rustc only passes an rlib's native libs when the rlib is actually
  linked, so an app must really use something from `rungui` (normal usage does).
* **Exceptions**: ObjC exceptions (e.g. `NSApplication` without a display) abort the Rust
  process ("Rust cannot catch foreign exceptions"). Run under `xvfb-run -a`.
* **Autorelease**: create an `NSAutoreleasePool` (GNUstep warns otherwise); same on Cocoa.
* `NSWindow` style masks print harmless "Failed to determine offsets for style N" logs.
* Selector/class registration (`objc_allocateClassPair`, `class_addMethod`) exists in
  GCC libobjc's runtime API with the same names as Apple's; delegates/targets can be made
  the same way.

Run headless: `xvfb-run -a cargo run --features emulate-mac --example ...`.

## Verified in this environment (aarch64 Debian trixie)

| step                                              | result |
|---------------------------------------------------|--------|
| `cargo build` (GTK3, gtk_init_check + window under xvfb) | ok |
| `cargo build --features emulate-mac` (NSNumber/NSString/NSWindow via objc_msg_lookup) | ok, runs |
| `cargo build --target x86_64-pc-windows-gnu`      | ok, PE32+ |
| `cargo check --target {aarch64,x86_64}-apple-darwin` | ok |
| run Windows exe under wine                        | not possible on aarch64 host |

## Cocoa backend notes (`src/backend/cocoa.rs`)

Pure Rust over the Objective-C runtime C API (`objc_getClass`, `sel_registerName`,
`objc_allocateClassPair`, `class_addMethod`, `objc_msgSend` / `objc_msg_lookup`); no binding
crates, no `#[link]` (build.rs links AppKit/Foundation/CoreGraphics/objc, or GNUstep).

**Real macOS (native)**: on a Mac, `cargo build` / `cargo run --example kitchen_sink`
(x86_64 or aarch64, Xcode command line tools only; no extra crates). Run the binary from a
terminal or wrap it in an `.app` bundle for a proper Dock icon/menu name. The code is
type-checked for both real targets from linux (`cargo check --target aarch64-apple-darwin`,
`--target x86_64-apple-darwin`), including the x86_64-only `objc_msgSend_stret` path used for
`NSRect` returns. It has NOT been run on real macOS in this environment: treat the Apple path
as compile-verified only; the GNUstep run below is the behavioural test.

**Cross build from linux to macOS**: `cargo check` works as above. Linking needs the Apple
SDK frameworks (not redistributable here); use `osxcross` or `cargo-zigbuild` with a macOS SDK
(`cargo zigbuild --target aarch64-apple-darwin`) and set `SDKROOT`. Nothing in the crate
needs a C/ObjC compiler.

**GNUstep emulation, tested under Xvfb**: `cargo build --features emulate-mac --example
kitchen_sink && xvfb-run -a target/debug/examples/kitchen_sink`. Verified there (screenshots +
xdotool): window/menus/tabs/group box/labels/text fields/combo/spin/slider/progress/radio
exclusivity/check/list/table/tree/NSTimer/modal NSAlert. GNUstep differences handled in code:
no `objc_msgSend` (uses `objc_msg_lookup`), no sort-indicator images, scrollers drawn on the
left, menus as floating windows, `NSAlert` legacy return values.

Design notes: one runtime class `RunguiTarget` is target/delegate/data source of everything and
maps senders back to ids via a pointer table; `RunguiFlipView` (flipped NSView) is the container
for windows, group boxes and tab pages so the core's top-left coordinates work unchanged;
`RunguiApp` (NSApplication subclass) only overrides `sendEvent:` to turn right-clicks into
`Event::ContextMenu`. Every application gets the standard app menu (Quit, Cmd+Q) and an Edit menu
(first-responder Cut/Copy/Paste/Select All) unless the app defines its own "Edit" menu.
Accessibility uses AppKit's built-in NSAccessibility for native controls; `a11y_changed` copies
the core's computed names/descriptions into `accessibilityLabel`/`accessibilityHelp`
(no accesskit platform adapter needed on macOS).
