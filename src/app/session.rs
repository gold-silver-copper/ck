//! Remembering where you were, and starting there next time.

use std::time::{Duration, Instant};

use super::{App, Picker, Sort, View};
use crate::store::{Place, Session};

/// How often the session is saved while ck runs (if it changed).
const SAVE_EVERY: Duration = Duration::from_secs(30);

fn view_name(v: View) -> &'static str {
    match v {
        View::Sites => "sites",
        View::Boards => "boards",
        View::Catalog => "catalog",
        View::Thread => "thread",
        View::Watched => "watched",
        View::History => "history",
        // Settings and search results aren't places to come back to.
        View::Settings | View::Search => "",
    }
}

impl App {
    /// Where this tab is.
    pub fn place(&self) -> Place {
        // From settings or search results: the view they were opened from.
        let view = match self.tab.view {
            View::Settings => self.tab.settings_back.unwrap_or(View::Sites),
            View::Search => View::Catalog,
            v => v,
        };
        let mut place = Place { view: view_name(view).into(), site: self.current_site().cfg.name.clone(), ..Default::default() };
        match view {
            View::Catalog | View::Thread => {
                place.board = self.tab.board.as_ref().map(|b| b.uri.clone());
                place.sort = (self.tab.catalog_sort != Sort::Bump).then_some(self.tab.catalog_sort);
                place.filter = self.tab.catalog_list.filter.clone();
            }
            _ => {}
        }
        match view {
            View::Thread => {
                let t = self.tab.thread.as_ref();
                place.thread = t.map(|t| t.no).or((self.tab.pending_thread > 0).then_some(self.tab.pending_thread));
                place.selected = t.and_then(|t| t.current()).map(|p| p.no).or(self.tab.pending_post);
            }
            View::Catalog => {
                place.selected = self.tab.catalog_list.state.selected().and_then(|i| self.visible_catalog().get(i).map(|&k| self.tab.catalog[k].no));
            }
            _ => {}
        }
        place
    }

    pub fn session(&mut self) -> Session {
        Session { tabs: self.places(), active: self.active }
    }

    /// Save the session now (on quit), or every so often if it changed.
    pub fn save_session(&mut self, now: Option<Instant>) {
        if !self.restore_session {
            return;
        }
        if let Some(now) = now {
            if now.duration_since(self.session_saved.1) < SAVE_EVERY {
                return;
            }
            self.session_saved.1 = now;
        }
        let session = self.session();
        if self.session_saved.0.as_ref() == Some(&session) {
            return;
        }
        // Not worth an error message; it's tried again later.
        if self.store.save_session(&session).is_ok() {
            self.session_saved.0 = Some(session);
        }
    }

    /// Start where the last run left off, with the same tabs.
    pub fn restore_session(&mut self) {
        let Some(session) = self.store.load_session() else { return };
        self.session_saved.0 = Some(session.clone());
        for (i, place) in session.tabs.iter().take(super::MAX_TABS).enumerate() {
            if i > 0 {
                self.tabs.push(super::Tab::new(0));
                self.switch_tab(i);
            }
            self.go_to_place(place);
        }
        self.switch_tab(session.active.min(self.tabs.len() - 1));
    }

    /// Open a saved place: the view shows at once, and loads like it would by hand.
    pub fn go_to_place(&mut self, p: &Place) {
        let Some(site) = self.sites.iter().position(|s| s.cfg.name == p.site) else { return };
        self.switch_site(site);
        self.tab.catalog_sort = p.sort.unwrap_or_default();
        self.tab.catalog_list = Picker { filter: p.filter.clone(), ..Picker::top() };
        let board = p.board.as_ref().map(|b| self.find_board(b));
        match (p.view.as_str(), board, p.thread) {
            ("watched", ..) => self.tab.view = View::Watched,
            ("history", ..) => self.tab.view = View::History,
            ("boards", ..) => self.enter_site(site),
            ("catalog", Some(board), _) => {
                self.tab.board = Some(board);
                self.tab.catalog.clear();
                self.tab.view = View::Catalog;
                self.tab.pending_catalog = p.selected;
                self.load_catalog();
            }
            ("thread", Some(board), Some(no)) => {
                self.open_thread_at(board, no, p.selected);
                self.tab.restoring = true;
            }
            _ => self.tab.view = View::Sites,
        }
    }
}
