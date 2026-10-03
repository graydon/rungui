//! GNUstep emulation only: rustc links with --as-needed, and Objective-C classes are
//! looked up by name at runtime, so no symbol would reference libgnustep-base/gui and
//! the linker would drop them (classes then vanish -> segfault). Referencing one symbol
//! from each library keeps them in DT_NEEDED.
#![cfg(rungui_gnustep)]
unsafe extern "C" {
    fn NSLog(fmt: *mut core::ffi::c_void, ...);
    fn NSApplicationMain(argc: i32, argv: *const *const i8) -> i32;
}
#[used]
static KEEP_BASE: unsafe extern "C" fn(*mut core::ffi::c_void, ...) = NSLog;
#[used]
static KEEP_GUI: unsafe extern "C" fn(i32, *const *const i8) -> i32 = NSApplicationMain;
