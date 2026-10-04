//! Backend selection and native library linking, all in one place.
//!
//! Emits these cfgs (declared via check-cfg so `unexpected_cfgs` stays quiet):
//!   rungui_gtk      GTK3 backend            (linux/other unix, default)
//!   rungui_win32    Win32 backend           (target_os = "windows", incl. x86_64-pc-windows-gnu cross builds)
//!   rungui_cocoa    Cocoa/AppKit backend    (target_os = "macos"  OR  linux + feature "emulate-mac")
//!   rungui_gnustep  set together with rungui_cocoa when emulating on linux: libobjc is GCC/GNUstep
//!                 (no objc_msgSend, no *_stret; use objc_msg_lookup) and the frameworks are
//!                 libgnustep-base / libgnustep-gui.
//! Exactly one of rungui_gtk / rungui_win32 / rungui_cocoa is set.
use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_EMULATE_MAC");
    for c in [
        "rungui_gtk",
        "rungui_win32",
        "rungui_cocoa",
        "rungui_gnustep",
    ] {
        println!("cargo:rustc-check-cfg=cfg({c})");
    }
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let emulate = env::var_os("CARGO_FEATURE_EMULATE_MAC").is_some();
    let link = |l: &str| println!("cargo:rustc-link-lib={l}");

    match os.as_str() {
        "windows" => {
            // the `windows` crate links its own import libraries (raw-dylib): nothing to emit
            println!("cargo:rustc-cfg=rungui_win32");
        }
        "macos" => {
            println!("cargo:rustc-cfg=rungui_cocoa");
            link("framework=AppKit");
            link("framework=Foundation");
            link("framework=CoreGraphics");
            link("objc");
        }
        "linux" if emulate => {
            println!("cargo:rustc-cfg=rungui_cocoa");
            println!("cargo:rustc-cfg=rungui_gnustep");
            for l in ["gnustep-gui", "gnustep-base", "objc"] {
                link(l);
            }
        }
        _ => {
            println!("cargo:rustc-cfg=rungui_gtk");
            for l in [
                "gtk-3",
                "gdk-3",
                "gdk_pixbuf-2.0",
                "pango-1.0",
                "cairo",
                "atk-1.0",
                "gio-2.0",
                "gobject-2.0",
                "glib-2.0",
            ] {
                link(l);
            }
        }
    }
}
