//! Numeric widgets: `Slider`, `SpinBox`, `ProgressBar`.

use super::*;

fn set_value(id: WidgetId, v: f64) {
    let mut vv = v;
    core::set(
        id,
        false,
        |n| {
            vv = if v.is_nan() {
                n.range.0
            } else {
                v.clamp(n.range.0, n.range.1.max(n.range.0))
            };
            n.value = vv;
        },
        Prop::Value(v),
    );
    // re-push the clamped value (the closure ran before `Prop::Value(v)` was consumed)
    core::set(id, false, |_| {}, Prop::Value(vv));
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
            n.range = (min, max, step);
            n.value = n.value.clamp(min, max);
        },
        Prop::Range { min, max, step },
    );
    let v = core::read(id, |n| n.value).unwrap_or(min);
    core::set(id, false, |_| {}, Prop::Value(v));
}
fn value(id: WidgetId) -> f64 {
    core::read(id, |n| n.value).unwrap_or(0.0)
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
            pub fn set_value(&self, v: f64) {
                set_value(self.id(), v)
            }
            pub fn value(&self) -> f64 {
                value(self.id())
            }
            pub fn set_range(&self, min: f64, max: f64, step: f64) {
                set_range(self.id(), min, max, step)
            }
            pub fn on_change(&self, f: impl FnMut(f64) + 'static) {
                on_value(self.id(), f)
            }
        }
    };
}
value_methods!(Slider);
value_methods!(SpinBox);

impl Slider {
    pub fn new(parent: impl Into<WidgetId>, min: f64, max: f64) -> Slider {
        let s = make(Slider::from_id, Kind::Slider, parent, |n| {
            n.range = sane_range(min, max, 1.0)
        });
        s.set_value(min);
        s
    }
}
impl SpinBox {
    pub fn new(parent: impl Into<WidgetId>, min: f64, max: f64, step: f64) -> SpinBox {
        let s = make(SpinBox::from_id, Kind::SpinBox, parent, |n| {
            n.range = sane_range(min, max, step)
        });
        s.set_value(min);
        s
    }
}

impl ProgressBar {
    pub fn new(parent: impl Into<WidgetId>) -> ProgressBar {
        make(ProgressBar::from_id, Kind::ProgressBar, parent, |_| {})
    }
    /// 0.0..=1.0 (clamped).
    pub fn set_fraction(&self, f: f64) {
        set_value(self.id(), f)
    }
    pub fn fraction(&self) -> f64 {
        value(self.id())
    }
    pub fn set_indeterminate(&self, v: bool) {
        core::set(
            self.id(),
            false,
            |n| n.indeterminate = v,
            Prop::Indeterminate(v),
        );
    }
}
