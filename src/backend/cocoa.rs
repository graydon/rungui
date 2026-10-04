//! Cocoa/AppKit backend built on the objc2 ecosystem (macOS; also GNUstep + libobjc2 when emulated
//! on linux, cfg `rungui_gnustep`, see `gnustep-objc2/`).
//!
//! This is the objc2 variant of the backend: typed bindings (`objc2-app-kit`, `objc2-foundation`),
//! `define_class!` for the runtime classes, `Retained` for ownership and `MainThreadMarker` for the
//! main-thread rule. No `extern` block is declared anywhere in it.
//!
//! * [`imp`] is the backend proper and is `#![forbid(unsafe_code)]`: widget graph, state tables,
//!   event translation and the class definitions `RunguiTarget` (target, delegate and data source
//!   of everything), `RunguiFlipView` (flipped `NSView`: windows, group boxes and tab pages use it
//!   so that the core's top-left coordinates work unchanged) and `RunguiSash` (the splitter handle).
//! * [`unsafe_calls`] is the only module that contains `unsafe`: thin, individually justified
//!   wrappers around the AppKit methods that objc2-app-kit itself marks `unsafe` (target/action,
//!   delegates and data sources are unretained, `NSWindow` creation, `NSTimer`, a few nullable
//!   getters), the `RunguiApp` class (`sendEvent:` needs a `super` message send; it turns right
//!   clicks into `Event::ContextMenu`), the per-radio-button action selectors (a `class_addMethod`:
//!   AppKit auto-groups radios that share target+action), the thread-safe wake-up
//!   (`performSelectorOnMainThread:` on the leaked global target) and the raw RGBA copy for images.
//!
//! Containers: Window (flipped content view), GroupBox (NSBox + flipped content view), Tabs/Page
//! (NSTabView + flipped page views). Layout comes from the core (absolute frames). Accessibility:
//! AppKit controls are natively accessible; `a11y_changed` pushes the core's computed
//! names/descriptions into `accessibilityLabel`/`accessibilityHelp`.
//!
//! GNUstep differences are `#[cfg(rungui_gnustep)]` blocks (hand-rolled event loop, popups through
//! `popUpContextMenu:withEvent:forView:`, no sort-indicator images, ...) plus `respondsToSelector`
//! guards: unlike hand-FFI, objc2 *panics* on a selector the runtime lacks.
#![deny(unsafe_code)]

mod imp;
mod unsafe_calls;

pub use imp::Cocoa;
