//! `:` and `ck URL`: go to a URL or a short form like `4chan/g/123`.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, Msg, Picker, View};
use crate::model::Board;
use crate::route::{self, SiteInfo, Target};

impl App {
    fn site_infos(&self) -> Vec<SiteInfo> {
        self.sites.iter().map(|s| SiteInfo::new(&s.cfg, &s.backend.board_url("x"))).collect()
    }

    /// Go where `input` leads, or say why it can't.
    pub fn goto_str(&mut self, input: &str) {
        let board = self.board.as_ref().map(|b| b.uri.clone());
        match route::resolve(input, &self.site_infos(), (self.site, board.as_deref())) {
            Ok(target) => self.go(target),
            Err(e) => self.status = Some((format!("{e:#}"), true)),
        }
    }

    pub(super) fn go(&mut self, target: Target) {
        let from_thread = self.view == View::Thread;
        // `u` comes back to the thread this was opened from.
        if let (Some(t), Some(b)) = (&self.thread, &self.board)
            && from_thread
            && target.thread.is_some()
        {
            self.trail.push((self.site, b.clone(), t.no, t.current().map_or(t.no, |p| p.no)));
        }
        let back_to = self.view;
        self.switch_site(target.site);
        let Some(uri) = target.board else {
            self.view = View::Boards;
            return;
        };
        let board = self.find_board(&uri);
        match (target.thread, target.post) {
            (Some(no), post) => {
                self.open_thread_at(board, no, post, false);
                if !from_thread && back_to != View::Settings {
                    self.return_to = Some(back_to);
                }
            }
            (None, Some(post)) => {
                // A post without its thread (FoolFuuka's /post/ links): ask the engine.
                let label = format!("Looking up post {post}");
                let job_board = uri.clone();
                self.spawn(label, move |b, _, _| b.find_thread(&job_board, post), move |id, r| Msg::Found(id, board, post, r));
            }
            (None, None) => {
                self.board = Some(board);
                self.catalog.clear();
                self.catalog_list = Picker::default();
                self.catalog_list.state.select(Some(0));
                self.return_to = None;
                self.view = View::Catalog;
                self.load_catalog();
            }
        }
    }

    pub fn on_goto_key(&mut self, key: KeyEvent) {
        let Some(text) = &mut self.goto else { return };
        match key.code {
            KeyCode::Esc => self.goto = None,
            KeyCode::Enter => {
                let input = std::mem::take(text);
                self.goto = None;
                self.goto_str(&input);
            }
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Tab => self.complete_goto(),
            KeyCode::Char(c) => text.push(c),
            _ => {}
        }
    }

    /// Text pasted into the terminal: into the input being typed, or a URL to go to.
    pub fn paste(&mut self, text: &str) {
        let text = text.trim().replace(['\n', '\r'], " ");
        if let Some(g) = &mut self.goto {
            g.push_str(&text);
        } else if self.searching {
            if let Some(t) = &mut self.thread {
                let q = format!("{}{text}", t.search);
                t.set_search(q);
            }
        } else if self.filtering {
            if let Some((p, len)) = self.picker() {
                p.filter.push_str(&text);
                p.clamp(len);
            }
        } else if self.settings.popup.is_none() && !self.show_help && self.viewer.is_none() {
            self.goto = Some(text);
        }
    }

    /// Tab in the `:` input: complete a site name, or a board of the site typed so far.
    fn complete_goto(&mut self) {
        let Some(text) = self.goto.clone() else { return };
        let (prefix, partial, site) = match text.rsplit_once('/') {
            Some((head, tail)) => {
                let name = head.trim_start_matches('/');
                let site = self.sites.iter().position(|s| s.cfg.name.eq_ignore_ascii_case(name)).or(head.is_empty().then_some(self.site));
                (format!("{head}/"), tail.to_string(), site)
            }
            None => (String::new(), text.clone(), None),
        };
        let boards = |site: usize| -> Vec<String> {
            let s = &self.sites[site];
            if let Some(b) = &s.boards {
                return b.iter().map(|b| b.uri.clone()).collect();
            }
            if let Some(b) = &s.cfg.boards {
                return b.iter().map(|b| crate::backend::to_board(b).uri).collect();
            }
            let saved: Vec<Board> = self.store.load_boards(&s.cfg.name).map(|(b, _)| b).unwrap_or_default();
            saved.into_iter().map(|b| b.uri).collect()
        };
        let candidates: Vec<String> = match site {
            Some(site) => boards(site),
            None => self.sites.iter().map(|s| s.cfg.name.clone()).chain(boards(self.site)).collect(),
        };
        let lower = partial.to_lowercase();
        let matches: Vec<&String> = candidates.iter().filter(|c| c.to_lowercase().starts_with(&lower)).collect();
        match matches.as_slice() {
            [] => self.status = Some((format!("Nothing starts with \"{partial}\""), false)),
            [one] => {
                let slash = if site.is_none() && self.sites.iter().any(|s| s.cfg.name == **one) { "/" } else { "" };
                self.goto = Some(format!("{prefix}{one}{slash}"));
            }
            many => {
                let common = many.iter().skip(1).fold(many[0].clone(), |acc, m| {
                    acc.chars().zip(m.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect()
                });
                if common.chars().count() > partial.chars().count() {
                    self.goto = Some(format!("{prefix}{common}"));
                }
                let shown: Vec<&str> = many.iter().take(10).map(|s| s.as_str()).collect();
                let more = if many.len() > 10 { format!(" (+{})", many.len() - 10) } else { String::new() };
                self.status = Some((format!("{}{more}", shown.join("  ")), false));
            }
        }
    }
}
