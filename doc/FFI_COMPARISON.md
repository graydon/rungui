# Hand-declared FFI vs. ecosystem crates (experiment, branch `reuse-ffi-crates`)

Each backend was rewritten with no FFI declarations of its own. Baseline = `main`.

| | GTK (`gtk` 0.18) | Win32 (`windows` 0.62) | Cocoa (`objc2` 0.6, objc2-foundation/app-kit 0.3) |
|---|---|---|---|
| Backend LOC before -> after | 2361 -> 1558 (-34%) | 4844 -> 4085 (-16%) | 2698 (+13 keepalive) -> 2923 (+8%) |
| `unsafe` | 115 -> **0**; `#![forbid(unsafe_code)]` on the module | 166 -> 115 (only `lists.rs` is forbid-clean) | 21 -> 26 blocks, all in `unsafe_calls.rs`; `imp.rs` (2552 lines) is `forbid(unsafe_code)` |
| Extra crates (unique) | 96 | ~30 (`cargo tree` 31 lines) | 6 |
| Clean build | 20 s (was 2.4 s) | 9.7 s (was 2.5 s) | check 3.5 s (was ~1 s) |
| Artifact size | hello 796 KB -> 996 KB; kitchen_sink 979 KB -> 1185 KB (+21%) | kitchen_sink.exe 1.86 MB -> 2.45 MB (+31%) | not measured (no Apple linker) |
| Verified | cargo test, mock tests, smoke-gtk, soak, file manager: all pass | builds+links PE32+ (gnu), checks on msvc; **never run** (no wine/box64/qemu on this aarch64 host) | checks clean on both darwin targets; full GNUstep smoke passes **only** on a libobjc2+GNUstep stack built from source (~6 min); never run on Apple frameworks |

## Findings
* **GTK is the clear win**: smaller, fully safe, no behaviour lost. Costs: 96 crates, 8x build time, +200 KB/binary.
  `Widget::destroy` is `unsafe` in gtk-rs, so teardown uses `close()`/`remove()` (no GLib criticals in the soak test, leaks not measured).
* **Win32**: the crate removes constant/struct/GUID bookkeeping and cuts the COM code (file dialog ~140 -> ~90 lines), but
  every call is still `unsafe`, constants are inconsistently typed, handles lack `Hash`/`Send`. Safety gain is small; forbid(unsafe) is not realistic beyond a thin unsafe layer.
* **Cocoa**: the typed API removes retain/release and ABI (`objc_msgSend_stret`) concerns and `define_class!` replaces hand `class_addMethod`.
  But code grew, AppKit needs ~50 hand-picked cargo features, objc2 panics on selectors GNUstep lacks (guards restored),
  and it only runs on GNUstep with libobjc2, not Debian's GCC libobjc used by the existing emulation.
  Files: `src/backend/cocoa/gnustep-objc2/` (build script + run script). `emulate-mac` now needs that stack and `RUNGUI_GNUSTEP_PREFIX`.
* Still stale: `doc/BUILDING.md`, `Cargo.toml` description/`lib.rs` docs say "hand-declared FFI".
* Crate-wide `#![forbid(unsafe_code)]` is not achievable: Win32 and Cocoa need a small unsafe island; only the GTK module can be fully forbidden.
