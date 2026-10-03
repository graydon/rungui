//! Dual-pane file manager, built from rungui's public API only. See app.rs for the layout.
//!
//!     cargo run --example file_manager -- [dirA] [dirB]
//!
//! Keys: F2 rename, F5 copy, F6 move, F7 new folder, F8 delete, F9 switch pane, Ctrl+H hidden
//! files, Ctrl+R refresh, Alt+Up parent folder, Ctrl+Q quit. Set RUNGUI_FM_TRACE=1 to print
//! machine-readable progress lines (used by scripts/smoke-filemanager.sh).

mod app;
mod fsmodel;

use rungui::App;
use std::path::PathBuf;

/// Windows `canonicalize` returns `\\?\C:\...`; show the ordinary form.
fn plain(p: PathBuf) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

fn start_dir(arg: Option<String>, fallback: PathBuf) -> PathBuf {
    match arg {
        Some(a) => match std::fs::canonicalize(&a) {
            Ok(p) if p.is_dir() => plain(p),
            _ => {
                eprintln!("file_manager: \"{a}\" is not a directory, using {}", fallback.display());
                fallback
            }
        },
        None => fallback,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).filter(|p| p.is_dir());
    let a = start_dir(args.next(), cwd.clone());
    let b = start_dir(args.next(), home.unwrap_or_else(|| cwd.parent().map(|p| p.to_path_buf()).unwrap_or(cwd)));
    let app = App::new("file-manager").expect("init");
    let _fm = app::build(app::Options { dirs: [a, b] });
    app.run();
}
