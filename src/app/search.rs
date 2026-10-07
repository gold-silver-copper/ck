//! `f`: search a board's archive (FoolFuuka), and browse the results.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{App, FilteredList, View};
use crate::store::ThreadKey;
use crate::backend::SearchPage;
use crate::config::SiteKind;
use crate::filter::Filters;
use crate::keys::Action;
use crate::model::Post;
use crate::store::Store;

pub struct Search {
    /// Searching saved threads instead of an archive.
    pub saved: Option<SavedSearch>,
    pub board: String,
    pub query: String,
    /// `(thread, post)` results so far, and how many there are in all.
    pub hits: Vec<(u64, Post)>,
    /// Whether each hit is hidden (beside `hits`): by a filter, a hidden word, or by hand.
    pub hidden: Vec<bool>,
    pub total: Option<u64>,
    /// Pages loaded.
    pub pages: u32,
    /// Where back goes: the site and view the search started from.
    back: (usize, View),
}

/// A search of the saved threads, running or done.
pub struct SavedSearch {
    /// The saved copy each hit is in (beside `hits`).
    pub keys: Vec<ThreadKey>,
    /// Copies read so far, of how many; and those that couldn't be read.
    pub done: usize,
    pub of: usize,
    pub skipped: usize,
    pub finished: bool,
    id: u64,
    /// Set when the search is dropped (another search in its tab, the results left, the tab
    /// closed): the copies left aren't read. Other tabs' searches go on.
    stop: Arc<AtomicBool>,
}

impl SavedSearch {
    #[cfg(test)]
    pub fn for_tests(keys: Vec<ThreadKey>, done: usize, of: usize, finished: bool) -> Self {
        Self { keys, done, of, skipped: 0, finished, id: 0, stop: Arc::default() }
    }
}

impl Drop for SavedSearch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Search {
    #[cfg(test)]
    pub fn for_tests(board: &str, query: &str, page: SearchPage) -> Self {
        Self { saved: None, board: board.into(), query: query.into(), hits: page.hits, hidden: Vec::new(), total: page.total, pages: 1, back: (0, View::Catalog) }
    }

    fn is_hidden(&self, k: usize) -> bool {
        self.hidden.get(k).copied().unwrap_or(false)
    }
}

/// How many results are hidden.
fn hidden_count(s: &Search) -> usize {
    s.hidden.iter().filter(|&&h| h).count()
}

/// Whether a result is hidden: by a filter or hidden word, or by hand on its board.
fn hit_hidden(filters: &Filters, store: &Store, (site, board): (&str, &str), thread: u64, p: &Post) -> bool {
    filters.check(site, board, p, p.no == thread).hidden.is_some() || store.is_hidden(site, board, p.no)
}

/// What a search of the saved threads found in one copy (`None`: nothing, or it couldn't
/// be read: `Err`).
pub enum SavedFound {
    Copy(ThreadKey, Vec<Post>),
    Nothing,
    Unreadable,
    Done,
}

impl App {
    /// The archive to search for the current board: the site itself if it's an archive,
    /// else its configured `archive`.
    pub(super) fn archive_site(&self) -> Option<usize> {
        let site = self.current_site();
        if site.cfg.kind == SiteKind::Foolfuuka {
            return Some(self.tab.site);
        }
        let name = site.cfg.archive.as_ref()?;
        self.site_index(name).filter(|&i| self.sites.get(i).is_some_and(|s| s.cfg.kind == SiteKind::Foolfuuka))
    }

    /// `f` in a catalog: ask what to search for.
    pub fn start_archive_search(&mut self) {
        match self.archive_site() {
            Some(_) => self.typing = Some(super::Typing::ArchiveQuery(String::new())),
            None => self.info("This site has no archive to search (see `archive` in the config)"),
        }
    }

    pub fn on_search_input_key(&mut self, key: KeyEvent) {
        let Some(super::Typing::ArchiveQuery(text)) = &mut self.typing else { return };
        match key.code {
            KeyCode::Esc => self.typing = None,
            KeyCode::Enter => {
                let query = std::mem::take(text).trim().to_string();
                self.typing = None;
                if !query.is_empty() {
                    self.search_archive(query);
                }
            }
            code => super::edit_text(text, code),
        }
    }

    fn search_archive(&mut self, query: String) {
        let (Some(archive), Some(board)) = (self.archive_site(), self.tab.board.as_ref().map(|b| b.uri.clone())) else { return };
        let back = match &self.tab.search {
            Some(s) => s.back,
            None => (self.tab.site, self.tab.view),
        };
        self.switch_site(archive);
        self.tab.search = Some(Search { saved: None, board, query, hits: Vec::new(), hidden: Vec::new(), total: None, pages: 0, back });
        self.tab.search_list = FilteredList::top();
        self.tab.view = View::Search;
        self.load_search_page();
    }

    /// Search every saved copy for `query`, newest first, off the UI thread: results come in
    /// as they're found. Another search in the tab, or leaving the results, stops it.
    pub fn search_saved(&mut self, query: &str) {
        let Some((dir, keys)) = self.store.saved_files() else {
            return self.error("No data folder: nothing is saved");
        };
        let back = match &self.tab.search {
            Some(s) => s.back,
            None => (self.tab.site, if self.tab.view == View::Search { View::Saved } else { self.tab.view }),
        };
        self.saved_search += 1;
        let (id, stop) = (self.saved_search, Arc::new(AtomicBool::new(false)));
        let of = keys.len();
        if of == 0 {
            self.info("Nothing is saved yet: watched threads are saved as they refresh");
        }
        let saved = SavedSearch { keys: Vec::new(), done: 0, of, skipped: 0, finished: of == 0, id, stop: stop.clone() };
        self.tab.search = Some(Search { saved: Some(saved), board: String::new(), query: query.to_string(), hits: Vec::new(), hidden: Vec::new(), total: None, pages: 0, back });
        self.tab.search_list = FilteredList::top();
        self.tab.view = View::Search;
        // Saving waits for nothing: copies being written are read as they were.
        let later = self.later();
        let needle = query.to_lowercase();
        std::thread::spawn(move || {
            for key in keys {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                let found = match std::fs::read(crate::saved::path(&dir, &key)).map_err(anyhow::Error::from).and_then(|b| crate::guard::result(|| crate::saved_search::matching(&b, &needle))) {
                    Ok(posts) if posts.is_empty() => SavedFound::Nothing,
                    Ok(posts) => SavedFound::Copy(key, posts),
                    Err(_) => SavedFound::Unreadable,
                };
                if !later.run(move |app| app.saved_found(id, found)) {
                    return;
                }
            }
            later.run(move |app| app.saved_found(id, SavedFound::Done));
        });
    }

    /// What the search of saved threads found in one more copy.
    pub(super) fn saved_found(&mut self, id: u64, found: SavedFound) {
        let ours = |s: &Option<Search>| s.as_ref().is_some_and(|s| s.saved.as_ref().is_some_and(|x| x.id == id));
        // The tab it was started in, if it isn't the one shown now.
        let here = ours(&self.tab.search);
        let active = self.active;
        let tab = if here { Some(&mut self.tab) } else { self.tabs.iter_mut().enumerate().find(|(i, t)| *i != active && ours(&t.search)).map(|(_, t)| t) };
        let Some(s) = tab.and_then(|t| t.search.as_mut()) else { return };
        let Some(saved) = s.saved.as_mut() else { return };
        match found {
            SavedFound::Copy(key, posts) => {
                saved.done += 1;
                for p in posts {
                    s.hidden.push(hit_hidden(&self.filters, &self.store, (&key.site, &key.board), key.no, &p));
                    saved.keys.push(key.clone());
                    s.hits.push((key.no, p));
                }
            }
            SavedFound::Nothing => saved.done += 1,
            SavedFound::Unreadable => {
                saved.done += 1;
                saved.skipped += 1;
            }
            SavedFound::Done => {
                saved.finished = true;
                if here {
                    let msg = match hidden_count(s) {
                        _ if s.hits.is_empty() => format!("No saved post matches \"{}\"", s.query),
                        n if n == s.hits.len() && !self.show_hidden => self.all_hidden(n),
                        _ => return,
                    };
                    self.info(msg);
                }
            }
        }
    }

    /// Fetch the next page of results (one request, like any other user action).
    pub fn load_search_page(&mut self) {
        let Some(s) = &self.tab.search else { return };
        if s.saved.is_some() {
            let query = s.query.clone();
            return self.search_saved(&query);
        }
        let (board, query, page) = (s.board.clone(), s.query.clone(), s.pages + 1);
        let label = if page == 1 { format!("Searching /{board}/ for \"{query}\"") } else { format!("Loading page {page} of results") };
        self.spawn(label, move |b, _, _| b.search(&board, &query, page), move |app, r| app.search_results(page, r));
    }

    /// A page of archive search results arrived.
    pub fn search_results(&mut self, page: u32, res: anyhow::Result<SearchPage>) {
        self.tab.loading = None;
        let site = self.current_site().cfg.name.clone();
        let Some(s) = &mut self.tab.search else { return };
        match res {
            Ok(p) => {
                s.pages = page;
                s.total = p.total.or(s.total);
                for (thread, p) in &p.hits {
                    s.hidden.push(hit_hidden(&self.filters, &self.store, (&site, p.board.as_deref().unwrap_or(&s.board)), *thread, p));
                }
                s.hits.extend(p.hits);
                let msg = match hidden_count(s) {
                    _ if s.hits.is_empty() => format!("No posts on /{}/ match \"{}\"", s.board, s.query),
                    n if n == s.hits.len() && !self.show_hidden => self.all_hidden(n),
                    _ => return,
                };
                self.info(msg);
            }
            Err(e) => self.error(e),
        }
    }

    /// The results shown: hidden ones only when `Z` shows them.
    pub fn visible_hits(&self) -> Vec<usize> {
        let Some(s) = &self.tab.search else { return Vec::new() };
        (0..s.hits.len()).filter(|&k| self.show_hidden || !s.is_hidden(k)).collect()
    }

    /// Mark the results again (a filter, a hidden word or a post hidden by hand changed).
    pub(super) fn remark_search(&mut self) {
        let site = self.current_site().cfg.name.clone();
        let Some(s) = &mut self.tab.search else { return };
        let keys = s.saved.as_ref().map(|x| &x.keys);
        s.hidden = s
            .hits
            .iter()
            .enumerate()
            .map(|(k, (thread, p))| match keys.and_then(|keys| keys.get(k)) {
                Some(key) => hit_hidden(&self.filters, &self.store, (&key.site, &key.board), *thread, p),
                None => hit_hidden(&self.filters, &self.store, (&site, p.board.as_deref().unwrap_or(&s.board)), *thread, p),
            })
            .collect();
    }

    /// Every result found is hidden: say so, and how to see them.
    fn all_hidden(&self, n: usize) -> String {
        let results = if n == 1 { "1 result".to_string() } else { format!("{n} results") };
        format!("{results}, all hidden ({} shows them)", self.keys.key(Action::ShowHidden))
    }

    /// More results to load past the ones shown.
    pub fn more_results(&self) -> bool {
        self.tab.search.as_ref().is_some_and(|s| s.saved.is_none() && s.total.is_some_and(|t| (s.hits.len() as u64) < t) && s.pages > 0)
    }

    /// Reaching the end of the results loads the next page.
    pub fn search_moved(&mut self) {
        let at_end = self.tab.search_list.state.selected().is_some_and(|i| i + 1 >= self.visible_hits().len());
        if at_end && self.more_results() && self.tab.loading.is_none() {
            self.load_search_page();
        }
    }

    /// Enter on a result: its thread on the archive, with the post selected.
    pub fn open_search_hit(&mut self) {
        let Some(k) = self.tab.search_list.state.selected().and_then(|i| self.visible_hits().get(i).copied()) else { return };
        let Some(s) = &self.tab.search else { return };
        // A saved copy: opened offline, on the post, with the search set for n / N.
        if let Some(saved) = &s.saved {
            let (Some(key), Some((_, post))) = (saved.keys.get(k).cloned(), s.hits.get(k)) else { return };
            let (no, query) = (post.no, s.query.clone());
            self.open_saved(&key);
            if let Some(t) = self.tab.thread.as_mut().filter(|_| self.tab.view == View::Thread) {
                if let Some(&i) = t.index.get(&no) {
                    t.select(i);
                }
                t.set_search(query);
                self.tab.return_to = Some(View::Search);
            }
            return;
        }
        let Some((thread, post)) = s.hits.get(k) else { return };
        let (thread, no) = (*thread, post.no);
        let board = self.find_board(post.board.as_deref().unwrap_or(&s.board));
        self.open_thread_at(board, thread, Some(no));
        self.tab.return_to = Some(View::Search);
    }

    /// Leave the results for where the search started.
    pub fn close_search(&mut self) -> View {
        // A search of saved threads still running stops (as it's dropped).
        let Some(s) = self.tab.search.take() else { return View::Sites };
        self.switch_site(s.back.0);
        s.back.1
    }
}
