//! Replays a seed of the native driver (`native.rs`) against the mock backend and prints the widget
//! tree it ends with: how to look at the state that made a real toolkit complain.
//!
//!   cargo run --manifest-path fuzz/Cargo.toml --features mock --bin replay_mock -- <seed> [ops]
use rungui_fuzz::ops::{Fuzz, Mode};

/// Must match `PROGRAM_LEN` in `native.rs`.
const PROGRAM_LEN: usize = 6_000;

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
    let seed: u64 = args.next().and_then(|a| a.parse().ok()).expect("seed");
    let ops: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(u32::MAX);
    std::panic::set_hook(Box::new(|_| {}));
    // Mode::Native consumes the input exactly as the native driver does
    let fz = Fuzz::new(&bytes(seed, PROGRAM_LEN), Mode::Native, true);
    fz.run_some(ops);
    println!("{}", rungui::backend::mock::dump());
}
