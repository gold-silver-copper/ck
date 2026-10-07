//! One tab's place and what it shows.

use std::collections::HashSet;
use std::time::Instant;

use super::{FilteredList, Gallery, Marks, Search, Sort, ThreadView, View, Viewer};
use super::tabs::{Offline, TabPopup, ThreadCopy, Trail};
use crate::model::{Board, Post};
use crate::store::ThreadKey;

/// One tab's place: the active one is `App::tab`.
pub struct Tab {
    pub view: View,
    /// Where esc goes back to from Settings.
    pub settings_back: Option<View>,
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
    /// Label of the in-flight request, if any.
    pub loading: Option<String>,
    /// The tab's own popup, if any: one at a time.
    pub popup: Option<TabPopup>,
    /// The thread's files as a grid (`V`), over the thread.
    pub gallery: Option<Gallery>,
    /// Threads left by following cross-thread links, for `u`.
    pub trail: Vec<Trail>,
    /// Post to select once the loading thread arrives.
    pub pending_post: Option<u64>,
    /// Post whose conversation to show once the loading thread arrives (a session's).
    pub pending_conversation: Option<u64>,
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
    /// The tab's last request, whose answers are its own. It outlives `loading`: a cached
    /// answer's note comes after the answer. `None` once dropped by navigating away.
    pub req: Option<u64>,
    /// Why the tab's last load failed, shown where what it loads would be (until the next).
    pub failed: Option<String>,
    /// The thread number of the last thread load, for 404 handling.
    pub pending_thread: Option<u64>,
    /// Thread to select in the catalog once it loads (restoring a session).
    pub pending_catalog: Option<u64>,
    /// The open thread is being restored from the last session.
    pub restoring: bool,
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

    pub fn new(site: usize, now: Instant) -> Self {
        Self {
            view: View::Sites,
            settings_back: None,
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
            loading: None,
            popup: None,
            gallery: None,
            trail: Vec::new(),
            pending_post: None,
            catalog_board: None,
            catalog_site: site,
            thread_site: site,
            catalog_of: None,
            from_catalog: false,
            archive_offer: None,
            saved_offer: None,
            pending_conversation: None,
            copy: None,
            catalog_cached: None,
            req: None,
            failed: None,
            pending_thread: None,
            pending_catalog: None,
            restoring: false,
            search: None,
            search_list: FilteredList::default(),
        }
    }
}
