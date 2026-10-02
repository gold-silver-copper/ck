//! Remembering where you were, and starting there next time.

use std::time::{Duration, Instant};

use super::{App, Picker, Sort, View};
use crate::store::{Place, Session};

const SORTS: [Sort; 4] = [Sort::Bump, Sort::Replies, Sort::Newest, Sort::Oldest];

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
        let view = match self.view {
            View::Settings => self.settings.back.unwrap_or(View::Sites),
            View::Search => View::Catalog,
            v => v,
        };
        let mut place = Place { view: view_name(view).into(), site: self.current_site().cfg.name.clone(), ..Default::default() };
        match view {
            View::Catalog | View::Thread => {
                place.board = self.board.as_ref().map(|b| b.uri.clone());
                place.sort = (self.catalog_sort != Sort::Bump).then(|| self.catalog_sort.label().to_string());
                place.filter = self.catalog_list.filter.clone();
            }
            _ => {}
        }
        match view {
            View::Thread => {
                let t = self.thread.as_ref();
                place.thread = t.map(|t| t.no).or((self.pending_thread > 0).then_some(self.pending_thread));
                place.selected = t.map(|t| t.posts[t.selected].no).or(self.pending_post);
            }
            View::Catalog => {
                place.selected = self.catalog_list.state.selected().and_then(|i| self.visible_catalog().get(i).map(|&k| self.catalog[k].no));
            }
            _ => {}
        }
        place
    }

    pub fn session(&self) -> Session {
        Session { tabs: vec![self.place()], active: 0 }
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

    /// Start where the last run left off.
    pub fn restore_session(&mut self) {
        let Some(session) = self.store.load_session() else { return };
        self.session_saved.0 = Some(session.clone());
        if let Some(place) = session.tabs.get(session.active).or(session.tabs.first()) {
            self.go_to_place(place);
        }
    }

    /// Open a saved place: the view shows at once, and loads like it would by hand.
    pub fn go_to_place(&mut self, p: &Place) {
        let Some(site) = self.sites.iter().position(|s| s.cfg.name == p.site) else { return };
        self.switch_site(site);
        self.catalog_sort = SORTS.into_iter().find(|s| Some(s.label()) == p.sort.as_deref()).unwrap_or_default();
        self.catalog_list = Picker { filter: p.filter.clone(), ..Default::default() };
        self.catalog_list.state.select(Some(0));
        let board = p.board.as_ref().map(|b| self.find_board(b));
        match (p.view.as_str(), board, p.thread) {
            ("watched", ..) => self.view = View::Watched,
            ("history", ..) => self.view = View::History,
            ("boards", ..) => self.enter_site(site),
            ("catalog", Some(board), _) => {
                self.board = Some(board);
                self.catalog.clear();
                self.view = View::Catalog;
                self.pending_catalog = p.selected;
                self.load_catalog();
            }
            ("thread", Some(board), Some(no)) => {
                self.open_thread_at(board, no, p.selected, false);
                self.restoring = true;
            }
            _ => self.view = View::Sites,
        }
    }
}
