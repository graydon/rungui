//! Table (SysListView32, report view) and Tree (SysTreeView32) helpers.

#![forbid(unsafe_code)] // only `api.rs` (and the COM / wndproc code) touches raw FFI

use super::*;

// ------------------------------------------------------------------ table (SysListView32)

pub(super) fn table_set_columns(id: WidgetId, cols: &[Column]) {
    let Some(h) = get(id, |w| w.hwnd) else { return };
    let dpi = dpi_of(id);
    with_w(id, |w| w.cols = cols.to_vec());
    // note: comctl32 always left-aligns the first column, whatever its `fmt`
    while send(h, LVM_DELETECOLUMN, 0, 0) != 0 {}
    for (i, c) in cols.iter().enumerate() {
        let mut title = wide(&c.title);
        let col = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT | LVCF_SUBITEM,
            fmt: match c.align {
                ColumnAlign::Left => LVCFMT_LEFT,
                ColumnAlign::Center => LVCFMT_CENTER,
                ColumnAlign::Right => LVCFMT_RIGHT,
            },
            cx: px(c.width.max(0), dpi),
            pszText: PWSTR(title.as_mut_ptr()),
            iSubItem: i as i32,
            ..Default::default()
        };
        send(h, LVM_INSERTCOLUMNW, i, ptr_arg(&col));
    }
}

pub(super) fn table_set_rows(id: WidgetId, rows: &[Vec<String>]) {
    let Some((h, ncols)) = get(id, |w| (w.hwnd, w.cols.len().max(1))) else {
        return;
    };
    let item_y = |h: HWND| -> Option<i32> {
        if send(h, LVM_GETITEMCOUNT, 0, 0) == 0 {
            return None;
        }
        let mut p = POINT::default();
        (send(h, LVM_GETITEMPOSITION, 0, ptr_arg_mut(&mut p)) != 0).then_some(p.y)
    };
    let before = item_y(h);
    send(h, WM_SETREDRAW, 0, 0);
    send(h, LVM_DELETEALLITEMS, 0, 0);
    send(h, LVM_SETITEMCOUNT, rows.len(), 0);
    for (i, row) in rows.iter().enumerate() {
        for c in 0..ncols {
            let mut text = wide(row.get(c).map_or("", |s| s.as_str()));
            let it = LVITEMW {
                mask: LVIF_TEXT,
                iItem: i as i32,
                iSubItem: c as i32,
                pszText: PWSTR(text.as_mut_ptr()),
                ..Default::default()
            };
            if c == 0 {
                send(h, LVM_INSERTITEMW, 0, ptr_arg(&it));
            } else {
                send(h, LVM_SETITEMTEXTW, i, ptr_arg(&it));
            }
        }
    }
    // keep the scroll position across the rebuild
    if let (Some(b), Some(a)) = (before, item_y(h)) {
        if b != a {
            send(h, LVM_SCROLL, 0, (a - b) as isize);
        }
    }
    send(h, WM_SETREDRAW, 1, 0);
    invalidate(h);
    with_w(id, |w| w.last_sel = None);
}

pub(super) fn table_set_sort(id: WidgetId, sort: Option<(usize, bool)>) {
    let Some((h, n)) = get(id, |w| (w.hwnd, w.cols.len())) else {
        return;
    };
    let hdr = hw(send(h, LVM_GETHEADER, 0, 0));
    if hdr.is_invalid() {
        return;
    }
    for i in 0..n {
        let mut it = HDITEMW {
            mask: HDI_FORMAT,
            ..Default::default()
        };
        if send(hdr, HDM_GETITEMW, i, ptr_arg_mut(&mut it)) == 0 {
            continue;
        }
        it.fmt = HEADER_CONTROL_FORMAT_FLAGS(it.fmt.0 & !(HDF_SORTUP.0 | HDF_SORTDOWN.0));
        if let Some((c, asc)) = sort {
            if c == i {
                let dir = if asc { HDF_SORTUP } else { HDF_SORTDOWN };
                it.fmt = HEADER_CONTROL_FORMAT_FLAGS(it.fmt.0 | dir.0);
            }
        }
        send(hdr, HDM_SETITEMW, i, ptr_arg(&it));
    }
}

/// The table's selection may have changed natively: report it once the notification burst is over
/// (a click deselects the old row and selects the new one in two notifications).
pub(super) fn table_sel_check(id: WidgetId) {
    let Some((h, last)) = get(id, |w| (w.hwnd, w.last_sel)) else {
        return;
    };
    let i = send(h, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    let sel = if i < 0 { None } else { Some(i as usize) };
    if sel != last {
        with_w(id, |w| w.last_sel = sel);
        emit(id, Event::Selected(sel));
    }
}

// ------------------------------------------------------------------ tree (SysTreeView32)

pub(super) fn tree_node_of(h: HWND, item: HTREEITEM) -> Option<u64> {
    let mut it = TVITEMEXW {
        mask: TVIF_PARAM,
        hItem: item,
        ..Default::default()
    };
    (item.0 != 0 && send(h, TVM_GETITEMW, 0, ptr_arg_mut(&mut it)) != 0)
        .then_some(it.lParam.0 as u64)
}

pub(super) fn tree_set_rows(id: WidgetId, rows: &[TreeRow]) {
    let Some(h) = get(id, |w| w.hwnd) else { return };
    let top = vscroll_pos(h);
    send(h, WM_SETREDRAW, 0, 0);
    send(h, TVM_DELETEITEM, 0, TVI_ROOT.0);
    let mut stack: Vec<HTREEITEM> = vec![];
    let mut items: Vec<HTREEITEM> = Vec::with_capacity(rows.len());
    let mut nodes = HashMap::new();
    for r in rows {
        stack.truncate(r.depth as usize);
        let parent = stack.last().copied().unwrap_or(TVI_ROOT);
        let mut text = wide(&r.text);
        let ins = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                itemex: TVITEMEXW {
                    mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN,
                    pszText: PWSTR(text.as_mut_ptr()),
                    cChildren: TVITEMEXW_CHILDREN(r.has_children as i32),
                    lParam: LPARAM(r.node as isize),
                    ..Default::default()
                },
            },
        };
        let it = HTREEITEM(send(h, TVM_INSERTITEMW, 0, ptr_arg(&ins)));
        stack.push(it);
        items.push(it);
        nodes.insert(r.node, it);
    }
    // expand parents before children (pre-order); only nodes with real child rows
    for (i, r) in rows.iter().enumerate() {
        if r.expanded && items[i].0 != 0 && rows.get(i + 1).is_some_and(|n| n.depth > r.depth) {
            send(h, TVM_EXPAND, TVE_EXPAND.0 as usize, items[i].0);
        }
    }
    send(h, WM_SETREDRAW, 1, 0);
    if top > 0 {
        send(
            h,
            WM_VSCROLL,
            SB_THUMBPOSITION.0 as usize | ((top as usize) << 16),
            0,
        );
    }
    invalidate(h);
    with_w(id, |w| {
        w.nodes = nodes;
        w.last_tsel = None;
    });
}

pub(super) fn tree_select(id: WidgetId, node: Option<u64>) {
    let Some((h, item)) = get(id, |w| {
        (w.hwnd, node.and_then(|n| w.nodes.get(&n).copied()))
    }) else {
        return;
    };
    send(
        h,
        TVM_SELECTITEM,
        TVGN_CARET as usize,
        item.unwrap_or_default().0,
    );
    let cur = HTREEITEM(send(h, TVM_GETNEXTITEM, TVGN_CARET as usize, 0));
    let sel = tree_node_of(h, cur);
    with_w(id, |w| w.last_tsel = sel);
}

pub(super) fn tree_activate_selected(id: WidgetId) {
    let Some(h) = get(id, |w| w.hwnd) else { return };
    let cur = HTREEITEM(send(h, TVM_GETNEXTITEM, TVGN_CARET as usize, 0));
    if let Some(n) = tree_node_of(h, cur) {
        emit(id, Event::TreeActivated(n));
    }
}
