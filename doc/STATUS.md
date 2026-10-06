# rungui status

**This is the single place project status is recorded.** README.md carries a three-line summary
and links here; `doc/DESIGN.md` describes the architecture and `doc/BUILDING.md` the build
modes, neither tracks progress. Update this file whenever a backend, test count or gap changes.

Last updated: 2026-10-05, after the quality pass (fuzzing, soak, coverage, API narrowing).

## Against INITIAL_PROMPT.md

| Requirement | State |
|---|---|
| win32 / macOS / linux backends | GTK3, Win32 and AppKit backends all exist, with hand-written FFI. Only GTK is verified at runtime on its real platform. |
| Hosted and emulated modes | `--features emulate-mac` (clang + GNUstep), `--features mock` (headless), Win32 via mingw-w64 and, on x86_64 Linux, run under wine (`cargo run-win`, `cargo test-win`). |
| Cross-compile | `x86_64-pc-windows-gnu` builds and links. `*-apple-darwin` is type-checked only. |
| Devcontainer scaffolding | `.devcontainer/`, `scripts/setup-devcontainer.sh`, `scripts/check-all.sh`, `scripts/wine-runner.sh`, `.cargo/config.toml` aliases. |
| a11y | Native controls are exposed by each platform; the core computes names/descriptions/roles (`a11y::resolve`, tested on mock) and each backend copies them onto the native controls. Win32 relies on the stock MSAA proxies plus `IAccPropServices` overrides (`win32/a11y.rs`, checked with MSAA and UIA clients; no UIA provider). GTK sets ATK name/description and (unverified) role overrides. macOS sets AppKit labels/help only. No accessibility crate is used. |
| Unicode | UTF-8 in Rust. UTF-16 on Win32, NSString on Cocoa, UTF-8 on GTK. Accents, CJK, emoji and Hebrew were typed on GTK in the smoke test. |
| Native handles | `native_handle()` returns the `GtkWidget*`, `HWND` or `NSView*`. |
| Simplicity and no panics | `Copy` id handles, stale handles are inert, borrows never held across callbacks. Fuzz, reference-model and re-entrancy tests run on the mock backend. |
| 100% Rust | Yes, apart from a small `build.rs` for linking. |

## Widget matrix

R = run-tested, C = compiles and links only, T = type-checked only, N = not implemented.
"R under wine" means run on Wine under Xvfb on Linux, not on real Windows.

| Widget | GTK | Win32 | Cocoa |
|---|---|---|---|
| Window, VBox, HBox, Grid, GroupBox, Tabs | R | R under wine | R on GNUstep |
| Label, Button, CheckBox, RadioButton | R | R under wine | R on GNUstep |
| TextInput, password, TextArea | R | R under wine | R on GNUstep |
| ComboBox, ListBox, Slider, SpinBox, ProgressBar | R | R under wine | R on GNUstep |
| Image | R (limited testing; used by the file manager preview) | C | C |
| MenuBar, Menu, MenuItem, CheckMenuItem, accelerators | R | R under wine | R on GNUstep |
| PopupMenu | R | R under wine (Shift+F10/Apps key untested) | R on GNUstep (macOS path type-checked only) |
| Table | R | R under wine (first column always left-aligned: comctl32) | R on GNUstep |
| Tree | R | R under wine | R on GNUstep |
| Splitter (core layout, panes) | R | C | C |
| Splitter sash (drag handle, keyboard, `Kind::Sash`) | R (drag and keys verified under xdotool) | R under wine (drag, both orientations); keys C | R on GNUstep (drag and keys, both orientations, clamping) |
| TextArea/TextInput monospace, TextArea wrap | R | R under wine | R on GNUstep |
| Window `set_position`, `Moved` event | R | C (`set_position` seen under wine; `Moved` untestable without a WM) | R on GNUstep |
| Window min size | C (geometry hint; Xvfb has no WM) | C (`WM_GETMINMAXINFO`; no WM under wine) | R on GNUstep |
| Windows shrink below natural size | R | not checked | not checked |
| Message box, FileDialog | R | message box R under wine, FileDialog C | C |
| Timer, `App::post` | R | R under wine | C |

## How things are verified

- `cargo test` (also `--features mock`): 118 core tests against the mock backend (seeded random-layout
  fuzzing, Table/Tree reference models, a11y metadata, a re-entrancy matrix of every event kind
  against hostile callbacks, API-surface and limit tests), 11 file-manager unit tests, 15
  file-manager integration tests and 4 seeded random-operation tests on the mock backend (need
  `--features mock`), 2 doctests. Line coverage of the shared code is about 97% (`cargo +nightly
  llvm-cov --features mock`); the backends are covered by the scripts below (GTK about 88% of regions).
- `fuzz/` (see doc/BUILDING.md, "Quality tooling"): one byte-driven interpreter over the whole public
  API (hostile arguments, wrong-kind and dead handles, re-entrant and panicking callbacks) used by a
  libFuzzer target under AddressSanitizer (`cargo +nightly fuzz run api_mock --features mock`, 600k+
  executions clean), by the seeded `tests/random_ops.rs`, and by a native driver that runs it inside
  the real toolkit's loop (`scripts/fuzz-native.sh`: GTK with fatal GLib warnings, Cocoa on GNUstep).
  `SOAK=1 scripts/fuzz-native.sh` is a create/destroy leak soak that reports RSS and open fds.
- `examples/bench_mock.rs`: growth exponent of every hot path of the core (all linear or better
  except destroying siblings one at a time, which is O(siblings) per call).
- `scripts/smoke-gtk.sh`, `scripts/smoke-gtk-soak.sh`, `scripts/smoke-filemanager.sh`: GTK under Xvfb
  with xdotool and `G_DEBUG=fatal-warnings`; they check results on disk and via trace output.
- `scripts/smoke-win32.sh`: the Win32 backend under wine + Xvfb (skips if wine is missing); the same
  checks as `smoke-gtk.sh` plus sash drags, monospace/wrap, table/tree, popup menu.
- `cargo test-win` runs the test suite as a Windows exe under wine, always on a private Xvfb. That
  includes `tests/win32_native.rs`, which sends the notifications of real user actions (button
  clicks, list/table/tree selection, keys, accelerators, resizing, DPI changes, message boxes and file
  dialogs) to the real controls and checks what the application hears, and reads native state back.
- `BACKEND=wine scripts/fuzz-native.sh` (also `SOAK=1`) runs the random-operation driver and the leak
  soak against the Win32 backend under wine: 10,000+ seeds clean. The soak shows a slow RSS
  creep under wine (about 3 KiB per window with ~30 labels; the Rust heap and handle counts stay flat and
  a bare-Win32 loop does not creep) that was not traced further. Wine itself crashes in its text shaping
  on some random strings, so that driver feeds it ASCII-only random text.
- `scripts/wincov.sh all`: line coverage of the Win32 backend under wine (block-level sanitizer coverage;
  LLVM's own has no runtime for windows-gnu): about 82% of `win32.rs` from the tests, fuzz, soak and
  benchmark drivers; what is left is mostly creation-failure paths and the results of file dialogs.
- `scripts/smoke-gnustep.sh`: the Cocoa backend on GNUstep under Xvfb, 35 checks (sash
  drags and keys, typing incl. unicode, table sort, tree expand, popup menu, accelerators, move/resize, quit).
- `scripts/check-all.sh` builds every mode (GTK, mock, emulate-mac, Windows GNU, both Apple
  targets, release), runs clippy with `-D warnings` in every mode, the benchmark bound, the smoke
  scripts and the fuzz/soak drivers.
- `.github/workflows/ci.yml` runs Linux (full check), Windows and macOS (build, test, brief launch)
  jobs.

## Known gaps

- **Win32:** never run on real Windows; verified only under wine on Xvfb with no window manager, so
  `Event::Moved`, `Prop::MinSize`, live window shrinking and DPI change are compile-checked only.
  Accessibility is MSAA only (the stock controls' own default actions apply), explicit
  `set_a11y_*` overrides on menu items are not applied (`IAccPropServices::SetHmenuProp` could do it), the SpinBox up/down control has no name or value unless the app sets one. Checked with a
  UIA client (comtypes) and an MSAA client, not with a real screen reader. The sash's keyboard handling (tab stop, `WM_KEYDOWN`, focus rectangle) is compile-checked only, and its tab stop comes before both panes because it is created first. comctl32 left-aligns the first Table column; Table
  column widths are not re-scaled on DPI change; Shift+F10/Apps-key menus untested.
- **macOS:** run on real macOS. Audited by reading, not run: `objc_msgSend_stret` (x86_64
  only), exact-type msgSend transmutes, BOOL/NSInteger sizes, common-modes timers, file dialogs,
  the macOS popup-menu path, sash cursor rects, `setFrameTopLeftPoint:` flipping. Accessibility
  is AppKit labels/help only, and the sash has no native a11y role/value.
- **GNUstep quirks (not fixable here):** CJK glyphs render as `?` (font), menus appear as a
  separate window, the first click on an unfocused window only activates it.
- **GTK:** label mnemonics not applied (the core has no label-to-target link); ATK role overrides
  compile and run but were not checked with an AT client; the sash is a plain 6px line with no grip;
  window content is clipped when shrunk below the layout minimum.
- **Core:** panes in a splitter can overlap when an explicit position or minimum is below a pane's
  natural size (documented). Widgets nest at most `MAX_NESTING` (128) deep.
- **GNUstep (emulation):** ObjC exceptions raised during event dispatch by GNUstep's own assertions
  (negative view sizes) abort the process under `NSZombieEnabled`, because our hand-rolled event loop
  has no `NS_DURING`; timers pause while menus and dialogs run (GNUstep has no common-modes mode).
- **GTK:** a `ComboBox` with thousands of entries is slow (GTK 3 builds a menu item per entry, quadratic:
  5000 entries take about 4.5 s; a `ListBox`, now a headerless tree view, takes 30 ms for 10000), and
  creating thousands of `GtkEntry` widgets is quadratic inside GTK (about 0.4 s per 500 entries once
  4000 exist). `fuzz/src/bin/bench_native.rs` times these. GTK 3's file chooser trips an internal assertion now and then under Xvfb without a WM
  (excluded from the default fuzz run, `MODAL=files`), `GtkMenu` leaks a few KiB per
  create/destroy (also in plain C), and a popup menu holding a submenu can warn about negative sizes.
- **Win32 under wine:** `listbox set_items` of 10,000 items takes about 0.6 s and resizing a window of
  1,000 controls about 0.8 s, both dominated by wine's own message handling (a bare-Win32 loop costs
  about half of the latter). A tree is rebuilt from the whole model on every change (as on the
  other backends), so changing a tree node costs about 110 ms there for 10,000 nodes (a table cell is changed in place and costs nothing).
- **API gaps found by the file manager:** no key-event or focus callbacks on tables, no
  `on_activate` (Enter) on `TextInput`, no modal windows or input dialog, no multi-select, no
  right-clicked-row query for context menus, no column-resize or scroll-to-row control.
- **All platforms:** no custom drawing, rich text, caret/selection accessibility; RTL is a global
  opt-in; `Window::set_position` is a request that Wayland and some WMs ignore.
- **Lint:** `cargo clippy --all-targets -- -D warnings` is clean in every mode.

Release binary sizes, direct/transitive dependencies and file-manager screenshots for each backend
are in the README ("Binary size and linkage")

## Next steps

1. Run the Win32 backend on real Windows (CI job exists, unrun); verify move/min-size/DPI and the
   MSAA overrides with Narrator/NVDA.
2. Close the API gaps above, then a canvas/custom-draw widget and virtualised tables/trees.

## Workflow notes

- Give every build a private `CARGO_TARGET_DIR` under `/home/vscode/rungui-target/` (the
  devcontainer sets it); the workspace is bind-mounted from the host, so building into the repo's
  `target/` collides with host builds.
- A known flake: GTK file chooser location-entry completion under Xvfb without a WM can trip a
  Gtk-CRITICAL (probably GTK's own).
