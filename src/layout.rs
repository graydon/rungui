//! Rust-side layout: stacks (HBox/VBox, also used by Window/Page/GroupBox as vertical stacks)
//! and grids, computed from backend-reported preferred sizes. Pure functions over the registry;
//! `compute` returns the `Bounds` the core must push to the backend (only those that changed).
//!
//! Space model: along the stack axis children get their natural size, plus a share of leftover
//! space proportional to `expand`; across the axis they follow `Align` (default Fill).
//! Hidden children take no space. Layout never shrinks below natural sizes (content overflows
//! rather than being squeezed), keeping the algorithm tiny and predictable.

use crate::backend::{Backend, Kind, Native as B, Prop};
use crate::core::{Node, Registry, panes_of};
use crate::types::*;

/// What layout pushes: `Prop::Bounds`, plus `Prop::Visible` for splitter sashes.
type Out = Vec<(WidgetId, Prop<'static>)>;

thread_local! { static RTL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

/// Mirror horizontal placement (right-to-left locales). The caller must relayout.
pub fn set_rtl(v: bool) {
    RTL.with(|c| c.set(v));
}
fn rtl() -> bool {
    RTL.with(|c| c.get())
}
pub fn is_rtl() -> bool {
    rtl()
}

/// The visible panes of a splitter (at most two).
fn split_panes(r: &Registry, id: WidgetId) -> Vec<WidgetId> {
    let Some(n) = r.nodes.get(&id) else { return vec![] };
    panes_of(r, n).into_iter().filter(|c| r.nodes.get(c).is_some_and(|c| c.visible)).take(2).collect()
}

fn visible_children(r: &Registry, id: WidgetId) -> Vec<WidgetId> {
    r.nodes.get(&id).map_or_else(Vec::new, |n| {
        n.children
            .iter()
            .copied()
            .filter(|c| r.nodes.get(c).is_some_and(|c| c.visible && c.kind.in_layout()))
            .collect()
    })
}

/// Resolved grid cells `(child, col, row, colspan, rowspan)` plus (ncols, nrows).
fn grid_cells(r: &Registry, id: WidgetId) -> (Vec<(WidgetId, usize, usize, usize, usize)>, usize, usize) {
    let cols = r.nodes[&id].lay.cols.max(1);
    let mut next = 0usize;
    let mut cells = vec![];
    for c in visible_children(r, id) {
        let (col, row, cs, rs) = match r.nodes[&c].lay.cell {
            Some((a, b, cs, rs)) => (a, b, cs.max(1), rs.max(1)),
            None => {
                let p = (next % cols, next / cols, 1, 1);
                next += 1;
                p
            }
        };
        cells.push((c, col, row, cs, rs));
    }
    let nc = cells.iter().map(|c| c.1 + c.3).max().unwrap_or(0).max(cols.min(cells.len()));
    let nr = cells.iter().map(|c| c.2 + c.4).max().unwrap_or(0);
    (cells, nc, nr)
}

/// Column widths / row heights and expand weights of a grid.
fn grid_tracks(r: &Registry, id: WidgetId) -> (Vec<(WidgetId, usize, usize, usize, usize)>, [Vec<i32>; 2], [Vec<f32>; 2]) {
    let (cells, nc, nr) = grid_cells(r, id);
    let mut size = [vec![0; nc], vec![0; nr]];
    let mut exp = [vec![0.0f32; nc], vec![0.0f32; nr]];
    let sp = r.nodes[&id].lay.spacing;
    for pass_span in [false, true] {
        for &(c, col, row, cs, rs) in &cells {
            if (cs > 1 || rs > 1) != pass_span {
                continue;
            }
            let m = measure(r, c);
            let e = r.nodes[&c].lay.expand;
            for (axis, (start, span, need)) in [(col, cs, m.w), (row, rs, m.h)].into_iter().enumerate() {
                let end = start + span - 1;
                let have: i32 = size[axis][start..=end].iter().sum::<i32>() + sp * (span as i32 - 1);
                if need > have {
                    size[axis][end] += need - have; // spanning cells grow their last track
                }
                if !pass_span {
                    exp[axis][start] = exp[axis][start].max(e);
                }
            }
        }
    }
    (cells, size, exp)
}

fn is_horizontal(k: Kind) -> bool {
    k == Kind::HBox
}

/// Size needed by the children (no padding, no chrome).
fn measure_children(r: &Registry, id: WidgetId, n: &Node) -> Size {
    let sp = n.lay.spacing;
    if let Some(split) = n.split.as_ref() {
        // natural: first pane at the requested position (else natural), sash, second pane
        let horiz = split.orient == Orientation::Horizontal;
        let ms: Vec<Size> = split_panes(r, id).iter().map(|c| measure(r, *c)).collect();
        let along = |m: &Size| if horiz { m.w } else { m.h };
        let across = |m: &Size| if horiz { m.h } else { m.w };
        let mut main = 0;
        for (i, m) in ms.iter().enumerate() {
            let a = if i == 0 && ms.len() == 2 { split.pos.unwrap_or(along(m)) } else { along(m) };
            main += a.max(if i == 0 { split.min.0 } else { split.min.1 });
        }
        if ms.len() == 2 && split.pos.is_none() {
            // no requested position: layout splits evenly, so both panes need the larger share
            let big = ms.iter().map(along).chain([split.min.0, split.min.1]).max().unwrap_or(0);
            main = 2 * big;
        }
        if ms.len() == 2 {
            main += sp;
        }
        let cross = ms.iter().map(across).max().unwrap_or(0);
        return if horiz { Size::new(main, cross) } else { Size::new(cross, main) };
    }
    if n.kind == Kind::Grid {
        let (_, size, _) = grid_tracks(r, id);
        let tot = |v: &Vec<i32>| v.iter().sum::<i32>() + sp * (v.len() as i32 - 1).max(0);
        return Size::new(tot(&size[0]), tot(&size[1]));
    }
    let kids = visible_children(r, id);
    let (mut main, mut cross) = (0, 0);
    for c in &kids {
        let m = measure(r, *c);
        let (a, b) = if is_horizontal(n.kind) { (m.w, m.h) } else { (m.h, m.w) };
        main += a;
        cross = cross.max(b);
    }
    main += sp * (kids.len() as i32 - 1).max(0);
    if is_horizontal(n.kind) { Size::new(main, cross) } else { Size::new(cross, main) }
}

/// Natural size of a widget (honouring min/fixed size).
pub fn measure(r: &Registry, id: WidgetId) -> Size {
    let Some(n) = r.nodes.get(&id) else { return Size::default() };
    let pad = 2 * n.lay.padding;
    let mut s = match n.kind {
        Kind::HBox | Kind::VBox | Kind::Grid | Kind::Splitter | Kind::Window | Kind::Page | Kind::GroupBox => {
            let c = measure_children(r, id, n);
            let ch = if n.kind == Kind::GroupBox { B::chrome(id) } else { Size::default() };
            Size::new(c.w + pad + ch.w, c.h + pad + ch.h)
        }
        Kind::Tabs => {
            let ch = B::chrome(id);
            let (mut w, mut h) = (0, 0);
            for p in &n.children {
                let m = measure(r, *p);
                w = w.max(m.w);
                h = h.max(m.h);
            }
            Size::new(w + ch.w, h + ch.h)
        }
        Kind::Spacer => Size::default(),
        _ => B::preferred_size(id),
    };
    if let Some(f) = n.lay.fixed {
        s = f;
    }
    Size::new(s.w.max(n.lay.min.w), s.h.max(n.lay.min.h))
}

fn place(r: &mut Registry, id: WidgetId, rect: Rect, out: &mut Out) {
    let Some(n) = r.nodes.get_mut(&id) else { return };
    let kind = n.kind;
    if kind.is_native() && n.bounds != rect {
        n.bounds = rect;
        out.push((id, Prop::Bounds(rect)));
    } else if kind == Kind::Splitter {
        n.bounds = rect; // virtual, but its area is handy (Widget::bounds, sash hit maths)
    }
    match kind {
        Kind::Tabs => {
            let ch = B::chrome(id);
            let inner = Rect::new(0, 0, (rect.w - ch.w).max(0), (rect.h - ch.h).max(0));
            let pages = r.nodes[&id].children.clone();
            for p in pages {
                place(r, p, inner, out);
            }
        }
        Kind::Window | Kind::Page | Kind::GroupBox => {
            let ch = if kind == Kind::GroupBox { B::chrome(id) } else { Size::default() };
            let inner = Rect::new(0, 0, (rect.w - ch.w).max(0), (rect.h - ch.h).max(0));
            arrange_children(r, id, inner, out);
        }
        Kind::HBox | Kind::VBox | Kind::Grid | Kind::Splitter => arrange_children(r, id, rect, out),
        _ => {}
    }
}

/// Offset/size a child of natural size `m` inside `avail` along one axis.
fn aligned(align: Align, fixed: bool, avail: i32, m: i32) -> (i32, i32) {
    let align = if fixed && align == Align::Fill { Align::Start } else { align };
    match align {
        Align::Fill => (0, avail.max(0)),
        Align::Start => (0, m),
        Align::Center => ((avail - m) / 2, m),
        Align::End => (avail - m, m),
    }
}

fn arrange_children(r: &mut Registry, id: WidgetId, area: Rect, out: &mut Out) {
    let n = &r.nodes[&id];
    let (kind, p, sp) = (n.kind, n.lay.padding, n.lay.spacing);
    let inner = Rect::new(area.x + p, area.y + p, (area.w - 2 * p).max(0), (area.h - 2 * p).max(0));
    let mut jobs: Vec<(WidgetId, Rect)> = vec![];
    if kind == Kind::Splitter {
        arrange_split(r, id, inner, &mut jobs, out);
    } else if kind == Kind::Grid {
        let (cells, size, exp) = grid_tracks(r, id);
        let mut size = size;
        for axis in 0..2 {
            let avail = if axis == 0 { inner.w } else { inner.h };
            let used: i32 = size[axis].iter().sum::<i32>() + sp * (size[axis].len() as i32 - 1).max(0);
            distribute(&mut size[axis], &exp[axis], avail - used);
        }
        let origin = |axis: usize, i: usize| -> i32 {
            size[axis][..i].iter().sum::<i32>() + sp * i as i32
        };
        for (c, col, row, cs, rs) in cells {
            let span = |axis: usize, s: usize, i: usize| -> i32 {
                size[axis][i..i + s].iter().sum::<i32>() + sp * (s as i32 - 1)
            };
            let (cw, ch) = (span(0, cs, col), span(1, rs, row));
            let m = measure(r, c);
            let l = &r.nodes[&c].lay;
            let (ox, w) = aligned(l.align, l.fixed.is_some(), cw, m.w);
            let (oy, h) = aligned(l.align, l.fixed.is_some(), ch, m.h);
            jobs.push((c, Rect::new(inner.x + origin(0, col) + ox, inner.y + origin(1, row) + oy, w, h)));
        }
    } else {
        let horiz = is_horizontal(kind);
        let kids = visible_children(r, id);
        let ms: Vec<Size> = kids.iter().map(|c| measure(r, *c)).collect();
        let mut main: Vec<i32> = ms.iter().map(|m| if horiz { m.w } else { m.h }).collect();
        let weights: Vec<f32> = kids.iter().map(|c| r.nodes[c].lay.expand).collect();
        let total: i32 = main.iter().sum::<i32>() + sp * (kids.len() as i32 - 1).max(0);
        distribute(&mut main, &weights, (if horiz { inner.w } else { inner.h }) - total);
        let cross_avail = if horiz { inner.h } else { inner.w };
        let mut pos = if horiz { inner.x } else { inner.y };
        for (i, c) in kids.iter().enumerate() {
            let l = &r.nodes[c].lay;
            let cm = if horiz { ms[i].h } else { ms[i].w };
            let (off, cs) = aligned(l.align, l.fixed.is_some(), cross_avail, cm);
            let rect = if horiz {
                Rect::new(pos, inner.y + off, main[i], cs)
            } else {
                Rect::new(inner.x + off, pos, cs, main[i])
            };
            jobs.push((*c, rect));
            pos += main[i] + sp;
        }
    }
    if rtl() {
        for (_, rc) in jobs.iter_mut() {
            rc.x = inner.x + (inner.x + inner.w - (rc.x + rc.w));
        }
    }
    for (c, rect) in jobs {
        place(r, c, rect, out);
    }
}

/// Splitter: both panes fill the cross axis; the first gets the (clamped) position, the sash
/// `spacing` pixels, the second the rest. With fewer than two visible panes the sash is hidden
/// and a lone pane takes everything. `jobs` are in unmirrored coordinates (the caller mirrors).
fn arrange_split(r: &mut Registry, id: WidgetId, inner: Rect, jobs: &mut Vec<(WidgetId, Rect)>, out: &mut Out) {
    let panes = split_panes(r, id);
    let thick = r.nodes[&id].lay.spacing;
    let Some(sp) = r.nodes.get_mut(&id).and_then(|n| n.split.as_mut()) else { return };
    let horiz = sp.orient == Orientation::Horizontal;
    let sash = sp.sash;
    let two = panes.len() == 2;
    if two {
        sp.thick = thick;
        sp.area = inner;
        let avail = if horiz { inner.w } else { inner.h };
        let want = sp.pos.unwrap_or((avail - thick) / 2);
        let p = sp.clamp(want, avail);
        sp.actual = p;
        sp.laid_out = true;
        let rest = (avail - thick - p).max(0);
        let (a, s, b) = if horiz {
            (
                Rect::new(inner.x, inner.y, p, inner.h),
                Rect::new(inner.x + p, inner.y, thick.min(avail - p), inner.h),
                Rect::new(inner.x + p + thick, inner.y, rest, inner.h),
            )
        } else {
            (
                Rect::new(inner.x, inner.y, inner.w, p),
                Rect::new(inner.x, inner.y + p, inner.w, thick.min(avail - p)),
                Rect::new(inner.x, inner.y + p + thick, inner.w, rest),
            )
        };
        jobs.push((panes[0], a));
        jobs.push((panes[1], b));
        if let Some(s_id) = sash {
            jobs.push((s_id, s));
        }
    } else if let Some(only) = panes.first() {
        jobs.push((*only, inner));
    }
    if let Some((s_id, n)) = sash.and_then(|s| Some(s).zip(r.nodes.get_mut(&s))).filter(|(_, n)| n.visible != two) {
        n.visible = two;
        out.push((s_id, Prop::Visible(two)));
    }
}

/// Give `extra` pixels to entries proportionally to `weights` (no-op if extra <= 0 or no weights).
fn distribute(sizes: &mut [i32], weights: &[f32], extra: i32) {
    let total: f32 = weights.iter().sum();
    if extra <= 0 || total <= 0.0 {
        return;
    }
    let last = weights.iter().rposition(|w| *w > 0.0).unwrap_or(0);
    let mut given = 0;
    for (i, (s, w)) in sizes.iter_mut().zip(weights).enumerate() {
        if *w <= 0.0 {
            continue;
        }
        let share = if i == last { extra - given } else { (extra as f32 * w / total) as i32 };
        *s += share;
        given += share;
    }
}

/// Lay out one window. Returns the `Bounds` updates to push (window size first).
pub fn compute(r: &mut Registry, w: WidgetId) -> Out {
    let mut out = vec![];
    let Some(n) = r.nodes.get(&w) else { return out };
    if n.kind != Kind::Window {
        return out;
    }
    let m = measure(r, w);
    let Some(n) = r.nodes.get_mut(&w) else { return out };
    if !n.explicit_size {
        n.client = m;
    }
    // a window is never laid out below its minimum size (backends may not enforce Prop::MinSize)
    n.client = Size::new(n.client.w.max(n.lay.min.w), n.client.h.max(n.lay.min.h));
    let rect = Rect::new(0, 0, n.client.w, n.client.h);
    place(r, w, rect, &mut out);
    out
}
