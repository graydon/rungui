//! One-shot and repeating timers.

use super::{guarded, wake_if_scheduled, with};
use crate::backend::{Backend, Native as B};
use crate::types::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TIMER: AtomicU64 = AtomicU64::new(1);

pub(super) struct TimerEntry {
    pub(super) cb: Option<Box<dyn FnMut()>>,
    pub(super) repeat: bool,
}

// ---------------------------------------------------------------- timers

pub fn timer_start(ms: u32, repeat: bool, cb: Box<dyn FnMut()>) -> Result<u64> {
    let token = NEXT_TIMER.fetch_add(1, Ordering::Relaxed);
    with(|r| {
        r.timers.insert(
            token,
            TimerEntry {
                cb: Some(cb),
                repeat,
            },
        )
    })
    .ok_or(Error::NotInitialized)?;
    if let Err(e) = B::timer_start(token, ms.max(1), repeat) {
        with(|r| r.timers.remove(&token));
        return Err(e);
    }
    Ok(token)
}

pub fn timer_stop(token: u64) {
    if with(|r| r.timers.remove(&token)).flatten().is_some() {
        B::timer_stop(token);
    }
}

/// Backend entry point: timer `token` expired.
pub fn timer_fired(token: u64) {
    let taken = with(|r| {
        let repeat = r.timers.get(&token)?.repeat;
        let cb = if repeat {
            r.timers.get_mut(&token)?.cb.take()
        } else {
            r.timers.remove(&token)?.cb
        };
        Some((cb, repeat))
    })
    .flatten();
    let Some((Some(mut cb), repeat)) = taken else {
        return;
    };
    guarded(|| cb());
    if repeat {
        with(|r| {
            if let Some(t) = r.timers.get_mut(&token) {
                t.cb = Some(cb);
            }
        });
    } else {
        B::timer_stop(token);
    }
    wake_if_scheduled();
}
