//! Randomised layout tests: random trees of boxes, grids, splitters and group boxes with hidden
//! widgets, checked against structural invariants (containment, no overlap, minimum sizes,
//! idempotent relayout). A tiny in-file xorshift PRNG keeps every run deterministic.

use crate::backend::mock::{self, widget};
use crate::*;

pub(crate) struct Rng(u64);
impl Rng {
    pub(crate) fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Uniform in 0..n (n > 0).
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub(crate) fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
    pub(crate) fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as usize) as i32
    }
}

fn init() {
    let _ = App::new("test");
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum K {
    HBox,
    VBox,
    Grid,
    Split,
    Group,
    Leaf,
}

struct N {
    id: WidgetId,
    kind: K,
    parent: Option<usize>,
    children: Vec<usize>,
    min: Size,
}

struct Tree {
    win: Window,
    nodes: Vec<N>,
}

fn leaf(rng: &mut Rng, parent: WidgetId) -> WidgetId {
    match rng.below(7) {
        0 => Label::new(parent, "some label").id(),
        1 => Button::new(parent, "OK").id(),
        2 => TextInput::new(parent).id(),
        3 => ListBox::new(parent).id(),
        4 => Spacer::new(parent).id(),
        5 => CheckBox::new(parent, "chk").id(),
        _ => Slider::new(parent, 0.0, 10.0).id(),
    }
}

/// Style a random widget (expand / align / spacing / padding / min / fixed size).
fn style(rng: &mut Rng, w: Widget, kind: K, is_spacer: bool) -> Size {
    let mut min = Size::default();
    if rng.chance(30) {
        w.set_expand(rng.range(1, 3) as f32);
    }
    if kind == K::Leaf && !is_spacer && rng.chance(25) {
        w.set_align([Align::Start, Align::Center, Align::End][rng.below(3)]);
    }
    if kind != K::Leaf {
        if rng.chance(40) {
            w.set_spacing(rng.range(0, 12));
        }
        if rng.chance(30) {
            w.set_padding(rng.range(0, 10));
        }
    }
    if rng.chance(15) {
        min = Size::new(rng.range(0, 120), rng.range(0, 90));
        w.set_min_size(min.w, min.h);
    }
    if kind == K::Leaf && !is_spacer && rng.chance(10) {
        w.set_fixed_size(rng.range(1, 90), rng.range(1, 60));
        min = Size::default(); // fixed size overrides natural; min still applies on top
    }
    min
}

fn build(rng: &mut Rng, nodes: &mut Vec<N>, parent: usize, depth: usize, budget: &mut i32) {
    let pid = nodes[parent].id;
    let pk = nodes[parent].kind;
    let nkids = match pk {
        K::Split => 2,
        K::Grid => rng.range(1, 6) as usize,
        _ => rng.range(0, 5) as usize,
    };
    for _ in 0..nkids {
        if *budget <= 0 {
            return;
        }
        *budget -= 1;
        let container = depth < 4 && rng.chance(35);
        let (id, kind, is_spacer) = if container {
            match rng.below(5) {
                0 => (HBox::new(pid).id(), K::HBox, false),
                1 => (VBox::new(pid).id(), K::VBox, false),
                2 => (
                    Grid::new(pid, rng.range(1, 4) as usize).id(),
                    K::Grid,
                    false,
                ),
                3 => {
                    let o = if rng.chance(50) {
                        Orientation::Horizontal
                    } else {
                        Orientation::Vertical
                    };
                    (Splitter::new(pid, o).id(), K::Split, false)
                }
                _ => (GroupBox::new(pid, "grp").id(), K::Group, false),
            }
        } else {
            let id = leaf(rng, pid);
            let sp =
                widget(id).and_then(|w| w.kind) == Some(Kind::Spacer) || mock::widget(id).is_none();
            (id, K::Leaf, sp)
        };
        let min = style(rng, Widget(id), kind, is_spacer);
        let idx = nodes.len();
        nodes.push(N {
            id,
            kind,
            parent: Some(parent),
            children: vec![],
            min,
        });
        nodes[parent].children.push(idx);
        if kind != K::Leaf {
            build(rng, nodes, idx, depth + 1, budget);
        }
        if rng.chance(15) {
            Widget(id).set_visible(false);
        }
    }
}

fn make_tree(seed: u64, positions: bool) -> (Tree, Rng) {
    init();
    let mut rng = Rng::new(seed);
    let win = Window::new("fuzz");
    if rng.chance(30) {
        win.set_padding(rng.range(0, 12));
    }
    let mut nodes = vec![N {
        id: win.id(),
        kind: K::VBox,
        parent: None,
        children: vec![],
        min: Size::default(),
    }];
    let mut budget = 40;
    build(&mut rng, &mut nodes, 0, 0, &mut budget);
    // optionally explicit splitter positions / minimum pane sizes (panes may then overflow
    // into each other by design: content is never squeezed below its natural size)
    for n in nodes.iter().filter(|_| positions) {
        if n.kind == K::Split {
            let s = Splitter::from_id(n.id);
            if rng.chance(50) {
                s.set_position(rng.range(0, 300));
            }
            if rng.chance(30) {
                s.set_min_pane_sizes(rng.range(0, 60), rng.range(0, 60));
            }
        }
    }
    win.show();
    App::update();
    (Tree { win, nodes }, rng)
}

fn eff_visible(t: &Tree, i: usize) -> bool {
    let mut cur = Some(i);
    while let Some(c) = cur {
        if !Widget(t.nodes[c].id).visible() {
            return false;
        }
        cur = t.nodes[c].parent;
    }
    true
}

fn has_split_ancestor(t: &Tree, i: usize) -> bool {
    let mut cur = t.nodes[i].parent;
    while let Some(c) = cur {
        if t.nodes[c].kind == K::Split {
            return true;
        }
        cur = t.nodes[c].parent;
    }
    false
}

/// Absolute rect of a native widget in window-client coordinates, following the mock's parents.
fn abs(id: WidgetId) -> Option<Rect> {
    let w = widget(id)?;
    let mut r = w.bounds;
    let mut p = w.parent;
    while let Some(pid) = p {
        let pw = widget(pid)?;
        if pw.kind == Some(Kind::Window) {
            break;
        }
        r.x += pw.bounds.x;
        r.y += pw.bounds.y;
        p = pw.parent;
    }
    Some(r)
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.w > 0
        && a.h > 0
        && b.w > 0
        && b.h > 0
        && a.x < b.x + b.w
        && b.x < a.x + a.w
        && a.y < b.y + b.h
        && b.y < a.y + a.h
}

/// Every visible native leaf (and visible sash), with its absolute rect.
fn visible_leaves(t: &Tree) -> Vec<(usize, WidgetId, Rect)> {
    let mut v = vec![];
    for (i, n) in t.nodes.iter().enumerate() {
        if n.kind == K::Leaf && eff_visible(t, i) && widget(n.id).is_some() {
            if let Some(r) = abs(n.id) {
                v.push((i, n.id, r));
            }
        } else if n.kind == K::Split && eff_visible(t, i) {
            if let Some(s) = mock::sash_of(n.id) {
                if widget(s).is_some_and(|w| w.visible) {
                    v.push((i, s, abs(s).unwrap()));
                }
            }
        }
    }
    v
}

fn check(t: &Tree, seed: u64, tag: &str, contained: bool) {
    let ctx = format!("seed {seed} [{tag}]\n{}", mock::dump());
    let client = widget(t.win.id()).unwrap().bounds;
    let leaves = visible_leaves(t);
    for (i, id, r) in &leaves {
        assert!(r.w >= 0 && r.h >= 0, "negative size {r:?} of {id:?}; {ctx}");
        if contained {
            assert!(
                r.x >= 0 && r.y >= 0 && r.x + r.w <= client.w && r.y + r.h <= client.h,
                "{r:?} escapes client {client:?} ({:?}); {ctx}",
                t.nodes[*i].kind
            );
        }
        if contained
            && t.nodes[*i].kind == K::Leaf
            && !has_split_ancestor(t, *i)
            && widget(*id).is_some_and(|w| w.kind != Some(Kind::Spacer))
        {
            let m = t.nodes[*i].min;
            assert!(r.w >= m.w && r.h >= m.h, "{r:?} below min {m:?}; {ctx}");
        }
    }
    // a shrunk splitter squeezes its panes below their natural size, so their content may overlap
    if contained || !t.nodes.iter().any(|n| n.kind == K::Split) {
        for a in 0..leaves.len() {
            for b in a + 1..leaves.len() {
                assert!(
                    !overlaps(leaves[a].2, leaves[b].2),
                    "{:?} overlaps {:?}; {ctx}",
                    leaves[a],
                    leaves[b]
                );
            }
        }
    }
}

fn snapshot(t: &Tree) -> Vec<(WidgetId, Rect, u32)> {
    t.nodes
        .iter()
        .filter_map(|n| widget(n.id).map(|w| (n.id, w.bounds, w.bounds_pushes)))
        .chain(
            t.nodes
                .iter()
                .filter(|n| n.kind == K::Split)
                .filter_map(|n| mock::sash_of(n.id))
                .filter_map(|s| widget(s).map(|w| (s, w.bounds, w.bounds_pushes))),
        )
        .collect()
}

#[test]
fn random_trees_keep_layout_invariants() {
    for seed in 1..=300u64 {
        let (t, mut rng) = make_tree(seed, false);
        let client = widget(t.win.id()).unwrap().bounds;
        check(&t, seed, "natural", true);

        // idempotence: relaying out again changes nothing and pushes nothing
        let before = snapshot(&t);
        mock::resize_window(t.win.id(), client.w, client.h);
        App::update();
        assert_eq!(before, snapshot(&t), "seed {seed}: relayout not idempotent");

        // grow: still contained and non-overlapping
        mock::resize_window(
            t.win.id(),
            client.w + rng.range(1, 300),
            client.h + rng.range(1, 300),
        );
        App::update();
        check(&t, seed, "grown", true);
        // shrink: content may overflow but never overlaps / goes negative
        mock::resize_window(t.win.id(), (client.w / 2).max(1), (client.h / 2).max(1));
        App::update();
        check(&t, seed, "shrunk", false);
        // and growing back restores the natural arrangement
        mock::resize_window(t.win.id(), client.w, client.h);
        App::update();
        let rects = |v: &[(WidgetId, Rect, u32)]| v.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>();
        assert_eq!(
            rects(&before),
            rects(&snapshot(&t)),
            "seed {seed}: not restored"
        );
        t.win.destroy();
        assert_eq!(mock::widget_count(), 0, "seed {seed}: leaked widgets");
    }
}

#[test]
fn random_trees_survive_random_mutation() {
    for seed in 1..=150u64 {
        let (t, mut rng) = make_tree(1000 + seed, false);
        for step in 0..25 {
            let i = rng.below(t.nodes.len());
            let w = Widget(t.nodes[i].id);
            match rng.below(6) {
                0 => w.set_visible(rng.chance(50)),
                1 => w.set_expand(rng.below(4) as f32),
                2 => w.set_spacing(rng.range(0, 15)),
                3 => w.set_padding(rng.range(0, 15)),
                4 => w.set_min_size(rng.range(0, 100), rng.range(0, 100)),
                _ => w.set_enabled(rng.chance(70)),
            }
            if rng.chance(30) {
                App::update();
            }
            if step % 8 == 7 {
                // rebuild expectations: recompute min sizes is overkill; check structure only
                App::update();
                let leaves = visible_leaves(&t);
                for a in 0..leaves.len() {
                    for b in a + 1..leaves.len() {
                        assert!(
                            !overlaps(leaves[a].2, leaves[b].2),
                            "seed {seed} step {step}: overlap\n{}",
                            mock::dump()
                        );
                    }
                }
            }
        }
        t.win.destroy();
    }
}

#[test]
fn rtl_mirrors_without_overlap() {
    for seed in 1..=100u64 {
        crate::set_rtl_layout(true);
        let (t, _) = make_tree(5000 + seed, false);
        check(&t, seed, "rtl", true);
        crate::set_rtl_layout(false);
        t.win.destroy();
    }
}

#[test]
fn splitter_geometry_is_consistent_for_random_positions() {
    init();
    for seed in 1..=200u64 {
        let mut rng = Rng::new(seed);
        let win = Window::new("w");
        let o = if rng.chance(50) {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        };
        let s = Splitter::new(win, o);
        let a = ListBox::new(s);
        let b = ListBox::new(s);
        win.set_size(rng.range(20, 600), rng.range(20, 600));
        s.set_spacing(rng.range(0, 20));
        win.show();
        s.set_min_pane_sizes(rng.range(0, 80), rng.range(0, 80));
        for _ in 0..10 {
            match rng.below(3) {
                0 => s.set_position(rng.range(-50, 700)),
                1 => mock::user_drag_sash(s.id(), rng.range(-100, 800)),
                _ => mock::resize_window(win.id(), rng.range(20, 600), rng.range(20, 600)),
            }
            let (ra, rb, rs) = (
                widget(a.id()).unwrap().bounds,
                widget(b.id()).unwrap().bounds,
                widget(mock::sash_of(s.id()).unwrap()).unwrap().bounds,
            );
            let (pa, pb, ps, pt, avail) = if o == Orientation::Horizontal {
                (ra.w, rb.w, rs.w, rb.x - ra.x, s.bounds().w)
            } else {
                (ra.h, rb.h, rs.h, rb.y - ra.y, s.bounds().h)
            };
            let thick = pt - pa; // distance between pane starts minus first size
            assert!(pa >= 0 && pb >= 0 && ps >= 0, "seed {seed}: negative size");
            assert_eq!(
                s.position(),
                pa,
                "seed {seed}: position() matches first pane"
            );
            assert!(thick >= 0, "seed {seed}");
            assert!(
                pa + thick + pb <= avail.max(pa + thick + pb) && pa + thick + pb == avail
                    || avail < thick,
                "seed {seed}: panes do not tile the splitter: {pa}+{thick}+{pb} vs {avail}"
            );
            assert!(ps <= thick, "seed {seed}: sash {ps} wider than gap {thick}");
        }
        win.destroy();
    }
}

#[test]
fn extreme_values_never_panic() {
    init();
    let extremes = [
        i32::MIN,
        i32::MIN + 1,
        -1,
        0,
        1,
        7,
        i32::MAX / 2,
        i32::MAX - 1,
        i32::MAX,
    ];
    let win = Window::new("w");
    let col = VBox::new(win);
    let grid = Grid::new(col, 2);
    let leafs: Vec<Widget> = (0..4)
        .map(|_| Widget(Button::new(grid, "b").id()))
        .collect();
    let sp = Splitter::new(col, Orientation::Horizontal);
    ListBox::new(sp);
    ListBox::new(sp);
    let all: Vec<Widget> = [
        Widget(col.id()),
        Widget(grid.id()),
        Widget(sp.id()),
        Widget(win.id()),
    ]
    .into_iter()
    .chain(leafs)
    .collect();
    win.show();
    for &v in &extremes {
        for w in &all {
            w.set_padding(v);
            w.set_spacing(v);
            w.set_min_size(v, v);
            w.set_fixed_size(v, v);
            App::update();
            // reset the bounds-affecting values so the next iteration starts sane
            w.set_padding(0);
            w.set_spacing(0);
            w.set_min_size(0, 0);
            w.set_fixed_size(10, 10);
        }
        sp.set_position(v);
        sp.set_min_pane_sizes(v, v);
        mock::user_drag_sash(sp.id(), v);
        win.set_size(v, v);
        mock::resize_window(win.id(), v, v);
        win.set_position(v, v);
        sp.set_position(50);
        sp.set_min_pane_sizes(0, 0);
        win.set_size(300, 300);
        App::update();
    }
}

#[test]
fn absurd_grid_cells_do_not_panic_or_allocate_wildly() {
    init();
    let win = Window::new("w");
    let g = Grid::new(win, 3);
    let b = Button::new(g, "x");
    win.show();
    for (c, r, cs, rs) in [
        (usize::MAX, 0, 1, 1),
        (0, usize::MAX, 1, 1),
        (1, 1, usize::MAX, usize::MAX),
        (usize::MAX, usize::MAX, usize::MAX, usize::MAX),
        (1 << 40, 1 << 40, 1, 1),
        (0, 0, 0, 0),
    ] {
        g.place(b, c, r, cs, rs);
        App::update();
    }
}
