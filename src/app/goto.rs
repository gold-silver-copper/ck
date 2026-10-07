//! `:` and `ck URL`: go to a URL or a short form like `4chan/g/123`.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, Popup, View};
use crate::config::SiteConfig;
use crate::model::Board;
use crate::route::{self, SiteInfo, Target};

/// The lists, by name (before boards that happen to be called that).
fn list_named(input: &str) -> Option<View> {
    match input.trim().to_lowercase().as_str() {
        "watched" => Some(View::Watched),
        "history" => Some(View::History),
        "saved" => Some(View::Saved),
        _ => None,
    }
}

/// `saved WORDS`: the words to search the saved threads for.
fn saved_query(input: &str) -> Option<&str> {
    input.trim().strip_prefix("saved ").map(str::trim).filter(|q| !q.is_empty())
}

/// Why ck can't start at `input` (`ck URL`), if it can't, to say before the terminal is
/// taken over. A link to a site that isn't one of `sites` is fine: ck asks it, to add it.
pub fn start_error(sites: &[SiteConfig], input: &str) -> Option<String> {
    if list_named(input).is_some() || saved_query(input).is_some() {
        return None;
    }
    let infos: Vec<SiteInfo> = sites.iter().map(|c| SiteInfo::new(c, &crate::backend::build(c).board_url("x"))).collect();
    if crate::backend::detect::link(input).is_some_and(|l| !infos.iter().any(|s| s.hosts.iter().any(|h| h == l.host.strip_prefix("www.").unwrap_or(&l.host)))) {
        return None;
    }
    route::resolve(input, &infos, (0, None)).err().map(|e| format!("{e:#}"))
}

impl App {
    fn site_infos(&self) -> Vec<SiteInfo> {
        self.sites.iter().map(|s| SiteInfo::new(&s.cfg, &s.backend.board_url("x"))).collect()
    }

    /// Go where `input` leads, or say why it can't.
    pub fn goto_str(&mut self, input: &str) {
        if let Some(view) = list_named(input) {
            self.tab.gallery = None;
            self.tab.view = view;
            return;
        }
        if let Some(query) = saved_query(input) {
            self.tab.gallery = None;
            return self.search_saved(query);
        }
        // A link to a site ck doesn't have (`somechan.org/b/`, not a board called that): ask
        // it what it runs, to add it.
        let infos = self.site_infos();
        if let Some(link) = crate::backend::detect::link(input) {
            let host = link.host.strip_prefix("www.").unwrap_or(&link.host);
            if !infos.iter().any(|s| s.hosts.iter().any(|h| h == host)) {
                return self.add_site_from(input, true);
            }
        }
        let board = self.tab.board.as_ref().map(|b| b.uri.clone());
        match route::resolve(input, &infos, (self.tab.site, board.as_deref())) {
            Ok(target) => self.go(target),
            Err(e) => self.error(e),
        }
    }

    pub(super) fn go(&mut self, target: Target) {
        let from_thread = self.tab.view == View::Thread;
        // `u` comes back to the thread this was opened from.
        if let Some(b) = self.tab.board.clone().filter(|_| from_thread && target.thread.is_some()) {
            self.leave_trail(b);
        }
        let back_to = self.tab.view;
        let Some(uri) = target.board else {
            self.switch_site(target.site);
            self.tab.view = View::Boards;
            return;
        };
        match (target.thread, target.post) {
            (Some(no), post) => {
                self.switch_site(target.site);
                self.open_thread_at(self.find_board(&uri), no, post);
                if !from_thread && back_to != View::Settings {
                    self.tab.return_to = Some(back_to);
                }
            }
            (None, Some(post)) => {
                // A post without its thread (FoolFuuka's /post/ links): ask the engine of the
                // post's site. The tab moves there only once it's found; `u` comes back here.
                let Some(backend) = self.sites.get(target.site).map(|s| s.backend.clone()) else { return };
                let board = self.known_boards(target.site).and_then(|b| b.into_iter().find(|b| b.uri == uri));
                let board = board.unwrap_or_else(|| Board { uri: uri.clone(), title: String::new(), nsfw: None });
                let trail = self.tab.board.clone().filter(|_| from_thread).and_then(|b| self.trail_here(b));
                let (label, site) = (format!("Looking up post {post}"), target.site);
                self.spawn(label, move |_, _, _| backend.find_thread(&uri, post), move |app, r| app.thread_found(site, board, post, trail, r));
            }
            (None, None) => {
                self.switch_site(target.site);
                self.tab.return_to = None;
                self.open_catalog(self.find_board(&uri));
            }
        }
    }

    pub fn on_goto_key(&mut self, key: KeyEvent) {
        let Some(super::Typing::Goto(text)) = &mut self.typing else { return };
        match key.code {
            KeyCode::Esc => self.typing = None,
            KeyCode::Enter => {
                let input = std::mem::take(text);
                self.typing = None;
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
        if let Some(super::Typing::Goto(g)) = &mut self.typing {
            g.push_str(&text);
        } else if let Some(Popup::AddFilter(a)) = &mut self.popup {
            if let Some(label) = &mut a.typing {
                label.push_str(&text);
            }
        } else if let Some(Popup::Settings(super::SettingsPopup::FilterEdit { typing: Some(field), .. })) = &mut self.popup {
            field.push_str(&text);
        } else if self.typing == Some(super::Typing::ThreadSearch) {
            if let Some(t) = &mut self.tab.thread {
                let q = format!("{}{text}", t.search);
                t.set_search(q);
            }
        } else if self.typing == Some(super::Typing::ListFilter) {
            if let Some((p, len)) = self.filtered_list() {
                p.filter.push_str(&text);
                p.clamp(len);
            }
        } else if !matches!(self.popup, Some(Popup::Settings(_) | Popup::Help(_))) && self.tab.viewer().is_none() {
            self.typing = Some(super::Typing::Goto(text));
        }
    }

    /// Tab in the `:` input: complete a site name, or a board of the site typed so far.
    fn complete_goto(&mut self) {
        let Some(text) = self.goto_text().map(String::from) else { return };
        let (prefix, partial, site) = match text.rsplit_once('/') {
            Some((head, tail)) => {
                let name = head.trim_start_matches('/');
                let site = self.sites.iter().position(|s| s.cfg.name.eq_ignore_ascii_case(name)).or_else(|| head.is_empty().then_some(self.tab.site));
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
                let slash = if site.is_none() && self.site_index(one).is_some() { "/" } else { "" };
                self.typing = Some(super::Typing::Goto(format!("{prefix}{one}{slash}")));
            }
            many @ [first, rest @ ..] => {
                let common = rest.iter().fold((*first).clone(), |acc, m| {
                    acc.chars().zip(m.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect()
                });
                if common.chars().count() > partial.chars().count() {
                    self.typing = Some(super::Typing::Goto(format!("{prefix}{common}")));
                }
                let shown: Vec<&str> = many.iter().take(10).map(|s| s.as_str()).collect();
                let more = if many.len() > 10 { format!(" (+{})", many.len() - 10) } else { String::new() };
                self.info(format!("{}{more}", shown.join("  ")));
            }
        }
    }
}
