//! libFuzzer target: random public-API operations against the mock backend.
//!
//!   cargo +nightly fuzz run api_mock           (from the repository root; uses AddressSanitizer)
#![no_main]
use libfuzzer_sys::fuzz_target;
use rungui_fuzz::ops::{Fuzz, Mode};

/// Operations per input; keeps one execution fast.
const MAX_OPS: u32 = 2_000;

fuzz_target!(|data: &[u8]| {
    Fuzz::new(data, Mode::Mock, false).run(MAX_OPS);
});
