//! Timers.

use super::*;

/// A running timer; callbacks run on the main thread. Dropping the handle does NOT stop it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Timer(u64);

impl Timer {
    /// Run `f` once after `ms` milliseconds (on the UI thread).
    pub fn once(ms: u32, f: impl FnMut() + 'static) -> Timer {
        Timer::start(ms, false, f)
    }
    /// Run `f` every `ms` milliseconds until stopped.
    pub fn every(ms: u32, f: impl FnMut() + 'static) -> Timer {
        Timer::start(ms, true, f)
    }
    fn start(ms: u32, repeat: bool, f: impl FnMut() + 'static) -> Timer {
        match core::timer_start(ms, repeat, Box::new(f)) {
            Ok(t) => Timer(t),
            Err(e) => {
                core::set_error(e);
                Timer(0)
            }
        }
    }
    /// Stop the timer; stopping a stopped or failed timer does nothing.
    pub fn stop(&self) {
        core::timer_stop(self.0)
    }
}
