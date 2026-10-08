//! Inline month calendar.

use super::*;

impl Calendar {
    /// A month calendar with today selected.
    pub fn new(parent: impl Into<WidgetId>) -> Calendar {
        make(Calendar::from_id, Kind::Calendar, parent, |_| {})
    }
    /// Select and show a date. An invalid date is ignored. No callback fires.
    pub fn set_date(&self, d: Date) {
        if d.is_valid() {
            core::set(
                self.id(),
                false,
                |n| {
                    if let Some(x) = n.date_mut() {
                        *x = d
                    }
                },
                Prop::Date(d),
            );
        }
    }
    /// The selected date.
    pub fn date(&self) -> Date {
        core::read(self.id(), |n| n.date().copied())
            .flatten()
            .unwrap_or_else(Date::today)
    }
    /// Run `f` with the new date when the user picks one.
    pub fn on_change(&self, mut f: impl FnMut(Date) + 'static) {
        on(self.id(), Ev::Date, move |e| {
            if let Event::DateChanged(d) = e {
                f(*d)
            }
        })
    }
}
