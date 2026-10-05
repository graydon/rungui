//! Backend selection and native library linking, all in one place.
//!
//! Emits these cfgs (declared via check-cfg so `unexpected_cfgs` stays quiet):
//!   rungui_gtk      GTK3 backend            (linux/other unix, default)
//!   rungui_win32    Win32 backend           (target_os = "windows", incl. x86_64-pc-windows-gnu cross builds)
//!   rungui_cocoa    Cocoa/AppKit backend    (target_os = "macos"  OR  linux + feature "emulate-mac")
//!   rungui_gnustep  set together with rungui_cocoa when emulating on linux: the objc2 crates run on
//!                 libobjc2 (not Apple's runtime) and the frameworks are libgnustep-base / libgnustep-gui.
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
            // AppKit / Foundation / libobjc are linked by the objc2-app-kit, objc2-foundation and
            // objc2 crates themselves (their `#[link(kind = "framework")]`): nothing to emit.
            println!("cargo:rustc-cfg=rungui_cocoa");
        }
        "linux" if emulate => {
            println!("cargo:rustc-cfg=rungui_cocoa");
            println!("cargo:rustc-cfg=rungui_gnustep");
            // The objc2 crates (features gnustep-*) name libobjc / libgnustep-base / libgnustep-gui
            // themselves. They need libobjc2 and a GNUstep built against it (Debian's GNUstep uses
            // GCC's libobjc, which objc2 cannot use); point RUNGUI_GNUSTEP_PREFIX at such an install
            // (lib dirs `$P/lib` and `$P/GS/local/lib`, see src/backend/cocoa/gnustep-objc2/).
            println!("cargo:rerun-if-env-changed=RUNGUI_GNUSTEP_PREFIX");
            if let Ok(p) = env::var("RUNGUI_GNUSTEP_PREFIX") {
                for d in [format!("{p}/lib"), format!("{p}/GS/local/lib")] {
                    println!("cargo:rustc-link-search=native={d}");
                    println!("cargo:rustc-link-arg=-Wl,-rpath,{d}");
                }
            }
        }
        _ => {
            println!("cargo:rustc-cfg=rungui_gtk");
            // GTK is linked by the gtk-rs crates (via pkg-config / system-deps), not here.
        }
    }
}
