//! GNUstep emulation only: rustc links with --as-needed, and Objective-C classes are looked up by
//! name at runtime, so no symbol would reference libgnustep-gui and the linker would drop it
//! (classes then vanish -> crash). Referencing one function that lives in it (through the
//! objc2-app-kit binding, no hand-declared extern) keeps it in DT_NEEDED; libgnustep-base comes
//! along as a dependency of libgnustep-gui.
#![cfg(rungui_gnustep)]
use objc2_app_kit::NSRectFill;
use objc2_foundation::NSRect;

#[used]
static KEEP_GUI: extern "C-unwind" fn(NSRect) = NSRectFill;
