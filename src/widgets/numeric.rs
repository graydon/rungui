//! Numeric widgets: `Slider`, `SpinBox`, `ProgressBar`.

use super::*;

fn set_value(id: WidgetId, v: f64) {
    // clamp first: the backend must never see NaN or a value outside the range
    let Some(v) = core::read(id, |n| {
        n.range().map(|r| {
            if v.is_nan() {
                r.range.0
            } else {
                v.clamp(r.range.0, r.range.1.max(r.range.0))
            }
        })
    })
    .flatten() else {
        return;
    };
    core::set(
        id,
        false,
        |n| {
            if let Some(r) = n.range_mut() {
                r.value = v;
            }
        },
        Prop::Value(v),
    );
}
/// Normalise a (min, max, step) request: finite, `min <= max`, `step > 0` (NaN / infinities
/// would make `f64::clamp` panic later).
fn sane_range(min: f64, max: f64, step: f64) -> (f64, f64, f64) {
    let min = if min.is_finite() { min } else { 0.0 };
    let max = if max.is_finite() {
        max.max(min)
    } else {
        min.max(min + 100.0)
    };
    (
        min,
        max,
        if step.is_finite() && step > 0.0 {
            step
        } else {
            1.0
        },
    )
}

fn set_range(id: WidgetId, min: f64, max: f64, step: f64) {
    let (min, max, step) = sane_range(min, max, step);
    core::set(
        id,
        false,
        |n| {
            if let Some(r) = n.range_mut() {
                r.range = (min, max, step);
                r.value = r.value.clamp(min, max);
            }
        },
        Prop::Range { min, max, step },
    );
    let v = core::read(id, |n| n.range().map(|r| r.value))
        .flatten()
        .unwrap_or(min);
    core::set(id, false, |_| {}, Prop::Value(v));
}
fn value(id: WidgetId) -> f64 {
    core::read(id, |n| n.range().map(|r| r.value))
        .flatten()
        .unwrap_or(0.0)
}
fn on_value(id: WidgetId, mut f: impl FnMut(f64) + 'static) {
    on(id, Ev::Value, move |e| {
        if let Event::Value(v) = e {
            f(*v)
        }
    })
}

macro_rules! value_methods {
    ($t:ident) => {
        impl $t {
            /// Set the value, clamped to the range (NaN becomes the minimum); no callback fires.
            pub fn set_value(&self, v: f64) {
                set_value(self.id(), v)
            }
            /// The current value.
            pub fn value(&self) -> f64 {
                value(self.id())
            }
            /// Set the range and step; non-finite numbers are replaced by safe ones and the value is clamped.
            pub fn set_range(&self, min: f64, max: f64, step: f64) {
                set_range(self.id(), min, max, step)
            }
            /// Run `f` with the new value when the user changes it.
            pub fn on_change(&self, f: impl FnMut(f64) + 'static) {
                on_value(self.id(), f)
            }
        }
    };
}
value_methods!(Slider);
value_methods!(SpinBox);

impl Slider {
    /// A slider over `min..=max` with step 1, starting at `min`.
    pub fn new(parent: impl Into<WidgetId>, min: f64, max: f64) -> Slider {
        let s = make(Slider::from_id, Kind::Slider, parent, |n| {
            if let Some(r) = n.range_mut() {
                r.range = sane_range(min, max, 1.0)
            }
        });
        s.set_value(min);
        s
    }
}
impl SpinBox {
    /// A spin box over `min..=max` changing by `step`, starting at `min`.
    pub fn new(parent: impl Into<WidgetId>, min: f64, max: f64, step: f64) -> SpinBox {
        let s = make(SpinBox::from_id, Kind::SpinBox, parent, |n| {
            if let Some(r) = n.range_mut() {
                r.range = sane_range(min, max, step)
            }
        });
        s.set_value(min);
        s
    }
}

impl ProgressBar {
    /// A progress bar showing `0.0..=1.0` (see [`ProgressBar::set_fraction`]).
    pub fn new(parent: impl Into<WidgetId>) -> ProgressBar {
        make(ProgressBar::from_id, Kind::ProgressBar, parent, |_| {})
    }
    /// 0.0..=1.0 (clamped).
    pub fn set_fraction(&self, f: f64) {
        set_value(self.id(), f)
    }
    /// The fraction shown.
    pub fn fraction(&self) -> f64 {
        value(self.id())
    }
    /// Show a busy animation instead of a fraction.
    pub fn set_indeterminate(&self, v: bool) {
        core::set(
            self.id(),
            false,
            |n| {
                if let Some(r) = n.range_mut() {
                    r.indeterminate = v
                }
            },
            Prop::Indeterminate(v),
        );
    }
}
