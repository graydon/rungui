# rungui — Rust Unified Native GUI

A small portable desktop GUI library for Rust: a thin layer over GTK3 (Linux), Win32 (Windows)
and AppKit (macOS). The platform C APIs are declared by hand (no binding crates, no
`gtk-rs`/`winapi`). Text is UTF-8 in Rust and converted at the boundary. The aim is
FLTK/libui/IUP-level simplicity: no user-facing traits, handles are `Copy` ids, a stale handle is
inert instead of a panic, and `native_handle()` gives you the real `GtkWidget*` / `HWND` /
`NSView*` when the portable API is not enough.

## LLM notice

This repository is written by an LLM.

If LLM code is a no-go for you, close the tab and move on.

I have lightly skimmed it to make sure it's roughly doing what I expect. I have not reviewed
every line, by any means. There are probably bugs present. On the other hand I have been
waiting for someone (including myself) to have time or inclination to write this crate
for over ten years, and Sonnet 5.5 wrote it in 3 hours this evening while I was doing
chores around the house, so .. I feel like the bugs are a risk I'm willing to take.

## Quickstart

```rust
use rungui::*;

fn main() {
    let app = App::new("hello").expect("init");
    let win = Window::new("Hello, rungui");
    let col = VBox::new(win);
    let label = Label::new(col, "Hello, wörld!");
    let name = TextInput::new(col);
    let button = Button::new(col, "Greet");
    button.on_click(move || label.set_text(&format!("Hello, {}!", name.text())));
    win.show();
    app.run();
}
```

```
cargo run --example hello          # Linux needs libgtk-3-dev
cargo run --example kitchen_sink   # every widget
cargo run --example file_manager   # dual-pane file manager
cargo run-mac --example kitchen_sink   # Cocoa backend on GNUstep (Linux)
cargo run-win --example kitchen_sink   # Win32 backend under wine (x86_64 Linux)
cargo doc --open                   # tutorial + API
```

Containers: `VBox`, `HBox`, `Grid`, `GroupBox`, `Tabs`/`Page`, `Splitter` (two panes with a
draggable sash; layout is core-driven). Widgets: `Label`, `Button`,
`CheckBox`, `RadioButton`, `TextInput`, `TextArea`, `ComboBox`, `ListBox`, `Slider`, `SpinBox`,
`ProgressBar`, `Image`, `Table`, `Tree`, menus (incl. popup), `message_box`, `FileDialog`, `Timer`.
Text widgets can be monospace (`set_monospace`) and a `TextArea` can turn off soft wrapping
(`set_wrap(false)`); windows have `set_position` / `position` and `set_min_size`.
Cross-thread: only `App::post` / `App::quit`.

## It runs the same program on three native toolkits

`examples/file_manager` is a dual-pane file manager (folder tree, two sortable file tables,
text / hex / image preview, menus with accelerators, context menu, status bar, copy / move /
rename / delete). It is about 2k lines of ordinary Rust against rungui's public API, with no
backend-specific code. These are release builds browsing this repository, taken on a Linux
machine under Xvfb:

**GTK3 (Linux)**

![file manager on GTK3](doc/screenshots/file-manager-gtk.png)

**Win32 (the Windows exe, run under Wine)**

![file manager on Win32 under Wine](doc/screenshots/file-manager-win32-wine.png)

**AppKit (Cocoa backend, run on macOS)**

![file manager on the Cocoa backend under macOS](doc/screenshots/file-manager-macos.png)

**AppKit (Cocoa backend, run on GNUstep)**

![file manager on the Cocoa backend under GNUstep](doc/screenshots/file-manager-gnustep.png)

The Win32 and GNUstep shots are emulated environments: no window manager (so no title bars), and
GNUstep's look and its detached menu at the top left. The Win32 backend has not yet been run on
real Windows; see [`doc/STATUS.md`](doc/STATUS.md). 

## Dependencies

**Rust crates.** Two on Linux and macOS: `accesskit` 0.25 (the accessibility tree) and `uuid`. On Windows only, `accesskit_windows` 0.35 and the `windows`
0.62 crate (`Foundation` feature) for the UI Automation adapter: 29 crates in total there,
mostly the `windows-*` support crates and proc-macros. The platform C APIs themselves are
declared by hand in the crate; there are no binding crates (`gtk-rs`, `winapi`, `objc2`).
Needs Rust 1.85+ (edition 2024); built and tested with 1.94.

**Native libraries.** What gets linked is decided in `build.rs`:

| Target | Links against | Install |
|---|---|---|
| Linux (GTK3) | libgtk-3, gdk-3, gdk_pixbuf, pango, cairo, atk, gio, gobject, glib | `apt install libgtk-3-dev pkg-config` |
| Windows | user32, gdi32, kernel32, comctl32, comdlg32, shell32, ole32, uxtheme, dwmapi, shcore, oleacc, uiautomationcore, imm32 (all part of Windows) | nothing |
| macOS | AppKit, Foundation, CoreGraphics, libobjc (all part of macOS) | Xcode command line tools |
| Linux, `--features emulate-mac` | libgnustep-gui, libgnustep-base, libobjc (GCC runtime) | `apt install gnustep-devel libgnustep-gui-dev libobjc-14-dev gobjc clang` |

**Only for testing and cross-building on Linux** (all installed by `scripts/setup-devcontainer.sh`):
`xvfb xauth xdotool x11-utils imagemagick` (headless GUI smoke tests and screenshots),
`gcc-mingw-w64-x86-64 binutils-mingw-w64-x86-64` plus the rustup target `x86_64-pc-windows-gnu`
(Windows builds), `wine wine64` (x86_64 hosts only; runs them), and the rustup targets
`aarch64-apple-darwin` and `x86_64-apple-darwin` (type-checking the real macOS backend).

## Binary size and linkage

Plain `cargo build --release` with the default profile, x86_64.

| Binary | GTK3 built | GTK3 stripped | Cocoa/macOS built | Cocoa/macOS stripped | Cocoa/GNUstep built | Cocoa/GNUstep stripped | Win32 built | Win32 stripped |
|---|---|---|---|---|---|---|---|---|
| `hello` (a window, a label, a button) | 756 KiB | **601 KiB** | 803 KiB | **663 KiB** | 873 KiB | **711 KiB** | 2,436 KiB | **1,764 KiB** |
| `file_manager` (the whole app above) | 1,092 KiB | **862 KiB** | 1,087 KiB | **884 KiB** | 1,173 KiB | **939 KiB** | 2,662 KiB | **1,926 KiB** |

Direct dynamic dependencies, the part the program itself asks for:

* **GTK3:** `libgtk-3 libgdk-3 libgdk_pixbuf-2.0 libatk-1.0 libgobject-2.0 libglib-2.0 libgcc_s libc`
* **Cocoa/macOS:** `AppKit.framework Foundation.framework CoreGraphics.framework libobjc libSystem`
* **Cocoa/GNUstep:** `libgnustep-gui libgnustep-base libobjc libgcc_s libc`
* **Win32** (import table, `objdump -p`; Windows has no `ldd`): `api-ms-win-core-synch-l1-2-0.dll`, `api-ms-win-core-winrt-error-l1-1-0.dll`, `bcryptprimitives.dll`, `combase.dll`, `gdi32.dll`, `kernel32.dll`, `KERNEL32.dll`, `msimg32.dll`, `msvcrt.dll`, `ntdll.dll`, `ole32.dll`, `oleaut32.dll`, `propsys.dll`, `rpcrt4.dll`, `SHELL32.dll`, `uiautomationcore.dll`, `user32.dll`, `USERENV.dll`, `UxTheme.dll`, `WS2_32.dll`

The Win32 exe also contains the accesskit UI Automation adapter, which is most of why it is the
largest. 

## Platform status

* **Linux (GTK3):** complete and run-tested under Xvfb, including a dual-pane file manager example.
* **Windows (Win32) and macOS (AppKit):** written and cross-built; run under wine and GNUstep on
  Linux, never yet on real Windows. Manually tested on macOS. Bring-up is in progress.
* **Mock:** a headless backend (`--features mock`) that the 116-test suite runs against.

Per-widget, per-platform status, how each part was verified, known gaps and next steps are in
[`doc/STATUS.md`](doc/STATUS.md). `scripts/check-all.sh` builds and tests every mode; see
[`doc/BUILDING.md`](doc/BUILDING.md) and [`doc/DESIGN.md`](doc/DESIGN.md).

## Known limits

See "Known gaps" in [`doc/STATUS.md`](doc/STATUS.md).

## License

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.

## Contributing

I want to keep this thing very small and simple. Contributions are .. possible, but unlikely
to be accepted given the LLM-generated nature of the codebase in the first place. Maybe make a
suggestion, but we live in a time when the LLM obviously implements any idea faster than I
could review the same change proposed from outside. The time and attention economics of
LLM-driven development are different and I'm still trying to figure them out.

Feel free to fork and do whatever you want with it, of course.
