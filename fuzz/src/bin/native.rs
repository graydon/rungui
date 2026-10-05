//! Drives the REAL backend (GTK, Cocoa on GNUstep, or Win32) with the random operations of
//! `ops.rs`, interleaved with the toolkit's own event loop, to shake out crashes, toolkit
//! criticals and leaks that the mock backend cannot show.
//!
//!   xvfb-run -a env G_DEBUG=fatal-warnings NO_AT_BRIDGE=1 \
//!       cargo run -p rungui-fuzz --bin native -- [seeds=200] [first_seed=1]
//!
//! (see scripts/fuzz-native.sh, which also builds it under AddressSanitizer). Each seed is a
//! deterministic byte stream, so a failure is reproduced by `native 1 <seed>`.
use rungui::*;
use rungui_fuzz::ops::{Fuzz, Mode};
use std::cell::RefCell;
use std::rc::Rc;

/// Bytes of "program" per seed and operations run per timer tick (the loop runs in between).
const PROGRAM_LEN: usize = 6_000;
const OPS_PER_TICK: u32 = 40;
const TICK_MS: u32 = 1;

/// xorshift64*: deterministic bytes for a seed.
fn bytes(seed: u64, n: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8
        })
        .collect()
}

/// With GTK: report every GLib warning/critical together with the Rust stack that caused it, then
/// abort (the same effect as `G_DEBUG=fatal-warnings`, but with a backtrace to find the culprit).
#[cfg(all(target_os = "linux", not(feature = "emulate-mac")))]
mod glib_log {
    use std::ffi::{CStr, c_char, c_int, c_void};
    /// GLib log levels that mean a bug: ERROR, CRITICAL, WARNING.
    const BAD: c_int = 0b100 | 0b1000 | 0b10000;
    unsafe extern "C" {
        fn gtk_window_list_toplevels() -> *mut GList;
        fn gtk_widget_get_visible(w: *mut c_void) -> c_int;
        fn gtk_widget_get_realized(w: *mut c_void) -> c_int;
        fn gtk_widget_get_mapped(w: *mut c_void) -> c_int;
        fn gtk_widget_get_child_visible(w: *mut c_void) -> c_int;
        fn gtk_widget_get_allocated_width(w: *mut c_void) -> c_int;
        fn gtk_widget_get_allocated_height(w: *mut c_void) -> c_int;
        fn gtk_container_forall(c: *mut c_void, f: unsafe extern "C" fn(*mut c_void, *mut c_void), d: *mut c_void);
        fn g_type_name_from_instance(i: *mut c_void) -> *const c_char;
        fn g_log_set_default_handler(
            f: unsafe extern "C" fn(*const c_char, c_int, *const c_char, *mut c_void),
            data: *mut c_void,
        ) -> *mut c_void;
    }
    #[repr(C)]
    struct GList {
        data: *mut c_void,
        next: *mut GList,
    }
    unsafe extern "C" {
        fn g_type_check_instance_is_a(i: *mut c_void, t: usize) -> c_int;
        fn gtk_container_get_type() -> usize;
    }
    /// Print `w` and everything below it: type, visible/realized/mapped, allocation.
    unsafe fn dump(w: *mut c_void, depth: usize) {
        unsafe {
            let name = CStr::from_ptr(g_type_name_from_instance(w)).to_string_lossy();
            eprintln!(
                "{:indent$}{name} vis={} real={} map={} childvis={} {}x{}",
                "",
                gtk_widget_get_visible(w),
                gtk_widget_get_realized(w),
                gtk_widget_get_mapped(w),
                gtk_widget_get_child_visible(w),
                gtk_widget_get_allocated_width(w),
                gtk_widget_get_allocated_height(w),
                indent = depth * 2
            );
            if g_type_check_instance_is_a(w, gtk_container_get_type()) != 0 {
                unsafe extern "C" fn each(child: *mut c_void, depth: *mut c_void) {
                    unsafe { dump(child, depth as usize) };
                }
                gtk_container_forall(w, each, (depth + 1) as *mut c_void);
            }
        }
    }
    unsafe extern "C" fn handler(domain: *const c_char, level: c_int, msg: *const c_char, _d: *mut c_void) {
        let text = |p: *const c_char| {
            if p.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
            }
        };
        if level & BAD == 0 && std::env::var_os("RUNGUI_FUZZ_VERBOSE").is_none() {
            return; // info and debug chatter of the toolkit
        }
        eprintln!("GLib [{}] level {level}: {}", text(domain), text(msg));
        if level & BAD != 0 {
            eprintln!("{}", std::backtrace::Backtrace::force_capture());
            unsafe {
                let mut l = gtk_window_list_toplevels();
                while !l.is_null() {
                    dump((*l).data, 0);
                    l = (*l).next;
                }
            }
            std::process::abort();
        }
    }
    pub fn install() {
        unsafe { g_log_set_default_handler(handler, std::ptr::null_mut()) };
    }
}
#[cfg(not(all(target_os = "linux", not(feature = "emulate-mac"))))]
mod glib_log {
    pub fn install() {}
}

/// Resident memory (KiB) and open file descriptors of this process, to spot leaks across seeds.
fn resources() -> (u64, usize) {
    let rss_pages = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|p| p.parse::<u64>().ok()))
        .unwrap_or(0);
    let fds = std::fs::read_dir("/proc/self/fd").map_or(0, |d| d.count());
    (rss_pages * 4, fds)
}

/// Print resource use every this many seeds.
const REPORT_EVERY: u64 = 100;

fn main() {
    let mut args = std::env::args().skip(1);
    let seeds: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(200);
    let first: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(1);
    // contained panics are part of the test; keep their messages out of the log
    std::panic::set_hook(Box::new(|_| {}));
    let app = App::new("rungui-fuzz-native").expect("toolkit init");
    glib_log::install();
    App::set_quit_on_last_close(false);
    let state: Rc<RefCell<(u64, Option<Rc<Fuzz>>)>> = Rc::new(RefCell::new((first, None)));
    let st = state.clone();
    Timer::every(TICK_MS, move || {
        let mut s = st.borrow_mut();
        let seed = s.0;
        if s.1.is_none() {
            if seed >= first + seeds {
                println!("done: {seeds} seeds");
                App::quit();
                return;
            }
            if seed % REPORT_EVERY == 0 {
                let (rss, fds) = resources();
                println!("resources at seed {seed}: rss {rss} KiB, {fds} fds");
            }
            eprintln!("seed {seed}");
            s.1 = Some(Fuzz::new(&bytes(seed, PROGRAM_LEN), Mode::Native, true));
        }
        let fz = s.1.clone().expect("set above");
        drop(s);
        if !fz.run_some(OPS_PER_TICK) {
            fz.finish();
            let mut s = st.borrow_mut();
            s.1 = None;
            s.0 += 1;
        }
    });
    app.run();
}
