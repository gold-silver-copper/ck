//! Tabs: each is a place (view, site, board, catalog, thread, ...) of its own. The active
//! tab is `App::tab`; the others wait in `App::tabs`, and switching swaps them in and out.

use std::collections::HashSet;
use std::time::Instant;

use super::{App, Gallery, LinksPanel, Msg, Picker, Preview, Search, Sort, ThreadView, View, Viewer, thread_subject};
use crate::filter::Mark;
use crate::model::{Board, Post};
use crate::store::ThreadKey;

/// Tabs open at once, at most.
pub const MAX_TABS: usize = 9;

/// One tab's place: the active one is `App::tab`.
/// A saved copy being read: when it was saved, and whether the thread is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offline {
    pub saved: i64,
    pub dead: bool,
}

pub struct Tab {
    pub view: View,
    /// Where esc goes back to from Settings.
    pub settings_back: Option<View>,
    pub board_list: Picker,
    pub catalog_list: Picker,
    pub catalog_sort: Sort,
    /// Where `back` goes from a thread opened from Watched or History.
    pub return_to: Option<View>,
    pub thread_checked: Instant,
    pub site: usize,
    pub board: Option<Board>,
    pub catalog: Vec<Post>,
    /// What filters and hiding say about each catalog thread.
    pub catalog_marks: Vec<Mark>,
    /// Catalog threads that weren't there on the previous visit.
    pub catalog_new: HashSet<u64>,
    pub thread: Option<ThreadView>,
    /// Label of the in-flight request, if any.
    pub loading: Option<String>,
    pub viewer: Option<Viewer>,
    pub preview: Option<Preview>,
    pub links: Option<LinksPanel>,
    /// The thread's files as a grid (`V`), over the thread.
    pub gallery: Option<Gallery>,
    /// Threads left by following cross-thread links: (site, board, thread, selected post), for `u`.
    pub trail: Vec<(usize, Board, u64, u64)>,
    /// Post to select once the loading thread arrives.
    pub pending_post: Option<u64>,
    /// Post whose conversation to show once the loading thread arrives (a session's).
    pub pending_conversation: Option<u64>,
    /// Board the loaded catalog belongs to.
    pub catalog_board: String,
    /// The site the loaded catalog is from.
    pub catalog_site: usize,
    /// The board whose catalog is loaded, to return to from a thread opened on another board
    /// (an overboard's threads live on their own boards).
    pub catalog_of: Option<Board>,
    /// The open thread was opened from the catalog (not by following a link).
    pub from_catalog: bool,
    /// After a thread 404'd: the same thread on the site's configured archive.
    pub archive_offer: Option<ThreadKey>,
    /// After a thread 404'd: its saved copy (`enter` opens it).
    pub saved_offer: Option<ThreadKey>,
    /// The open thread is a saved copy, read offline.
    pub offline: Option<Offline>,
    /// The tab's request in flight (0: none).
    pub req: u64,
    /// The thread number of the last thread load, for 404 handling.
    pub pending_thread: u64,
    /// Thread to select in the catalog once it loads (restoring a session).
    pub pending_catalog: Option<u64>,
    /// The open thread is being restored from the last session.
    pub restoring: bool,
    /// Archive search: its results and the list over them.
    pub search: Option<Search>,
    pub search_list: Picker,
}

impl Tab {
    pub fn new(site: usize, now: Instant) -> Self {
        Self {
            view: View::Sites,
            settings_back: None,
            board_list: Picker::top(),
            catalog_list: Picker::default(),
            catalog_sort: Sort::default(),
            return_to: None,
            thread_checked: now,
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
            catalog_site: site,
            catalog_of: None,
            from_catalog: false,
            archive_offer: None,
            saved_offer: None,
            pending_conversation: None,
            offline: None,
            req: 0,
            pending_thread: 0,
            pending_catalog: None,
            restoring: false,
            search: None,
            search_list: Picker::default(),
        }
    }
}

impl App {
    /// Make tab `i` the active one.
    pub fn switch_tab(&mut self, i: usize) {
        if i == self.active || i >= self.tabs.len() {
            return;
        }
        std::mem::swap(&mut self.tab, &mut self.tabs[self.active]);
        std::mem::swap(&mut self.tab, &mut self.tabs[i]);
        self.active = i;
    }

    /// Lay every tab's thread out again (their colors changed).
    pub fn invalidate_layouts(&mut self) {
        for t in self.tab.thread.iter_mut().chain(self.tabs.iter_mut().filter_map(|tab| tab.thread.as_mut())) {
            t.layout = None;
            t.cache.clear();
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
    Saved(ThreadKey),
    Link(crate::model::Link),
}

impl App {
    /// `T`: open the selected thread (or the link enter would follow) in a new tab, after
    /// this one.
    pub fn new_tab(&mut self) {
        if self.tabs.len() >= MAX_TABS {
            self.info(format!("{MAX_TABS} tabs is the most; close one with {}", self.keys.key(crate::keys::Action::CloseTab)));
            return;
        }
        let open = match self.tab.view {
            View::Catalog => self.selected_index().map(|i| {
                let p = &self.tab.catalog[i];
                Open::Thread(self.find_board(&self.board_of(p)), p.no)
            }),
            View::Watched => self.selected_index().map(|i| Open::Key(self.store.watched[i].key.clone())),
            View::History => self.selected_index().map(|i| Open::Key(self.store.history[i].key.clone())),
            View::Saved => self.selected_index().map(|i| Open::Saved(self.store.saved[i].key.clone())),
            View::Thread => match self.outgoing_link() {
                Some(link) => Some(Open::Link(link)),
                None => {
                    self.info("The post quotes nothing in another thread to open in a tab");
                    return;
                }
            },
            _ => None,
        };
        let Some(open) = open else { return };
        let (site, board) = (self.tab.site, self.tab.board.clone());
        let at = self.active + 1;
        self.tabs.insert(at, Tab::new(site, self.clock.instant()));
        self.switch_tab(at);
        self.tab.board = board;
        match open {
            Open::Thread(board, no) => self.open_thread_at(board, no, None),
            Open::Key(key) => self.open_key(key),
            Open::Saved(key) => self.open_saved(key),
            Open::Link(link) => self.follow(link),
        }
    }

    pub fn cycle_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        if n < 2 {
            self.info(format!("One tab: {} opens a thread in a new one", self.keys.key(crate::keys::Action::NewTab)));
            return;
        }
        self.switch_tab(if forward { (self.active + 1) % n } else { (self.active + n - 1) % n });
    }

    /// Close the active tab (not the last one).
    pub fn close_tab(&mut self) {
        let n = self.tabs.len();
        if n < 2 {
            self.info("This is the only tab (q quits)");
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

    /// Every tab's place, for the session.
    pub fn places(&mut self) -> Vec<crate::store::Place> {
        let active = self.active;
        let places = (0..self.tabs.len())
            .map(|i| {
                self.switch_tab(i);
                self.place()
            })
            .collect();
        self.switch_tab(active);
        places
    }

    /// What a tab shows, in a few words.
    pub fn tab_label(&self, i: usize) -> String {
        let t = if i == self.active { &self.tab } else { self.tabs.get(i).unwrap_or(&self.tab) };
        let board = t.board.as_ref().map_or("", |b| b.uri.as_str());
        match t.view {
            View::Thread => match &t.thread {
                Some(th) => thread_subject(&th.posts),
                None => format!("/{board}/{}", t.pending_thread),
            },
            View::Catalog => format!("/{board}/"),
            View::Boards => self.sites.get(t.site).map_or(String::new(), |s| s.cfg.name.clone()),
            View::Search => t.search.as_ref().map_or("Search".into(), |s| format!("Search: {}", s.query)),
            View::Sites => "Sites".into(),
            View::Watched => "Watched".into(),
            View::History => "History".into(),
            View::Saved => "Saved".into(),
            View::Settings => "Settings".into(),
        }
    }
}
