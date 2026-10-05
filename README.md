# rungui — Rust Unified Native GUI

## LLM notice

This repository is written by an LLM.

If LLM code is a no-go for you, close the tab and move on.

If there are bugs I'll fix them, but it's a small codebase bridging to stable
APIs and was synthesized in 3 hours with a cheap model; it really shouldn't
require much maintenance. It's just fussy code no human bothered to write.

## Overview

Rungui is a portable wrapper over 3 desktop GUI toolkits: Linux/GTK, macOS/AppKit
and Win32. It is intended as a simple 80/20 option in the sprawling landscape of
"GUIs for Rust".

Benefits:

  1. It's lightweight: 14kloc and no external dependencies, compiles in ~3
     seconds to a few hundred KiB of object code. All FFIs are locally declared.

  2. It gets a fair amount of the tricky stuff in GUIs -- eg. accessibility and
     text-rendering, tables and trees -- by delegating to the platform libraries.

  3. It doesn't have any complex traits or macros or preprocessors or anything.
     You just build a tree of nested objects and attach callbacks.

  4. It doesn't hide the platform libraries, you can call `native_handle()` to
     get a `GtkWidget*` / `HWND` / `NSView*` if you want to go further.

Drawbacks:

  1. You have to write your applications "the old fashioned way" with stateful
     UI object handles and callbacks, not "the new way" with FRP-style
     reactive/declarative UI or immediate mode or anything.

  2. There's some runtime overhead mapping the memory-safe `Copy` integer IDs
     used as object handles to native abstractions, and there's some imprecision
     about lifetimes and validity contexts (eg. if you use such a handle on the
     wrong thread or after the object dies it just goes inert and does nothing).

  3. It doesn't do cutting-edge GPU rendering or cool visual effects or run on
     webassembly or anything flashy. Just a bunch of old standard widgets.

  4. While it is 100% Rust and the interface ought to be safe, of course the
     platform libraries are typically decades-old C code and so there are lots
     of `unsafe` blocks inside the implementation.


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

Containers: `VBox`, `HBox`, `Grid`, `GroupBox`, `Tabs`/`Page`, `Splitter`.

Widgets: `Label`, `Button`, `CheckBox`, `RadioButton`, `TextInput`, `TextArea`, `ComboBox`, `ListBox`, `Slider`, `SpinBox`, `ProgressBar`, `Image`, `Table`, `Tree`, `Menu`, `MenuBar`, `MenuItem`, `PopupMenu`, `message_box`, `FileDialog`, `Timer`.

## Example screenshots

`examples/file_manager` is a dual-pane file manager (folder tree, two sortable file tables,
text / hex / image preview, menus with accelerators, context menu, status bar, copy / move /
rename / delete). It is about 2k lines of ordinary Rust against rungui's public API, with no
backend-specific code.

**GTK3 (Linux)**

![file manager on GTK3](doc/screenshots/file-manager-gtk.png)

**Win32 (the Windows exe, run under Wine)**

![file manager on Win32 under Wine](doc/screenshots/file-manager-win32-wine.png)

**AppKit (Cocoa backend, run on macOS)**

![file manager on the Cocoa backend under macOS](doc/screenshots/file-manager-macos.png)

**AppKit (Cocoa backend, run on GNUstep)**

![file manager on the Cocoa backend under GNUstep](doc/screenshots/file-manager-gnustep.png)

 The Win32 backend has not yet been run on real Windows, only Wine.
 See [`doc/STATUS.md`](doc/STATUS.md). 

## Dependencies

**Rust crates.** None:

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

## Binary sizes

Plain `cargo build --release`:

| Binary | GTK3 built | GTK3 stripped | Cocoa/macOS built | Cocoa/macOS stripped | Cocoa/GNUstep built | Cocoa/GNUstep stripped | Win32 built | Win32 stripped |
|---|---|---|---|---|---|---|---|---|
| `hello` | 756 KiB | **601 KiB** | 803 KiB | **663 KiB** | 873 KiB | **711 KiB** | 1,738 KiB | **1,266 KiB** |
| `file_manager` | 1,092 KiB | **862 KiB** | 1,087 KiB | **884 KiB** | 1,173 KiB | **939 KiB** | 1,964 KiB | **1,427 KiB** |

## Platform status

* **Linux (GTK3):** complete and run-tested under Xvfb.
* **Windows (Win32) and macOS (AppKit):** written and cross-built; run under wine and GNUstep on
  Linux, never yet on real Windows. Manually tested on macOS.
* **Mock:** a headless backend (`--features mock`) for testingt.

Per-widget, per-platform status is in
[`doc/STATUS.md`](doc/STATUS.md). `scripts/check-all.sh` builds and tests every mode; see
[`doc/BUILDING.md`](doc/BUILDING.md) and [`doc/DESIGN.md`](doc/DESIGN.md).

## Known limits

See "Known gaps" in [`doc/STATUS.md`](doc/STATUS.md).

## License

ASL2/MIT at your option

## Contributing

I want to keep this thing very small and simple. Bug reports or fixes welcome. I
intend to shake bugs out of it until it feels stable and then call it 1.0 and
mostly leave it be. Beyond that, if you have big ambitions probably best to fork.