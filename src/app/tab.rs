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
    /// Just show it (a board list, search results, the thread a post is in).
    Show,
    /// Select the catalog thread `select` once it's there.
    Catalog { select: Option<u64> },
    /// Open the thread as `open` says.
    Thread { open: Opening },
}

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

/// One tab's place: the active one is `App::tab`. Only `navigate` moves it, and moving it
/// ends the load that was going to land there.
pub struct Tab {
    /// Where the tab is (never Settings: they open over it).
    view: View,
    /// The settings are open over the place.
    settings: bool,
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
    pub catalog: Vec<Post>,
    /// What filters and hiding say about each catalog thread.
    pub catalog_marks: Marks,
    /// Catalog threads that weren't there on the previous visit.
    pub catalog_new: HashSet<u64>,
    pub thread: Option<ThreadView>,
    /// The tab's own popup, if any: one at a time.
    pub popup: Option<TabPopup>,
    /// The thread's files as a grid (`V`), over the thread.
    pub gallery: Option<Gallery>,
    /// Threads left by following cross-thread links, for `u`.
    pub trail: Vec<Trail>,
    /// Board the loaded catalog belongs to.
    pub catalog_board: Option<String>,
    /// The site the loaded catalog is from.
    pub catalog_site: usize,
    /// The site the open thread is from (an archive search moves the tab to the archive).
    pub thread_site: usize,
    /// The board whose catalog is loaded, to return to from a thread opened on another board
    /// (an overboard's threads live on their own boards).
    pub catalog_of: Option<Board>,
    /// The open thread was opened from the catalog (not by following a link).
    pub from_catalog: bool,
    /// After a thread 404'd: the same thread on the site's configured archive.
    pub archive_offer: Option<ThreadKey>,
    /// After a thread 404'd: its saved copy (`enter` opens it).
    pub saved_offer: Option<ThreadKey>,
    /// The thread shown is a copy, not the live thread.
    pub copy: Option<ThreadCopy>,
    /// The same for the catalog.
    pub catalog_cached: Option<Offline>,
    /// Why the tab's last load failed, shown where what it loads would be (until the next).
    pub failed: Option<String>,
    /// The thread number of the last thread load, for 404 handling.
    pub pending_thread: Option<u64>,
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
        self.settings = false;
        self.view = to;
        self.load = None;
        self.gallery = None;
        self.failed = None;
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

    /// The thread the catalog being loaded is to select, if it's still to come.
    pub fn catalog_selecting(&self) -> Option<u64> {
        match self.load.as_ref().map(|l| &l.then) {
            Some(Then::Catalog { select }) => *select,
            _ => None,
        }
    }

    /// The same, to change.
    pub(super) fn catalog_select(&mut self) -> Option<&mut Option<u64>> {
        match self.load.as_mut().map(|l| &mut l.then) {
            Some(Then::Catalog { select }) => Some(select),
            _ => None,
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
            load: None,
            board_list: FilteredList::top(),
            catalog_list: FilteredList::default(),
            catalog_sort: Sort::default(),
            return_to: None,
            thread_checked: now,
            thread_quiet: 0,
            site,
            board: None,
            catalog: Vec::new(),
            catalog_marks: Marks::default(),
            catalog_new: HashSet::new(),
            thread: None,
            popup: None,
            gallery: None,
            trail: Vec::new(),
            catalog_board: None,
            catalog_site: site,
            thread_site: site,
            catalog_of: None,
            from_catalog: false,
            archive_offer: None,
            saved_offer: None,
            copy: None,
            catalog_cached: None,
            failed: None,
            pending_thread: None,
            search: None,
            search_list: FilteredList::default(),
        }
    }
}
