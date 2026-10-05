//! Seeded random-operation test: the same interpreter the libFuzzer target uses
//! (`fuzz/src/ops.rs`), run for a fixed set of seeds on the mock backend so every `cargo test
//! --features mock` exercises the whole API with hostile arguments, wrong-kind handles and
//! re-entrant, panicking callbacks.
#![cfg(feature = "mock")]

#[path = "../fuzz/src/ops.rs"]
mod ops;

use ops::{Fuzz, Mode};

/// Operations per seed.
const MAX_OPS: u32 = 3_000;
/// Bytes of input per seed.
const INPUT_LEN: usize = 12_000;

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

fn run_seeds(range: std::ops::Range<u64>) {
    // contained callback panics are expected; keep them out of the test output
    std::panic::set_hook(Box::new(|_| {}));
    for seed in range {
        let data = bytes(seed, INPUT_LEN);
        let r = std::panic::catch_unwind(|| Fuzz::new(&data, Mode::Mock, true).run(MAX_OPS));
        if r.is_err() {
            let _ = std::panic::take_hook();
            panic!("random ops failed for seed {seed}");
        }
    }
}

#[test]
fn random_ops_a() {
    run_seeds(0..40);
}
#[test]
fn random_ops_b() {
    run_seeds(40..80);
}
#[test]
fn random_ops_c() {
    run_seeds(80..120);
}
#[test]
fn random_ops_d() {
    run_seeds(120..160);
}
