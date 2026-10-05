//! Reference-model tests for the table and tree models: random operation sequences are applied
//! to both the real widget (through the mock backend) and a deliberately naive model; every
//! step compares the widget API and what the backend last received.

use crate::backend::mock::{self, widget};
use crate::tests::fuzz_layout::Rng;
use crate::*;

fn init() {
    let _ = App::new("test");
}

// ------------------------------------------------------------------ table

#[derive(Default, Clone, Debug, PartialEq)]
struct TModel {
    ncols: usize,
    rows: Vec<Vec<String>>,
    sel: Option<usize>,
    sort: Option<(usize, bool)>,
}

fn word(rng: &mut Rng) -> String {
    ["", "a", "b", "ünï", "日本", "x y", "😀", "&&", "z"][rng.below(9)].to_string()
}

fn row(rng: &mut Rng) -> Vec<String> {
    (0..rng.below(4)).map(|_| word(rng)).collect()
}

fn table_op(rng: &mut Rng, t: Table, m: &mut TModel) -> String {
    match rng.below(13) {
        0 => {
            let r = row(rng);
            t.push_row(&r);
            m.rows.push(r);
            "push".into()
        }
        1 => {
            let i = rng.below(m.rows.len() + 3);
            let r = row(rng);
            t.insert_row(i, &r);
            let at = i.min(m.rows.len());
            m.rows.insert(at, r);
            if let Some(s) = m.sel.as_mut() {
                if *s >= at {
                    *s += 1;
                }
            }
            format!("insert {i}")
        }
        2 => {
            let i = rng.below(m.rows.len() + 2);
            t.remove_row(i);
            if i < m.rows.len() {
                m.rows.remove(i);
                m.sel = match m.sel {
                    Some(s) if s == i => None,
                    Some(s) if s > i => Some(s - 1),
                    o => o,
                };
            }
            format!("remove {i}")
        }
        3 => {
            let (r, c) = (rng.below(m.rows.len() + 2), rng.below(6));
            let w = word(rng);
            t.set_cell(r, c, &w);
            if let Some(row) = m.rows.get_mut(r) {
                if row.len() <= c {
                    row.resize(c + 1, String::new());
                }
                row[c] = w;
            }
            format!("set_cell {r} {c}")
        }
        4 => {
            let rows: Vec<Vec<String>> = (0..rng.below(6)).map(|_| row(rng)).collect();
            t.set_rows(&rows);
            m.rows = rows;
            if m.sel.is_some_and(|i| i >= m.rows.len()) {
                m.sel = None;
            }
            "set_rows".into()
        }
        5 => {
            t.clear();
            m.rows.clear();
            m.sel = None;
            "clear".into()
        }
        6 => {
            let s = if rng.chance(20) {
                None
            } else {
                Some(rng.below(m.rows.len() + 3))
            };
            t.set_selected(s);
            m.sel = s.filter(|i| *i < m.rows.len());
            format!("set_selected {s:?}")
        }
        7 => {
            let s = if rng.chance(30) {
                None
            } else {
                Some(rng.below(m.ncols + 2))
            };
            let v = s.map(|c| (c, rng.chance(50)));
            t.set_sort_indicator(v);
            m.sort = v.filter(|(c, _)| *c < m.ncols);
            format!("sort {v:?}")
        }
        8 => {
            let n = rng.below(5);
            let cols: Vec<Column> = (0..n).map(|i| Column::new(&format!("c{i}"))).collect();
            t.set_columns(&cols);
            m.ncols = n;
            if m.sort.is_some_and(|(c, _)| c >= n) {
                m.sort = None;
            }
            format!("set_columns {n}")
        }
        9 => {
            t.add_column(Column::new("extra"));
            m.ncols += 1;
            "add_column".into()
        }
        10 => {
            if !m.rows.is_empty() {
                let i = rng.below(m.rows.len());
                mock::user_select_row(t.id(), Some(i));
                m.sel = Some(i);
            } else {
                mock::user_select_row(t.id(), None);
                m.sel = None;
            }
            "user_select".into()
        }
        11 => {
            // out-of-range user events must be ignored
            let before = t.selected();
            mock::user_select_row(t.id(), Some(m.rows.len() + rng.below(5)));
            assert_eq!(t.selected(), before);
            mock::user_activate_row(t.id(), m.rows.len());
            mock::user_click_column(t.id(), m.ncols + 1);
            t.set_cell(usize::MAX, 0, ""); // no-op that re-pushes the model (the mock mirrored the bogus selection)
            "stale user events".into()
        }
        _ => {
            let mut inner = m.clone();
            let r = row(rng);
            let rr = r.clone();
            t.batch(|t| {
                t.push_row(&rr);
                t.remove_row(0);
                t.push_row(&rr);
            });
            inner.rows.push(r.clone());
            if !inner.rows.is_empty() {
                // remove_row(0) semantics
                inner.rows.remove(0);
                inner.sel = match inner.sel {
                    Some(0) => None,
                    Some(s) => Some(s - 1),
                    None => None,
                };
            }
            inner.rows.push(r);
            *m = inner;
            "batch".into()
        }
    }
}

#[test]
fn table_matches_reference_model() {
    init();
    for seed in 1..=200u64 {
        let mut rng = Rng::new(seed);
        let win = Window::new("w");
        let t = Table::new(win);
        win.show();
        let mut m = TModel::default();
        let mut log = vec![];
        for _ in 0..60 {
            log.push(table_op(&mut rng, t, &mut m));
            App::update();
            let ctx = || format!("seed {seed}, ops {log:?}");
            assert_eq!(t.rows(), m.rows, "{}", ctx());
            assert_eq!(t.row_count(), m.rows.len(), "{}", ctx());
            assert_eq!(t.selected(), m.sel, "{}", ctx());
            assert_eq!(t.sort_indicator(), m.sort, "{}", ctx());
            assert_eq!(t.columns().len(), m.ncols, "{}", ctx());
            assert_eq!(
                t.selected_row(),
                m.sel.map(|i| m.rows[i].clone()),
                "{}",
                ctx()
            );
            let w = widget(t.id()).unwrap();
            assert_eq!(w.rows, m.rows, "backend rows: {}", ctx());
            assert_eq!(w.selected, m.sel, "backend selection: {}", ctx());
            assert_eq!(w.sort, m.sort, "backend sort: {}", ctx());
            assert_eq!(w.columns.len(), m.ncols, "backend columns: {}", ctx());
            for (i, r) in m.rows.iter().enumerate() {
                for (c, v) in r.iter().enumerate() {
                    assert_eq!(t.cell(i, c), *v);
                }
                assert_eq!(t.cell(i, 99), "");
            }
            assert_eq!(t.cell(m.rows.len(), 0), "");
        }
        win.destroy();
    }
}

// ------------------------------------------------------------------ tree

#[derive(Clone, Debug)]
struct MNode {
    id: u64,
    text: String,
    expanded: bool,
    lazy: bool,
    kids: Vec<MNode>,
}

#[derive(Default)]
struct TrModel {
    roots: Vec<MNode>,
    sel: Option<u64>,
}

fn find(l: &mut [MNode], id: u64) -> Option<&mut MNode> {
    for n in l.iter_mut() {
        if n.id == id {
            return Some(n);
        }
        if let Some(f) = find(&mut n.kids, id) {
            return Some(f);
        }
    }
    None
}

fn ids(l: &[MNode], out: &mut Vec<u64>) {
    for n in l {
        out.push(n.id);
        ids(&n.kids, out);
    }
}

fn flat(l: &[MNode], depth: u32, out: &mut Vec<(u64, u32, String, bool, bool)>) {
    for n in l {
        out.push((
            n.id,
            depth,
            n.text.clone(),
            n.expanded,
            n.lazy || !n.kids.is_empty(),
        ));
        flat(&n.kids, depth + 1, out);
    }
}

/// Remove `id` (and its subtree) wherever it is; true if found.
fn detach(l: &mut Vec<MNode>, id: u64) -> bool {
    if let Some(i) = l.iter().position(|n| n.id == id) {
        l.remove(i);
        return true;
    }
    l.iter_mut().any(|n| detach(&mut n.kids, id))
}

/// Expand all ancestors of `id`.
fn reveal(l: &mut [MNode], id: u64) -> bool {
    for n in l.iter_mut() {
        if n.id == id {
            return true;
        }
        if reveal(&mut n.kids, id) {
            n.expanded = true;
            return true;
        }
    }
    false
}

fn contains(l: &[MNode], id: u64) -> bool {
    l.iter().any(|n| n.id == id || contains(&n.kids, id))
}

fn pick(rng: &mut Rng, m: &TrModel, dead: &[u64]) -> u64 {
    let mut all = vec![];
    ids(&m.roots, &mut all);
    match rng.below(10) {
        0 if !dead.is_empty() => dead[rng.below(dead.len())],
        1 => 0,
        _ if all.is_empty() => 0,
        _ => all[rng.below(all.len())],
    }
}

fn tree_op(rng: &mut Rng, t: Tree, m: &mut TrModel, dead: &mut Vec<u64>) -> String {
    match rng.below(14) {
        0..=3 => {
            let parent = if rng.chance(25) {
                None
            } else {
                Some(pick(rng, m, dead))
            };
            let idx = if rng.chance(30) {
                usize::MAX
            } else {
                rng.below(5)
            };
            let w = word(rng);
            let id = t.insert(parent.map(TreeNodeId), idx, &w).0;
            let list = match parent {
                None => Some(&mut m.roots),
                Some(p) => find(&mut m.roots, p).map(|n| &mut n.kids),
            };
            match list {
                Some(l) => {
                    assert_ne!(id, 0);
                    let at = idx.min(l.len());
                    l.insert(
                        at,
                        MNode {
                            id,
                            text: w,
                            expanded: false,
                            lazy: false,
                            kids: vec![],
                        },
                    );
                }
                None => assert_eq!(id, 0, "unknown parent must return 0"),
            }
            format!("insert {parent:?} {idx}")
        }
        4 | 5 => {
            let id = pick(rng, m, dead);
            let mut sub = vec![];
            if let Some(n) = find(&mut m.roots, id) {
                ids(std::slice::from_ref(&*n), &mut sub);
            }
            t.remove(TreeNodeId(id));
            if detach(&mut m.roots, id) {
                dead.extend(sub.iter().copied());
                if m.sel.is_some_and(|s| sub.contains(&s)) {
                    m.sel = None;
                }
            }
            format!("remove {id}")
        }
        6 => {
            let id = pick(rng, m, dead);
            let w = word(rng);
            t.set_text(TreeNodeId(id), &w);
            if let Some(n) = find(&mut m.roots, id) {
                n.text = w;
            }
            format!("set_text {id}")
        }
        7 => {
            let (id, v) = (pick(rng, m, dead), rng.chance(50));
            t.set_expanded(TreeNodeId(id), v);
            if let Some(n) = find(&mut m.roots, id) {
                n.expanded = v;
            }
            format!("set_expanded {id} {v}")
        }
        8 => {
            let v = rng.chance(50);
            t.expand_all(v);
            fn all(l: &mut [MNode], v: bool) {
                for n in l {
                    n.expanded = v;
                    all(&mut n.kids, v);
                }
            }
            all(&mut m.roots, v);
            format!("expand_all {v}")
        }
        9 => {
            let (id, v) = (pick(rng, m, dead), rng.chance(50));
            t.set_has_children(TreeNodeId(id), v);
            if let Some(n) = find(&mut m.roots, id) {
                n.lazy = v;
            }
            format!("lazy {id} {v}")
        }
        10 => {
            let id = if rng.chance(15) {
                None
            } else {
                Some(pick(rng, m, dead))
            };
            t.set_selected(id.map(TreeNodeId));
            match id {
                Some(i) if contains(&m.roots, i) => {
                    reveal(&mut m.roots, i);
                    m.sel = Some(i);
                }
                _ => m.sel = None,
            }
            format!("select {id:?}")
        }
        11 => {
            t.clear();
            let mut all = vec![];
            ids(&m.roots, &mut all);
            dead.extend(all);
            m.roots.clear();
            m.sel = None;
            "clear".into()
        }
        12 => {
            // user events: expand/select on live nodes, and stale ones that must be ignored
            let id = pick(rng, m, dead);
            let live = contains(&m.roots, id);
            let v = rng.chance(50);
            mock::user_tree_expand(t.id(), id, v);
            mock::user_tree_select(t.id(), Some(id));
            if live {
                find(&mut m.roots, id).unwrap().expanded = v;
                m.sel = Some(id);
            } else {
                t.set_has_children(TreeNodeId(0), false); // re-push: the mock mirrored the stale selection
            }
            format!("user {id} {v}")
        }
        _ => {
            let id = pick(rng, m, dead);
            let w = word(rng);
            t.batch(|t| {
                t.set_text(TreeNodeId(id), &w);
                t.add(Some(TreeNodeId(id)), "batched");
            });
            let mut kid = None;
            if let Some(n) = find(&mut m.roots, id) {
                n.text = w;
                // new id is unknown to the model: read it back from the tree
                kid = Some(());
            }
            if kid.is_some() {
                let last = t
                    .children(Some(TreeNodeId(id)))
                    .last()
                    .expect("batched child")
                    .0;
                find(&mut m.roots, id).unwrap().kids.push(MNode {
                    id: last,
                    text: "batched".into(),
                    expanded: false,
                    lazy: false,
                    kids: vec![],
                });
            }
            format!("batch {id}")
        }
    }
}

#[test]
fn tree_matches_reference_model() {
    init();
    for seed in 1..=200u64 {
        let mut rng = Rng::new(7000 + seed);
        let win = Window::new("w");
        let t = Tree::new(win);
        win.show();
        let (mut m, mut dead, mut log) = (TrModel::default(), vec![], vec![]);
        for _ in 0..60 {
            log.push(tree_op(&mut rng, t, &mut m, &mut dead));
            App::update();
            let ctx = || format!("seed {seed}, ops {log:?}");
            let mut want = vec![];
            flat(&m.roots, 0, &mut want);
            let got: Vec<_> = widget(t.id())
                .unwrap()
                .tree_rows
                .iter()
                .map(|r| (r.node, r.depth, r.text.clone(), r.expanded, r.has_children))
                .collect();
            assert_eq!(got, want, "backend rows: {}", ctx());
            assert_eq!(
                widget(t.id()).unwrap().tree_selected,
                m.sel,
                "backend selection: {}",
                ctx()
            );
            assert_eq!(t.selected().map(|n| n.0), m.sel, "{}", ctx());
            assert_eq!(t.len(), want.len(), "{}", ctx());
            assert_eq!(t.is_empty(), want.is_empty());
            let roots: Vec<u64> = m.roots.iter().map(|n| n.id).collect();
            assert_eq!(
                t.children(None).iter().map(|n| n.0).collect::<Vec<_>>(),
                roots,
                "{}",
                ctx()
            );
            for (id, depth, text, expanded, _) in &want {
                let n = TreeNodeId(*id);
                assert!(t.contains(n));
                assert_eq!(t.text(n), *text);
                assert_eq!(t.expanded(n), *expanded);
                let mut d = 0;
                let mut cur = t.parent(n);
                while let Some(p) = cur {
                    d += 1;
                    assert!(t.contains(p));
                    cur = t.parent(p);
                }
                assert_eq!(d, *depth, "{}", ctx());
            }
            for d in &dead {
                assert!(
                    !t.contains(TreeNodeId(*d)),
                    "dead node resurrected: {}",
                    ctx()
                );
            }
            if let Some(s) = m.sel {
                // a selected node set programmatically has all ancestors expanded; user selection
                // of a hidden node is allowed, so only check it exists
                assert!(t.contains(TreeNodeId(s)));
            }
        }
        win.destroy();
    }
}
