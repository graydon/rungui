//! Multi-column `Table`.

use super::*;

/// Calls `core::freeze(id, false)` on drop so a panicking batch closure cannot leave it frozen.
pub(super) struct Thaw(pub(super) WidgetId);
impl Drop for Thaw {
    fn drop(&mut self) {
        core::freeze(self.0, false)
    }
}

fn cells<S: AsRef<str>>(c: &[S]) -> Vec<String> {
    c.iter().map(|s| s.as_ref().to_string()).collect()
}

impl Table {
    /// An empty table without columns.
    pub fn new(parent: impl Into<WidgetId>) -> Table {
        make(Table::from_id, Kind::Table, parent, |_| {})
    }
    /// Replace the columns (rows and selection are kept). At most [`MAX_COLUMNS`] are kept.
    pub fn set_columns(&self, cols: &[Column]) {
        let v = cols[..cols.len().min(MAX_COLUMNS)].to_vec();
        core::data_update(self.id(), core::Data::TableAll, |n| {
            if let Some(t) = n.table_mut() {
                t.columns = v;
                if t.sort.is_some_and(|(c, _)| c >= t.columns.len()) {
                    t.sort = None;
                }
            }
        });
    }
    /// Append a column.
    pub fn add_column(&self, col: Column) {
        core::data_update(self.id(), core::Data::TableAll, |n| {
            if let Some(t) = n.table_mut().filter(|t| t.columns.len() < MAX_COLUMNS) {
                t.columns.push(col);
            }
        });
    }
    /// The columns.
    pub fn columns(&self) -> Vec<Column> {
        core::read(self.id(), |n| n.table().map(|t| t.columns.clone()))
            .flatten()
            .unwrap_or_default()
    }
    /// Replace all rows. The selection is cleared if it is now out of range.
    pub fn set_rows<S: AsRef<str>>(&self, rows: &[Vec<S>]) {
        let v: Vec<Vec<String>> = rows.iter().map(|r| cells(r)).collect();
        core::data_update(self.id(), core::Data::TableRows, |n| {
            if let Some(t) = n.table_mut() {
                t.rows = v;
                if t.selected.is_some_and(|i| i >= t.rows.len()) {
                    t.selected = None;
                }
            }
        });
    }
    /// Append a row (missing cells are empty).
    pub fn push_row<S: AsRef<str>>(&self, row: &[S]) {
        let v = cells(row);
        core::data_update(self.id(), core::Data::TableRows, |n| {
            if let Some(t) = n.table_mut() {
                t.rows.push(v);
            }
        });
    }
    /// Insert before `index` (clamped); the selection follows its row.
    pub fn insert_row<S: AsRef<str>>(&self, index: usize, row: &[S]) {
        let v = cells(row);
        core::data_update(self.id(), core::Data::TableRows, |n| {
            if let Some(t) = n.table_mut() {
                let i = index.min(t.rows.len());
                t.rows.insert(i, v);
                if let Some(s) = t.selected.as_mut() {
                    if *s >= i {
                        *s += 1;
                    }
                }
            }
        });
    }
    /// Remove a row (ignored if out of range); the selection follows its row or clears.
    pub fn remove_row(&self, index: usize) {
        core::data_update(self.id(), core::Data::TableRows, |n| {
            if let Some(t) = n.table_mut() {
                if index < t.rows.len() {
                    t.rows.remove(index);
                    t.selected = match t.selected {
                        Some(s) if s == index => None,
                        Some(s) if s > index => Some(s - 1),
                        o => o,
                    };
                }
            }
        });
    }
    /// Remove every row.
    pub fn clear(&self) {
        core::data_update(self.id(), core::Data::TableRows, |n| {
            if let Some(t) = n.table_mut() {
                t.rows.clear();
                t.selected = None;
            }
        });
    }
    /// Set one cell (the row is padded with empty cells if needed; out-of-range rows are ignored).
    /// Note: this shadows the grid-placement `Widget::set_cell`, reachable as `(*table).set_cell(..)`.
    pub fn set_cell(&self, row: usize, col: usize, text: &str) {
        core::data_update(self.id(), core::Data::TableCells, |n| {
            let Some(t) = n.table_mut() else { return };
            let Some(r) = t.rows.get_mut(row) else { return };
            if col >= MAX_COLUMNS {
                return; // absurd column index: would allocate gigabytes
            }
            if r.len() <= col {
                r.resize(col + 1, String::new());
            }
            r[col] = text.to_string();
            t.batch.cells.push((row, col));
        });
    }
    /// The text of one cell (empty when out of range).
    pub fn cell(&self, row: usize, col: usize) -> String {
        core::read(self.id(), |n| n.table()?.rows.get(row)?.get(col).cloned())
            .flatten()
            .unwrap_or_default()
    }
    /// The cells of a row (empty when out of range).
    pub fn row(&self, row: usize) -> Vec<String> {
        core::read(self.id(), |n| n.table()?.rows.get(row).cloned())
            .flatten()
            .unwrap_or_default()
    }
    /// Every row.
    pub fn rows(&self) -> Vec<Vec<String>> {
        core::read(self.id(), |n| n.table().map(|t| t.rows.clone()))
            .flatten()
            .unwrap_or_default()
    }
    /// How many rows there are.
    pub fn row_count(&self) -> usize {
        core::read(self.id(), |n| n.table().map_or(0, |t| t.rows.len())).unwrap_or(0)
    }
    /// Run `f` and send the table to the backend once at the end (fast bulk updates).
    pub fn batch(&self, f: impl FnOnce(&Table)) {
        core::freeze(self.id(), true);
        let _g = Thaw(self.id());
        f(self)
    }
    /// Select a row (out of range = clear). No callback fires.
    pub fn set_selected(&self, row: Option<usize>) {
        core::data_update(self.id(), core::Data::TableSelected, |n| {
            if let Some(t) = n.table_mut() {
                t.selected = row.filter(|i| *i < t.rows.len());
            }
        });
    }
    /// The selected row.
    pub fn selected(&self) -> Option<usize> {
        selected(self.id())
    }
    /// The selected row's cells.
    pub fn selected_row(&self) -> Option<Vec<String>> {
        core::read(self.id(), |n| {
            let t = n.table()?;
            t.rows.get(t.selected?).cloned()
        })
        .flatten()
    }
    /// Show the sort arrow on `(column, ascending)`. Display only: the app reorders the rows itself.
    pub fn set_sort_indicator(&self, s: Option<(usize, bool)>) {
        core::data_update(self.id(), core::Data::TableSort, |n| {
            if let Some(t) = n.table_mut() {
                t.sort = s.filter(|(c, _)| *c < t.columns.len());
            }
        });
    }
    /// The sort arrow set with [`Table::set_sort_indicator`].
    pub fn sort_indicator(&self) -> Option<(usize, bool)> {
        core::read(self.id(), |n| n.table().and_then(|t| t.sort)).flatten()
    }
    /// Run `f` with the new row when the user changes the selection (`None` when cleared).
    pub fn on_select(&self, f: impl FnMut(Option<usize>) + 'static) {
        on_select(self.id(), f)
    }
    /// Double-click / Enter on a row.
    pub fn on_activate(&self, mut f: impl FnMut(usize) + 'static) {
        on(self.id(), Ev::Activated, move |e| {
            if let Event::Activated(i) = e {
                f(*i)
            }
        })
    }
    /// A column header was clicked (typically: sort the rows and call `set_sort_indicator`).
    pub fn on_column_click(&self, mut f: impl FnMut(usize) + 'static) {
        on(self.id(), Ev::ColumnClicked, move |e| {
            if let Event::ColumnClicked(c) = e {
                f(*c)
            }
        })
    }
}
