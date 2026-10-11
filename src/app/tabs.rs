//! Tabs: each is a place (view, site, board, catalog, thread, ...) of its own. The active
//! tab is `App::tab`; the others wait in `App::tabs`, and switching swaps them in and out.

use super::{App, LinksPanel, Opening, Preview, Tab, View, Viewer, thread_subject};
use crate::model::Board;
use crate::store::ThreadKey;

/// A thread left for another, to come back to: its selected post, and where back went from
/// it (`View::Thread`: the thread it was opened from).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trail {
    pub key: ThreadKey,
    pub post: u64,
    pub back: Option<View>,
}

/// Tabs open at once, at most.
pub const MAX_TABS: usize = 9;

/// A saved copy being read: when it was saved, and whether the thread is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offline {
    pub saved: i64,
    pub dead: bool,
}

/// A copy of the thread shown instead of the live one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadCopy {
    /// The last copy kept, shown while the thread is fetched: when it was fetched.
    Cached(Offline),
    /// A saved copy, read offline.
    Saved(Offline),
}

/// A popup of the tab's own (it stays with the tab when another is shown).
pub enum TabPopup {
    /// The full-screen image viewer.
    Viewer(Viewer),
    /// The posts the selected post quotes (`p`).
    Preview(Preview),
    /// The selected post's links and files (`O`).
    Links(LinksPanel),
}

impl App {
    /// Make tab `i` the active one.
    pub fn switch_tab(&mut self, i: usize) {
        // Not the active tab itself, nor one that isn't there.
        let Ok([here, there]) = self.tabs.get_disjoint_mut([self.active, i]) else { return };
        std::mem::swap(&mut self.tab, here);
        std::mem::swap(&mut self.tab, there);
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
    pub(super) fn handle_in_tab(&mut self, i: usize, apply: Box<dyn FnOnce(&mut App) + Send>) {
        let (active, before) = (self.active, self.footer.clone());
        self.switch_tab(i);
        apply(self);
        self.switch_tab(active);
        // What happens in other tabs isn't news here.
        self.footer = before;
    }

    /// The inactive tab whose request this is.
    pub(super) fn tab_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().enumerate().position(|(i, t)| i != self.active && t.req() == Some(id))
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
        let open = match self.tab.view() {
            View::Catalog => self.selected_index().and_then(|i| self.tab.catalog.posts().get(i)).map(|p| Open::Thread(self.find_board(&self.tab.catalog.board_of(p)), p.no)),
            View::Watched | View::History => self.selected_listed().map(|(key, _)| Open::Key(key.clone())),
            View::Saved => self.selected_listed().map(|(key, _)| Open::Saved(key.clone())),
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
            Open::Thread(board, no) => self.open_thread_at(board, no, Opening::default()),
            Open::Key(key) => self.open_key(&key),
            Open::Saved(key) => self.open_saved(&key, Opening::default()),
            Open::Link(link) => self.follow(&link),
        }
    }

    pub fn cycle_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        if n < 2 {
            self.info(format!("One tab: {} opens a thread in a new one", self.keys.how(crate::keys::Action::NewTab)));
            return;
        }
        self.show_tab(if forward { (self.active + 1) % n } else { (self.active + n - 1) % n });
    }

    /// Go to tab `i`, leaving what was said in the last one behind.
    pub(super) fn show_tab(&mut self, i: usize) {
        self.switch_tab(i);
        self.footer.clear_seen();
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

    /// Tab `i`, the active one or another.
    fn tab_at(&self, i: usize) -> &Tab {
        if i == self.active { &self.tab } else { self.tabs.get(i).unwrap_or(&self.tab) }
    }

    /// Posts a watched thread shown in tab `i` has gained since it was read (its count in
    /// Watched), when there are any.
    pub fn tab_unread(&self, i: usize) -> Option<usize> {
        let t = self.tab_at(i);
        let th = t.thread.as_ref().filter(|_| t.view() == View::Thread)?;
        let w = self.store.watched(th.key())?;
        Some(w.status.counts().0).filter(|&n| n > 0)
    }

    /// The terminal's title: "ck: (3) /g/ Subject" in a thread, with the new posts below the
    /// screen; elsewhere where ck is, with the watched threads' unread posts. "(You)" when
    /// some of those reply to yours. `None` with `set_title` off.
    pub fn terminal_title(&self) -> Option<String> {
        if !self.set_title {
            return None;
        }
        let (new, yours) = match self.tab.thread.as_ref().filter(|_| self.tab.view() == View::Thread) {
            Some(th) => th.new_below(),
            None => self.store.watched_new(),
        };
        let new = if new > 0 { format!("({new}) ") } else { String::new() };
        let yours = if yours > 0 { "(You) " } else { "" };
        let place = match self.tab.thread.as_ref().filter(|_| self.tab.view() == View::Thread) {
            Some(th) => format!("/{}/ {}", th.key().board, thread_subject(&th.posts)),
            None => self.tab_label(self.active),
        };
        Some(crate::title::clean(&format!("ck: {new}{yours}{place}")))
    }

    /// Write the terminal's title if it changed (or put the old one back once it's off).
    pub fn show_title(&mut self) {
        let title = self.terminal_title();
        if title == self.title_shown {
            return;
        }
        match &title {
            Some(t) => crate::title::set(t),
            None => crate::title::restore(),
        }
        self.title_shown = title;
    }

    /// On exit: the terminal's title as it was before ck.
    pub fn restore_title(&mut self) {
        crate::title::restore();
        self.title_shown = None;
    }

    /// What a tab shows, in a few words.
    pub fn tab_label(&self, i: usize) -> String {
        let t = self.tab_at(i);
        let board = t.board.as_ref().map_or("", |b| b.uri.as_str());
        match t.view() {
            View::Thread => match &t.thread {
                Some(th) => thread_subject(&th.posts),
                None => format!("/{board}/{}", t.pending_thread.as_ref().map_or(0, |k| k.no)),
            },
            View::Catalog => format!("/{board}/"),
            View::Boards => self.sites.get(t.site).map_or(String::new(), |s| s.cfg.name.clone()),
            View::Search => t.search.as_ref().map_or_else(|| "Search".into(), |s| format!("Search: {}", s.query)),
            View::Sites => "Sites".into(),
            View::Watched => "Watched".into(),
            View::History => "History".into(),
            View::Saved => "Saved".into(),
            View::Settings => "Settings".into(),
        }
    }
}
