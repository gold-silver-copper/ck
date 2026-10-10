use ratatui::widgets::ListState;

use super::{App, RowKey, View};

/// A filterable list. Its selection is the item it's on, so it stays on it as the list
/// changes under it: nothing outside this file can hold, set or clamp a selected row.
pub struct FilteredList {
    pub filter: String,
    /// What's selected; `None` until something is (then the top row).
    key: Option<RowKey>,
    /// While the list is coming and `key` isn't in it: the item drawn in its place, which
    /// is what's selected until `key` comes (or the list stops coming).
    stand_in: Option<RowKey>,
    /// `selected` is the row the item was last on, used only while it isn't listed (it
    /// went, or hasn't come yet): the row now there is selected. `offset` is the scroll.
    state: ListState,
}

impl FilteredList {
    /// A list opened anew, at its top: what a background change must never make.
    pub(super) fn fresh() -> Self {
        Self::on(None, String::new())
    }

    /// On `key` with `filter`, as a session left it: waited for while its list loads.
    pub(super) fn on(key: Option<RowKey>, filter: String) -> Self {
        Self { filter, key, stand_in: None, state: ListState::default() }
    }

    pub(super) fn key(&self) -> Option<&RowKey> {
        self.key.as_ref()
    }
}

/// Which list `view` shows (a thread has none): `&self` or `&mut self`.
macro_rules! list_of {
    ($view:expr, $($app:tt)+) => {
        match $view {
            View::Sites => Some($($app)+.site_list),
            View::Boards => Some($($app)+.tab.board_list),
            View::Catalog => Some($($app)+.tab.catalog_list),
            View::Watched => Some($($app)+.watched_list),
            View::History => Some($($app)+.history_list),
            View::Saved => Some($($app)+.saved_list),
            View::Settings => Some($($app)+.settings_list),
            View::Search => Some($($app)+.tab.search_list),
            View::Thread => None,
        }
    };
}

/// The selected row of `rows`: its item's (or stand-in's), else the one it was last on.
fn row_in(l: &FilteredList, rows: &[RowKey]) -> Option<usize> {
    let last = l.state.selected().unwrap_or(0);
    let find = |k: &RowKey| if rows.get(last) == Some(k) { Some(last) } else { rows.iter().position(|r| r == k) };
    let listed = l.key.as_ref().and_then(find).or_else(|| l.stand_in.as_ref().and_then(find));
    listed.or_else(|| Some(last.min(rows.len().checked_sub(1)?)))
}

/// Select row `row` of `rows`: the item on it, from now on.
fn put(l: &mut FilteredList, rows: &[RowKey], row: usize) -> bool {
    let Some(k) = rows.get(row) else { return false };
    l.key = Some(k.clone());
    l.stand_in = None;
    l.state.select(Some(row));
    true
}

impl App {
    fn list(&self, view: View) -> Option<&FilteredList> {
        list_of!(view, &self)
    }

    fn list_mut(&mut self, view: View) -> Option<&mut FilteredList> {
        list_of!(view, &mut self)
    }

    pub(crate) fn filter(&self, view: View) -> &str {
        self.list(view).map_or("", |l| &l.filter)
    }

    /// Change `view`'s filter, which goes back to the top of what it lists.
    pub(super) fn edit_filter(&mut self, view: View, f: impl FnOnce(&mut String)) {
        if let Some(l) = self.list_mut(view) {
            f(&mut l.filter);
            *l = FilteredList::on(None, std::mem::take(&mut l.filter));
        }
    }

    /// The selected row of `view`'s list, as it is now. None when it's empty.
    pub(crate) fn selected_row(&self, view: View) -> Option<usize> {
        row_in(self.list(view)?, &self.row_keys(view))
    }

    /// Where the selection is in what's shown: its place (from 1) and how many there are; in
    /// a thread, the selected post's.
    pub(crate) fn position(&self) -> Option<(usize, usize)> {
        let view = self.tab.view();
        if view == View::Thread {
            return self.tab.thread.as_ref().filter(|_| self.tab.gallery.is_none())?.ruler();
        }
        let rows = self.row_keys(view);
        Some((row_in(self.list(view)?, &rows)?.saturating_add(1), rows.len())).filter(|&(_, n)| n > 0)
    }

    pub(super) fn selected_key(&self, view: View) -> Option<RowKey> {
        let rows = self.row_keys(view);
        rows.get(row_in(self.list(view)?, &rows)?).cloned()
    }

    /// Select what's on row `row` of `view`'s list now (a click, or a test). Whether
    /// there's such a row.
    pub(crate) fn pick_row(&mut self, view: View, row: usize) -> bool {
        let rows = self.row_keys(view);
        self.list_mut(view).is_some_and(|l| put(l, &rows, row))
    }

    /// Select `key` if it's listed (a hint label). Whether it is.
    pub(super) fn select_key(&mut self, view: View, key: &RowKey) -> bool {
        self.row_keys(view).iter().position(|r| r == key).is_some_and(|row| self.pick_row(view, row))
    }

    /// Move `delta` rows, stopping at either end (g and G go past them).
    pub(super) fn move_selection(&mut self, view: View, delta: isize) {
        let rows = self.row_keys(view);
        if let Some(l) = self.list_mut(view)
            && let Some(row) = row_in(l, &rows)
        {
            put(l, &rows, row.saturating_add_signed(delta).min(rows.len().saturating_sub(1)));
        }
    }

    /// The state to draw `view`'s list with. What's drawn selected is the selection from
    /// then on: when its item went, the item now in its place. While the list is still
    /// coming (or failed to: `r` may bring it) the gone item is still waited for.
    pub(crate) fn list_state(&mut self, view: View) -> ListState {
        let rows = self.row_keys(view);
        let coming = matches!(view, View::Boards | View::Catalog | View::Search) && (self.tab.loading().is_some() || self.tab.failed.is_some());
        let Some(l) = self.list_mut(view) else { return ListState::default() };
        let row = row_in(l, &rows);
        if let Some(row) = row {
            let waited = l.key.clone().filter(|k| coming && rows.get(row) != Some(k));
            put(l, &rows, row);
            l.stand_in = waited.and_then(|k| l.key.replace(k));
        }
        l.state.with_selected(row)
    }

    /// After drawing `view`'s list: where it's scrolled to, as `ui::window` worked it out.
    #[allow(clippy::disallowed_methods)]
    pub(crate) fn list_scrolled(&mut self, view: View, offset: usize) {
        if let Some(l) = self.list_mut(view) {
            *l.state.offset_mut() = offset;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{App, MenuItem, RowKey, Then, View};
    use crate::keys::Action;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    use crate::test_fixtures::{nos, test_app as app};

    fn catalog(nos_: &[u64]) -> App {
        let mut app = app();
        app.tab.navigate(View::Catalog);
        app.show_catalog(nos(nos_));
        app
    }

    #[test]
    fn a_reorder_follows_the_selected_item() {
        let mut app = catalog(&[1, 2, 3]);
        assert!(app.pick_row(View::Catalog, 1));
        app.show_catalog(nos(&[3, 1, 2]));
        assert_eq!(app.selected_row(View::Catalog), Some(2));
        assert!(!app.pick_row(View::Catalog, 3), "no such row");
        assert_eq!(app.selected_key(View::Catalog), Some(RowKey::Thread(2)));
    }

    #[test]
    fn a_gone_item_leaves_its_row_selected_kept_in_the_list() {
        let mut app = catalog(&[1, 2, 3, 4]);
        app.pick_row(View::Catalog, 2);
        app.show_catalog(nos(&[1, 2, 4]));
        assert_eq!(app.selected_row(View::Catalog), Some(2));
        app.show_catalog(nos(&[1]));
        assert_eq!(app.selected_row(View::Catalog), Some(0));
        app.show_catalog(Vec::new());
        assert_eq!(app.selected_row(View::Catalog), None);
        app.show_catalog(nos(&[9, 3]));
        assert_eq!(app.selected_row(View::Catalog), Some(1), "back on it");
    }

    #[test]
    fn drawing_without_the_item_selects_the_row_in_its_place() {
        let mut app = catalog(&[1, 2, 3, 4]);
        app.pick_row(View::Catalog, 2);
        app.show_catalog(nos(&[1, 2, 4]));
        assert_eq!(app.list_state(View::Catalog).selected(), Some(2));
        app.show_catalog(nos(&[0, 1, 2, 3, 4]));
        assert_eq!(app.selected_key(View::Catalog), Some(RowKey::Thread(4)));
        // An untouched list is on its top row, and drawing pins that.
        let mut app = catalog(&[1, 2]);
        app.list_state(View::Catalog);
        app.show_catalog(nos(&[0, 1, 2]));
        assert_eq!(app.selected_row(View::Catalog), Some(1));
    }

    #[test]
    fn a_loading_catalog_waits_for_a_restored_thread() {
        let mut app = catalog(&[]);
        app.tab.catalog_list = super::FilteredList::on(Some(RowKey::Thread(5)), String::new());
        app.tab.fake_load(8, "Loading /a/", Then::Show);
        app.show_catalog(nos(&[1, 2]));
        app.list_state(View::Catalog);
        app.show_catalog(nos(&[1, 2, 5]));
        assert_eq!(app.selected_row(View::Catalog), Some(2));
        // Once loaded, a thread that didn't come is gone: the row drawn in its place.
        app.tab.answered(8);
        app.show_catalog(nos(&[1, 2]));
        app.list_state(View::Catalog);
        app.show_catalog(nos(&[1, 2, 5]));
        assert_eq!(app.selected_row(View::Catalog), Some(0));
    }

    #[test]
    fn a_failed_load_keeps_a_restored_thread_for_a_retry() {
        let mut app = catalog(&[]);
        app.tab.catalog_list = super::FilteredList::on(Some(RowKey::Thread(5)), String::new());
        app.tab.fake_load(8, "Loading /a/", Then::Show);
        app.show_catalog(nos(&[1, 2]));
        app.tab.answered(8);
        app.catalog_arrived(Err(anyhow::anyhow!("offline")));
        app.list_state(View::Catalog);
        assert_eq!(app.tab.catalog_list.key(), Some(&RowKey::Thread(5)));
    }

    /// A menu opened on the row drawn in a coming thread's place runs on what it opened on,
    /// though another row is drawn there by the time it runs.
    #[test]
    fn a_menu_runs_on_what_it_opened_on() {
        let mut app = catalog(&[]);
        app.tab.catalog_list = super::FilteredList::on(Some(RowKey::Thread(5)), String::new());
        app.tab.fake_load(8, "Loading /a/", Then::Show);
        app.show_catalog(nos(&[1, 2]));
        app.on_key(KeyEvent::from(KeyCode::Char('.')));
        app.show_catalog(nos(&[0, 1, 2]));
        let m = app.menu_mut().unwrap();
        let hide = m.items.iter().position(|it| matches!(it, MenuItem::Act(Action::Hide, _))).unwrap();
        m.list.select(Some(hide));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.row_keys(View::Catalog), [RowKey::Thread(0), RowKey::Thread(2)]);
    }

    /// Only what acts on the row it opened on is refused once that's gone.
    #[test]
    fn a_menu_whose_row_went_still_sorts() {
        let mut app = catalog(&[1, 2, 3]);
        app.on_key(KeyEvent::from(KeyCode::Char('.')));
        app.show_catalog(nos(&[2, 3]));
        let m = app.menu_mut().unwrap();
        let sort = m.items.iter().position(|it| matches!(it, MenuItem::Act(Action::Sort, _))).unwrap();
        m.list.select(Some(sort));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_ne!(app.tab.catalog_sort, crate::app::Sort::Bump);
    }

    #[test]
    fn sorting_an_empty_list_goes_back_to_the_top() {
        let mut app = catalog(&[1, 2, 3, 4, 5]);
        app.pick_row(View::Catalog, 3);
        app.show_catalog(Vec::new());
        app.act(Action::Sort);
        app.show_catalog(nos(&[1, 2, 3, 4, 5]));
        assert_eq!(app.selected_row(View::Catalog), Some(0));
    }
}
