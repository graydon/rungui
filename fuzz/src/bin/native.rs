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

fn main() {
    let mut args = std::env::args().skip(1);
    let seeds: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(200);
    let first: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(1);
    // contained panics are part of the test; keep their messages out of the log
    std::panic::set_hook(Box::new(|_| {}));
    let app = App::new("rungui-fuzz-native").expect("toolkit init");
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
