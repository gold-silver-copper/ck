//! One tab's place and what it shows.

use std::collections::HashSet;
use std::time::Instant;

use super::{FilteredList, Gallery, Marks, Search, Sort, ThreadView, View, Viewer};
use super::tabs::{Offline, TabPopup, ThreadCopy, Trail};
use crate::model::{Board, Post};
use crate::store::ThreadKey;

/// What a load is to do once it's answered, beyond showing what came. Its answer's handler
/// is handed what the load was asked for (a thread's key) by the closure that started it.
pub enum Then {
    /// Just show it (a board list, a catalog, search results, the thread a post is in).
    Show,
    /// Open the thread as `open` says.
    Thread { open: Opening },
}

/// The most threads the jump list keeps.
const JUMPS: usize = 100;

/// How a thread being loaded is opened once it arrives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Opening {
    /// The post to select.
    pub select: Option<u64>,
    /// The post whose conversation to show (a session's).
    pub conversation: Option<u64>,
    /// It's the thread last session had open: if it's gone, its catalog instead.
    pub restoring: bool,
}

impl Opening {
    /// Opened on `select`.
    pub fn at(select: Option<u64>) -> Self {
        Opening { select, ..Opening::default() }
    }
}

/// The tab's load in flight (or answered): its request, its label while it's in flight,
/// and what to do with its answer.
struct Load {
    id: u64,
    label: Option<String>,
    then: Then,
}

/// A board's catalog as shown: the site and board it was loaded from, which only a new
/// `Catalog` sets, with its threads and what's known about them. `Default`: none loaded.
#[derive(Default)]
pub struct Catalog {
    site: String,
    board: Option<Board>,
    posts: Vec<Post>,
    /// What filters and hiding say about each thread.
    pub marks: Marks,
    /// Threads that weren't there on the previous visit.
    pub new: HashSet<u64>,
    /// It's the last copy kept, shown while the board is fetched.
    pub cached: Option<Offline>,
}

impl Catalog {
    /// `board` on `site`, with nothing loaded yet.
    pub fn new(site: String, board: Board) -> Self {
        Catalog { site, board: Some(board), ..Catalog::default() }
    }

    pub fn site(&self) -> &str {
        &self.site
    }

    pub fn board(&self) -> Option<&Board> {
        self.board.as_ref()
    }

    /// Whether it's board `uri` on `site`.
    pub fn is(&self, site: &str, uri: &str) -> bool {
        self.site == site && self.board.as_ref().is_some_and(|b| b.uri == uri)
    }

    pub fn posts(&self) -> &[Post] {
        &self.posts
    }

    /// The board thread `p` is on: its own (an overboard's threads are on theirs), else this.
    pub fn board_of(&self, p: &Post) -> String {
        p.board.clone().or_else(|| self.board.as_ref().map(|b| b.uri.clone())).unwrap_or_default()
    }

    /// Thread `p`'s key.
    pub fn key(&self, p: &Post) -> ThreadKey {
        ThreadKey { site: self.site.clone(), board: self.board_of(p), no: p.no }
    }

    /// Show `posts`, marked `marks`.
    pub(super) fn show(&mut self, posts: Vec<Post>, marks: Marks) {
        self.posts = posts;
        self.marks = marks;
    }

    /// Tests' shorthand: `posts` shown on `board` of `site`, unmarked.
    #[cfg(test)]
    pub fn of(site: &str, board: &str, posts: Vec<Post>) -> Self {
        let board = Board { uri: board.to_string(), title: String::new(), nsfw: None };
        Catalog { posts, ..Catalog::new(site.to_string(), board) }
    }
}

/// One tab's place: the active one is `App::tab`. Only `navigate` moves it, and moving it
/// ends the load that was going to land there.
pub struct Tab {
    /// Where the tab is (never Settings: they open over it).
    view: View,
    /// The settings are open over the place.
    settings: bool,
    /// How many times the tab has moved: a move to a place that shows as the same view (a
    /// thread to another) still makes what the last frame drew stale.
    moves: u32,
    /// The tab's last load, whose answers are its own. It outlives its label: a cached
    /// answer's note comes after the answer. `None` once dropped by navigating away.
    load: Option<Load>,
    pub board_list: FilteredList,
    pub catalog_list: FilteredList,
    pub catalog_sort: Sort,
    /// Where `back` goes from a thread opened from Watched or History.
    pub return_to: Option<View>,
    pub thread_checked: Instant,
    /// Background refreshes of the thread in a row that brought nothing (`refresh_backoff`).
    pub thread_quiet: u32,
    pub site: usize,
    pub board: Option<Board>,
    pub catalog: Catalog,
    pub thread: Option<ThreadView>,
    /// The tab's own popup, if any: one at a time.
    pub popup: Option<TabPopup>,
    /// The thread's files as a grid (`V`), over the thread.
    pub gallery: Option<Gallery>,
    /// The jump list: threads left (and the post selected in each), newest last, for `u`;
    /// and those `u` went back from, for `ctrl-r` to go forward to again.
    pub trail: Vec<Trail>,
    pub ahead: Vec<Trail>,
    /// The open thread was opened from the catalog (not by following a link).
    pub from_catalog: bool,
    /// After a thread 404'd: the same thread on the site's configured archive.
    pub archive_offer: Option<ThreadKey>,
    /// After a thread 404'd: its saved copy (`enter` opens it).
    pub saved_offer: Option<ThreadKey>,
    /// The thread shown is a copy, not the live thread.
    pub copy: Option<ThreadCopy>,
    /// Why the tab's last load failed, shown where what it loads would be (until the next).
    pub failed: Option<String>,
    /// The thread last asked for.
    pub pending_thread: Option<ThreadKey>,
    /// Archive search: its results and the list over them.
    pub search: Option<Search>,
    pub search_list: FilteredList,
}

impl Tab {
    pub fn viewer(&self) -> Option<&Viewer> {
        if let Some(TabPopup::Viewer(v)) = &self.popup { Some(v) } else { None }
    }

    pub fn viewer_mut(&mut self) -> Option<&mut Viewer> {
        if let Some(TabPopup::Viewer(v)) = &mut self.popup { Some(v) } else { None }
    }

    /// The saved copy being read, if that's what's shown.
    pub fn saved(&self) -> Option<Offline> {
        if let Some(ThreadCopy::Saved(o)) = self.copy { Some(o) } else { None }
    }

    /// The last copy kept, if that's what's shown while the thread is fetched.
    pub fn cached(&self) -> Option<Offline> {
        if let Some(ThreadCopy::Cached(o)) = self.copy { Some(o) } else { None }
    }

    /// What's shown: the settings, if they're open, else the place.
    pub fn view(&self) -> View {
        if self.settings { View::Settings } else { self.view }
    }

    /// The place, under the settings if they're open.
    pub fn place_view(&self) -> View {
        self.view
    }

    /// Go to `to`. Settings open over the place and change nothing else; anywhere else
    /// closes them, and ends the load, the gallery and the failure of the place left (not
    /// the thread, which search results go back to).
    pub fn navigate(&mut self, to: View) {
        if to == View::Settings {
            self.settings = true;
            return;
        }
        // Leaving a thread puts it on the jump list.
        if to != View::Thread
            && let Some(here) = self.here()
        {
            self.remember(here);
        }
        self.settings = false;
        self.view = to;
        self.moves = self.moves.wrapping_add(1);
        self.load = None;
        self.gallery = None;
        self.failed = None;
    }

    /// The thread shown and its selected post, as the jump list keeps it.
    pub fn here(&self) -> Option<Trail> {
        let t = self.thread.as_ref().filter(|_| self.view == View::Thread)?;
        Some((t.key().clone(), t.current().map_or_else(|| t.key().no, |p| p.no)))
    }

    /// Put `here` on the jump list: each thread once, where it was left last, and the list
    /// no longer than `JUMPS`.
    pub fn remember(&mut self, here: Trail) {
        self.trail.retain(|(k, _)| *k != here.0);
        self.trail.push(here);
        if self.trail.len() > JUMPS {
            self.trail.remove(0);
        }
    }

    /// How many times the tab has moved (see `navigate`), to tell one place from the next.
    pub fn moves(&self) -> u32 {
        self.moves
    }

    /// Close the settings: whether they were open.
    pub fn close_settings(&mut self) -> bool {
        std::mem::take(&mut self.settings)
    }

    /// The tab's request, whose answers are its own.
    pub fn req(&self) -> Option<u64> {
        self.load.as_ref().map(|l| l.id)
    }

    /// The label of the load in flight, if one is.
    pub fn loading(&self) -> Option<&str> {
        self.load.as_ref().and_then(|l| l.label.as_deref())
    }

    /// Start load `id`, shown as `label`, to do `then` with its answer.
    pub(super) fn begin(&mut self, id: u64, label: String, then: Then) {
        self.load = Some(Load { id, label: Some(label), then });
        self.failed = None;
    }

    /// Load `id` was answered: whether that's to be applied (it's still the tab's load, and
    /// wasn't answered before). The load stays the tab's, with what it was to do: its note
    /// may follow, and a failed open is tried again (or saved in the session) as asked.
    pub(super) fn answered(&mut self, id: u64) -> bool {
        self.load.as_mut().filter(|l| l.id == id).and_then(|l| l.label.take()).is_some()
    }

    /// How the thread being loaded is to be opened.
    pub fn opening(&self) -> Opening {
        match self.load.as_ref().map(|l| &l.then) {
            Some(Then::Thread { open }) => *open,
            _ => Opening::default(),
        }
    }

    /// How the thread loaded is opened, for its answer: a restore is answered once.
    pub(super) fn opened(&mut self) -> Opening {
        match self.load.as_mut().map(|l| &mut l.then) {
            Some(Then::Thread { open }) => {
                let was = *open;
                open.restoring = false;
                was
            }
            _ => Opening::default(),
        }
    }

    /// A load in flight, as `begin` starts it (without a request to answer it).
    #[cfg(test)]
    pub fn fake_load(&mut self, id: u64, label: &str, then: Then) {
        self.begin(id, label.to_string(), then);
    }

    pub fn new(site: usize, now: Instant) -> Self {
        Self {
            view: View::Sites,
            settings: false,
            moves: 0,
            load: None,
            board_list: FilteredList::fresh(),
            catalog_list: FilteredList::fresh(),
            catalog_sort: Sort::default(),
            return_to: None,
            thread_checked: now,
            thread_quiet: 0,
            site,
            board: None,
            catalog: Catalog::default(),
            thread: None,
            popup: None,
            gallery: None,
            trail: Vec::new(),
            ahead: Vec::new(),
            from_catalog: false,
            archive_offer: None,
            saved_offer: None,
            copy: None,
            failed: None,
            pending_thread: None,
            search: None,
            search_list: FilteredList::fresh(),
        }
    }
}
