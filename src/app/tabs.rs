//! Tabs: each is a place (view, site, board, catalog, thread, ...) of its own. The active
//! tab's state lives in `App`'s fields; the others wait in `App::tabs`, and switching
//! swaps them in and out.

use std::collections::HashSet;
use std::time::Instant;

use super::{App, Gallery, LinksPanel, Msg, Picker, Preview, Search, Sort, ThreadView, View, Viewer, thread_subject};
use crate::filter::Mark;
use crate::model::{Board, Post};
use crate::store::ThreadKey;

/// Tabs open at once, at most.
pub const MAX_TABS: usize = 9;

/// One tab's place. Mirrors the per-place fields of `App`.
pub struct Tab {
    view: View,
    settings_back: Option<View>,
    board_list: Picker,
    catalog_list: Picker,
    catalog_sort: Sort,
    return_to: Option<View>,
    thread_checked: Instant,
    site: usize,
    board: Option<Board>,
    catalog: Vec<Post>,
    catalog_marks: Vec<Mark>,
    catalog_new: HashSet<u64>,
    thread: Option<ThreadView>,
    loading: Option<String>,
    viewer: Option<Viewer>,
    preview: Option<Preview>,
    links: Option<LinksPanel>,
    gallery: Option<Gallery>,
    trail: Vec<(usize, Board, u64, u64)>,
    pending_post: Option<u64>,
    catalog_board: String,
    catalog_of: Option<Board>,
    from_catalog: bool,
    archive_offer: Option<ThreadKey>,
    req: u64,
    pending_thread: u64,
    pending_catalog: Option<u64>,
    restoring: bool,
    search: Option<Search>,
    search_list: Picker,
}

impl Tab {
    pub fn new(site: usize) -> Self {
        let mut board_list = Picker::default();
        board_list.state.select(Some(0));
        Self {
            view: View::Sites,
            settings_back: None,
            board_list,
            catalog_list: Picker::default(),
            catalog_sort: Sort::default(),
            return_to: None,
            thread_checked: Instant::now(),
            site,
            board: None,
            catalog: Vec::new(),
            catalog_marks: Vec::new(),
            catalog_new: HashSet::new(),
            thread: None,
            loading: None,
            viewer: None,
            preview: None,
            links: None,
            gallery: None,
            trail: Vec::new(),
            pending_post: None,
            catalog_board: String::new(),
            catalog_of: None,
            from_catalog: false,
            archive_offer: None,
            req: 0,
            pending_thread: 0,
            pending_catalog: None,
            restoring: false,
            search: None,
            search_list: Picker::default(),
        }
    }
}

impl Tab {
    #[cfg(test)]
    pub fn req_for_tests(&self) -> u64 {
        self.req
    }
}

impl App {
    /// Exchange the active tab's state (in `App`'s fields) with `tab`.
    fn swap_tab(&mut self, tab: &mut Tab) {
        use std::mem::swap;
        swap(&mut self.view, &mut tab.view);
        swap(&mut self.settings.back, &mut tab.settings_back);
        swap(&mut self.board_list, &mut tab.board_list);
        swap(&mut self.catalog_list, &mut tab.catalog_list);
        swap(&mut self.catalog_sort, &mut tab.catalog_sort);
        swap(&mut self.return_to, &mut tab.return_to);
        swap(&mut self.thread_checked, &mut tab.thread_checked);
        swap(&mut self.site, &mut tab.site);
        swap(&mut self.board, &mut tab.board);
        swap(&mut self.catalog, &mut tab.catalog);
        swap(&mut self.catalog_marks, &mut tab.catalog_marks);
        swap(&mut self.catalog_new, &mut tab.catalog_new);
        swap(&mut self.thread, &mut tab.thread);
        swap(&mut self.loading, &mut tab.loading);
        swap(&mut self.viewer, &mut tab.viewer);
        swap(&mut self.preview, &mut tab.preview);
        swap(&mut self.links, &mut tab.links);
        swap(&mut self.gallery, &mut tab.gallery);
        swap(&mut self.trail, &mut tab.trail);
        swap(&mut self.pending_post, &mut tab.pending_post);
        swap(&mut self.catalog_board, &mut tab.catalog_board);
        swap(&mut self.catalog_of, &mut tab.catalog_of);
        swap(&mut self.from_catalog, &mut tab.from_catalog);
        swap(&mut self.archive_offer, &mut tab.archive_offer);
        swap(&mut self.req, &mut tab.req);
        swap(&mut self.pending_thread, &mut tab.pending_thread);
        swap(&mut self.pending_catalog, &mut tab.pending_catalog);
        swap(&mut self.restoring, &mut tab.restoring);
        swap(&mut self.search, &mut tab.search);
        swap(&mut self.search_list, &mut tab.search_list);
    }

    /// Make tab `i` the active one.
    pub fn switch_tab(&mut self, i: usize) {
        if i == self.active || i >= self.tabs.len() {
            return;
        }
        let mut tabs = std::mem::take(&mut self.tabs);
        self.swap_tab(&mut tabs[self.active]);
        self.swap_tab(&mut tabs[i]);
        self.tabs = tabs;
        self.active = i;
    }

    /// Lay every tab's thread out again (their colors changed).
    pub fn invalidate_layouts(&mut self) {
        for t in self.thread.iter_mut().chain(self.tabs.iter_mut().filter_map(|tab| tab.thread.as_mut())) {
            t.layout = None;
        }
    }

    /// Handle a response for an inactive tab, as that tab, then switch back.
    pub(super) fn handle_in_tab(&mut self, i: usize, msg: Msg) {
        let (active, status) = (self.active, self.status.clone());
        self.switch_tab(i);
        self.handle(msg);
        self.switch_tab(active);
        // What happens in other tabs isn't news here.
        self.status = status;
    }

    /// The inactive tab whose request this is.
    pub(super) fn tab_of(&self, id: u64) -> Option<usize> {
        (id != 0).then(|| self.tabs.iter().enumerate().position(|(i, t)| i != self.active && t.req == id)).flatten()
    }
}

/// What `T` opens in a new tab.
enum Open {
    Thread(Board, u64),
    Key(ThreadKey),
    Link(crate::model::Link),
}

impl App {
    /// `T`: open the selected thread (or the link enter would follow) in a new tab, after
    /// this one.
    pub fn new_tab(&mut self) {
        if self.tabs.len() >= MAX_TABS {
            self.status = Some((format!("{MAX_TABS} tabs is the most; close one with {}", self.keys.key(crate::keys::Action::CloseTab)), false));
            return;
        }
        let open = match self.view {
            View::Catalog => self.selected_index().map(|i| {
                let p = &self.catalog[i];
                Open::Thread(self.find_board(&self.board_of(p)), p.no)
            }),
            View::Watched => self.selected_index().map(|i| Open::Key(self.store.watched[i].key.clone())),
            View::History => self.selected_index().map(|i| Open::Key(self.store.history[i].key.clone())),
            View::Thread => match self.outgoing_link() {
                Some(link) => Some(Open::Link(link)),
                None => {
                    self.status = Some(("The post quotes nothing in another thread to open in a tab".into(), false));
                    return;
                }
            },
            _ => None,
        };
        let Some(open) = open else { return };
        let (site, board) = (self.site, self.board.clone());
        let at = self.active + 1;
        self.tabs.insert(at, Tab::new(site));
        self.switch_tab(at);
        self.board = board;
        match open {
            Open::Thread(board, no) => self.open_thread_at(board, no, None, false),
            Open::Key(key) => self.open_key(key),
            Open::Link(link) => self.follow(link),
        }
    }

    pub fn cycle_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        if n < 2 {
            self.status = Some((format!("One tab: {} opens a thread in a new one", self.keys.key(crate::keys::Action::NewTab)), false));
            return;
        }
        self.switch_tab(if forward { (self.active + 1) % n } else { (self.active + n - 1) % n });
    }

    /// Close the active tab (not the last one).
    pub fn close_tab(&mut self) {
        let n = self.tabs.len();
        if n < 2 {
            self.status = Some(("This is the only tab (q quits)".into(), false));
            return;
        }
        let old = self.active;
        let to = if old + 1 < n { old + 1 } else { old - 1 };
        self.switch_tab(to);
        self.tabs.remove(old);
        if to > old {
            self.active -= 1;
        }
    }

    /// Run `f` with tab `i` as the active one (without the redraw a real switch does).
    fn in_tab<T>(&mut self, i: usize, f: impl FnOnce(&Self) -> T) -> T {
        if i == self.active {
            return f(self);
        }
        let mut tabs = std::mem::take(&mut self.tabs);
        self.swap_tab(&mut tabs[self.active]);
        self.swap_tab(&mut tabs[i]);
        let out = f(self);
        self.swap_tab(&mut tabs[i]);
        self.swap_tab(&mut tabs[self.active]);
        self.tabs = tabs;
        out
    }

    /// Every tab's place, for the session.
    pub fn places(&mut self) -> Vec<crate::store::Place> {
        (0..self.tabs.len()).map(|i| self.in_tab(i, |app| app.place())).collect()
    }

    /// What a tab shows, in a few words.
    pub fn tab_label(&mut self, i: usize) -> String {
        self.in_tab(i, |app| match app.view {
            View::Thread => match &app.thread {
                Some(t) => thread_subject(&t.posts),
                None => format!("/{}/{}", app.board.as_ref().map_or("", |b| b.uri.as_str()), app.pending_thread),
            },
            View::Catalog => format!("/{}/", app.board.as_ref().map_or("", |b| b.uri.as_str())),
            View::Boards => app.current_site().cfg.name.clone(),
            View::Search => app.search.as_ref().map_or("Search".into(), |s| format!("Search: {}", s.query)),
            View::Sites => "Sites".into(),
            View::Watched => "Watched".into(),
            View::History => "History".into(),
            View::Settings => "Settings".into(),
        })
    }
}
