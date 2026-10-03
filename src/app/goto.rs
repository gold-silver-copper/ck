//! `:` and `ck URL`: go to a URL or a short form like `4chan/g/123`.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, Msg, View};
use crate::route::{self, SiteInfo, Target};

impl App {
    fn site_infos(&self) -> Vec<SiteInfo> {
        self.sites.iter().map(|s| SiteInfo::new(&s.cfg, &s.backend.board_url("x"))).collect()
    }

    /// Go where `input` leads, or say why it can't.
    pub fn goto_str(&mut self, input: &str) {
        // The lists by name (before boards that happen to be called that).
        let view = match input.trim().to_lowercase().as_str() {
            "watched" => Some(View::Watched),
            "history" => Some(View::History),
            "saved" => Some(View::Saved),
            _ => None,
        };
        if let Some(view) = view {
            self.tab.gallery = None;
            self.tab.view = view;
            return;
        }
        let board = self.tab.board.as_ref().map(|b| b.uri.clone());
        match route::resolve(input, &self.site_infos(), (self.tab.site, board.as_deref())) {
            Ok(target) => self.go(target),
            // A link to a site ck doesn't have: ask it what it runs, to add it.
            Err(_) if crate::backend::detect::link(input).is_some() => self.add_site_from(input, true),
            Err(e) => self.error(e),
        }
    }

    pub(super) fn go(&mut self, target: Target) {
        let from_thread = self.tab.view == View::Thread;
        // `u` comes back to the thread this was opened from.
        if let (Some(t), Some(b)) = (&self.tab.thread, &self.tab.board)
            && from_thread
            && target.thread.is_some()
        {
            self.tab.trail.push((self.tab.site, b.clone(), t.no, t.current().map_or(t.no, |p| p.no)));
        }
        let back_to = self.tab.view;
        self.switch_site(target.site);
        let Some(uri) = target.board else {
            self.tab.view = View::Boards;
            return;
        };
        let board = self.find_board(&uri);
        match (target.thread, target.post) {
            (Some(no), post) => {
                self.open_thread_at(board, no, post);
                if !from_thread && back_to != View::Settings {
                    self.tab.return_to = Some(back_to);
                }
            }
            (None, Some(post)) => {
                // A post without its thread (FoolFuuka's /post/ links): ask the engine.
                let label = format!("Looking up post {post}");
                let job_board = uri.clone();
                self.spawn(label, move |b, _, _| b.find_thread(&job_board, post), move |id, r| Msg::Found(id, board, post, r));
            }
            (None, None) => {
                self.tab.return_to = None;
                self.open_catalog(board);
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
            KeyCode::Tab => self.complete_goto(),
            code => super::edit_text(text, code),
        }
    }

    /// Text pasted into the terminal: into the input being typed, or a URL to go to.
    pub fn paste(&mut self, text: &str) {
        let text = text.trim().replace(['\n', '\r'], " ");
        if self.paste_adding(&text) {
            return;
        }
        if let Some(g) = &mut self.goto {
            g.push_str(&text);
        } else if let Some(a) = &mut self.filter_add {
            if let Some(label) = &mut a.typing {
                label.push_str(&text);
            }
        } else if let Some(super::settings::Popup::FilterEdit { typing: Some(field), .. }) = &mut self.settings_popup {
            field.push_str(&text);
        } else if self.searching {
            if let Some(t) = &mut self.tab.thread {
                let q = format!("{}{text}", t.search);
                t.set_search(q);
            }
        } else if self.filtering {
            if let Some((p, len)) = self.picker() {
                p.filter.push_str(&text);
                p.clamp(len);
            }
        } else if self.settings_popup.is_none() && !self.show_help && self.tab.viewer.is_none() {
            self.goto = Some(text);
        }
    }

    /// Tab in the `:` input: complete a site name, or a board of the site typed so far.
    fn complete_goto(&mut self) {
        let Some(text) = self.goto.clone() else { return };
        let (prefix, partial, site) = match text.rsplit_once('/') {
            Some((head, tail)) => {
                let name = head.trim_start_matches('/');
                let site = self.sites.iter().position(|s| s.cfg.name.eq_ignore_ascii_case(name)).or(head.is_empty().then_some(self.tab.site));
                (format!("{head}/"), tail.to_string(), site)
            }
            None => (String::new(), text.clone(), None),
        };
        let boards = |site: usize| -> Vec<String> { self.known_boards(site).unwrap_or_default().into_iter().map(|b| b.uri).collect() };
        let candidates: Vec<String> = match site {
            Some(site) => boards(site),
            None => {
                let lists = ["saved", "watched", "history"].map(String::from);
                self.sites.iter().map(|s| s.cfg.name.clone()).chain(boards(self.tab.site)).chain(lists).collect()
            }
        };
        let lower = partial.to_lowercase();
        let matches: Vec<&String> = candidates.iter().filter(|c| c.to_lowercase().starts_with(&lower)).collect();
        match matches.as_slice() {
            [] => self.info(format!("Nothing starts with \"{partial}\"")),
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
                self.info(format!("{}{more}", shown.join("  ")));
            }
        }
    }
}
