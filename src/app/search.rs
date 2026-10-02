//! `f`: search a board's archive (FoolFuuka), and browse the results.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, Msg, Picker, View};
use crate::backend::SearchPage;
use crate::config::SiteKind;
use crate::model::Post;

pub struct Search {
    pub board: String,
    pub query: String,
    /// `(thread, post)` results so far, and how many there are in all.
    pub hits: Vec<(u64, Post)>,
    pub total: Option<u64>,
    /// Pages loaded.
    pub pages: u32,
    /// Where back goes: the site and view the search started from.
    back: (usize, View),
}

impl Search {
    #[cfg(test)]
    pub fn for_tests(board: &str, query: &str, page: SearchPage) -> Self {
        Self { board: board.into(), query: query.into(), hits: page.hits, total: page.total, pages: 1, back: (0, View::Catalog) }
    }
}

impl App {
    /// The archive to search for the current board: the site itself if it's an archive,
    /// else its configured `archive`.
    fn archive_site(&self) -> Option<usize> {
        let site = self.current_site();
        if site.cfg.kind == SiteKind::Foolfuuka {
            return Some(self.site);
        }
        let name = site.cfg.archive.as_ref()?;
        self.sites.iter().position(|s| s.cfg.name == *name && s.cfg.kind == SiteKind::Foolfuuka)
    }

    /// `f` in a catalog: ask what to search for.
    pub fn start_archive_search(&mut self) {
        match self.archive_site() {
            Some(_) => self.search_input = Some(String::new()),
            None => self.info("This site has no archive to search (see `archive` in the config)"),
        }
    }

    pub fn on_search_input_key(&mut self, key: KeyEvent) {
        let Some(text) = &mut self.search_input else { return };
        match key.code {
            KeyCode::Esc => self.search_input = None,
            KeyCode::Enter => {
                let query = std::mem::take(text).trim().to_string();
                self.search_input = None;
                if !query.is_empty() {
                    self.search_archive(query);
                }
            }
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c) => text.push(c),
            _ => {}
        }
    }

    fn search_archive(&mut self, query: String) {
        let (Some(archive), Some(board)) = (self.archive_site(), self.board.as_ref().map(|b| b.uri.clone())) else { return };
        let back = match &self.search {
            Some(s) => s.back,
            None => (self.site, self.view),
        };
        self.switch_site(archive);
        self.search = Some(Search { board, query, hits: Vec::new(), total: None, pages: 0, back });
        self.search_list = Picker::default();
        self.search_list.state.select(Some(0));
        self.view = View::Search;
        self.load_search_page();
    }

    /// Fetch the next page of results (one request, like any other user action).
    pub fn load_search_page(&mut self) {
        let Some(s) = &self.search else { return };
        let (board, query, page) = (s.board.clone(), s.query.clone(), s.pages + 1);
        let label = if page == 1 { format!("Searching /{board}/ for \"{query}\"") } else { format!("Loading page {page} of results") };
        self.spawn(label, move |b, _, _| b.search(&board, &query, page), move |id, r| Msg::Search(id, page, r));
    }

    pub fn search_results(&mut self, page: u32, res: anyhow::Result<SearchPage>) {
        let Some(s) = &mut self.search else { return };
        match res {
            Ok(p) => {
                s.pages = page;
                s.total = p.total.or(s.total);
                s.hits.extend(p.hits);
                if s.hits.is_empty() {
                    let msg = format!("No posts on /{}/ match \"{}\"", s.board, s.query);
                    self.info(msg);
                }
            }
            Err(e) => self.error(e),
        }
    }

    /// More results to load past the ones shown.
    pub fn more_results(&self) -> bool {
        self.search.as_ref().is_some_and(|s| s.total.is_some_and(|t| (s.hits.len() as u64) < t) && s.pages > 0)
    }

    /// Reaching the end of the results loads the next page.
    pub fn search_moved(&mut self) {
        let at_end = self.search_list.state.selected().is_some_and(|i| i + 1 >= self.search.as_ref().map_or(0, |s| s.hits.len()));
        if at_end && self.more_results() && self.loading.is_none() {
            self.load_search_page();
        }
    }

    /// Enter on a result: its thread on the archive, with the post selected.
    pub fn open_search_hit(&mut self) {
        let Some(s) = &self.search else { return };
        let Some((thread, post)) = self.search_list.state.selected().and_then(|i| s.hits.get(i)) else { return };
        let (thread, no) = (*thread, post.no);
        let board = self.find_board(post.board.as_deref().unwrap_or(&s.board));
        self.open_thread_at(board, thread, Some(no), false);
        self.return_to = Some(View::Search);
    }

    /// Leave the results for where the search started.
    pub fn close_search(&mut self) -> View {
        let Some(s) = self.search.take() else { return View::Sites };
        self.switch_site(s.back.0);
        s.back.1
    }
}
