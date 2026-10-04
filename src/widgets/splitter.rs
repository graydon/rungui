//! The two-pane `Splitter` container.

use super::*;

/// A two-pane container: create exactly two children with the splitter as parent (the first is
/// the left/top pane, under RTL the right pane for `Horizontal`); a third child is refused like
/// any invalid parent. Panes fill the splitter's cross axis; the first pane gets
/// [`Splitter::position`] pixels along the main axis, the sash `set_spacing` pixels (default 6)
/// and the second pane the rest. Hiding a pane gives the whole area to the other one. The user
/// drags the sash to move the split; on backends without a native sash the split is fixed.
impl Splitter {
    pub fn new(parent: impl Into<WidgetId>, orientation: Orientation) -> Splitter {
        let s = make(Splitter::from_id, Kind::Splitter, parent, |n| {
            n.lay.padding = 0;
            n.lay.expand = 1.0;
            if let Some(sp) = n.split.as_mut() {
                sp.orient = orientation;
            }
        });
        if s.is_alive() {
            core::create_sash(s.id(), orientation);
        }
        s
    }
    pub fn orientation(&self) -> Orientation {
        core::read(self.id(), |n| n.split.as_ref().map(|s| s.orient))
            .flatten()
            .unwrap_or_default()
    }
    /// Size of the first pane along the main axis, in pixels. Remembered as requested and clamped
    /// to the current size (and the pane minimums) at every layout, so shrinking and re-growing
    /// the window restores it. Does not call `on_move`.
    pub fn set_position(&self, px: i32) {
        core::split_set(self.id(), self::px(px), false);
    }
    /// The effective first-pane size from the last layout (before the first layout: the requested
    /// position, or 0 if none). Without `set_position` the space is split evenly.
    pub fn position(&self) -> i32 {
        core::read(self.id(), |n| {
            n.split.as_ref().map_or(0, |s| {
                if s.laid_out {
                    s.actual
                } else {
                    s.pos.unwrap_or(0)
                }
            })
        })
        .unwrap_or(0)
    }
    /// Minimum main-axis sizes of the first and second pane (default 0, 0). Positions are clamped
    /// so both fit; if the splitter is too small for both, the first pane wins.
    pub fn set_min_pane_sizes(&self, first: i32, second: i32) {
        core::update(self.id(), true, |n| {
            if let Some(s) = n.split.as_mut() {
                s.min = (self::px(first), self::px(second));
            }
        });
    }
    /// Called with the new position after the USER moved the sash (drag, keyboard, AT).
    pub fn on_move(&self, mut f: impl FnMut(i32) + 'static) {
        on(self.id(), Ev::SashMoved, move |e| {
            if let Event::SashDragged(p) = e {
                f(*p)
            }
        });
    }
}
