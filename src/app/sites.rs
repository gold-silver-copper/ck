//! Adding sites: from a link to any page of one (pasted after `:`, or typed in Settings ›
//! Sites), ck asks the site what it runs and, once you've named it, adds a `[[site]]` to
//! the config. A link to a board a vichan site you have doesn't list adds the board.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;

use super::{App, Popup, Site, edit_text, list_move};
use crate::backend::{self, detect};
use crate::config::{self, BoardConfig, SiteConfig, SiteKind};

/// Adding a site, step by step.
pub enum Adding {
    /// Typing a link to it.
    Typing(String),
    /// Asking the site what it runs (request `id`). `open` is the link to go to after.
    Looking { id: u64, host: String, open: Option<String> },
    /// What it runs; its name can be changed before it's added.
    Site { site: SiteConfig, name: String, open: Option<String> },
    /// A board of a vichan site you have, which its list doesn't have.
    Board { site: usize, board: String, open: Option<String> },
    /// A vichan site's board list read again: what changes. Boards gone from the bar are
    /// kept unless `drop`. `builtin`: a built-in site not in the config yet, which this makes
    /// one of yours.
    Boards { site: usize, update: BoardsUpdate, drop: bool, builtin: bool },
}

/// How a vichan site's board bar differs from its list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardsUpdate {
    /// In the bar, not the list.
    pub added: Vec<BoardConfig>,
    /// In the list, not the bar.
    pub missing: Vec<BoardConfig>,
    /// In both, with another title in the bar: (uri, the list's title, the bar's).
    pub renamed: Vec<(String, String, String)>,
    /// The bar, in its order.
    pub bar: Vec<BoardConfig>,
}

impl BoardsUpdate {
    pub fn new(list: &[BoardConfig], bar: Vec<BoardConfig>) -> Self {
        let title = |b: &BoardConfig| match b {
            BoardConfig::Full { title, .. } => title.clone(),
            BoardConfig::Uri(_) => String::new(),
        };
        let added = bar.iter().filter(|b| !list.iter().any(|l| l.uri() == b.uri())).cloned().collect();
        let missing = list.iter().filter(|l| !bar.iter().any(|b| b.uri() == l.uri())).cloned().collect();
        let renamed = bar
            .iter()
            .filter_map(|b| {
                let old = list.iter().find(|l| l.uri() == b.uri())?;
                (title(old) != title(b) && !title(b).is_empty()).then(|| (b.uri().to_string(), title(old), title(b)))
            })
            .collect();
        BoardsUpdate { added, missing, renamed, bar }
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.missing.is_empty() && self.renamed.is_empty()
    }

    /// The new list: the bar's, in its order, then the boards it doesn't have (unless dropped).
    pub fn list(&self, drop: bool) -> Vec<BoardConfig> {
        let mut out = self.bar.clone();
        if !drop {
            out.extend(self.missing.iter().cloned());
        }
        out
    }
}

/// What asking a site found.
pub enum Detected {
    Site(SiteConfig),
    /// That this site (by index) has this board.
    Board(usize, String),
    /// A vichan site's (by index) board bar.
    Bar(usize, Vec<BoardConfig>),
}

/// Your config's `[[site]]` tables, as Settings › Sites lists them.
pub struct MySites {
    pub list: ListState,
    pub sites: Vec<SiteConfig>,
    /// The row `x` was pressed on once: a second `x` removes it.
    pub armed: Option<usize>,
}

/// How a site of your config relates to the built-in ones.
pub fn origin(site: &SiteConfig) -> &'static str {
    match config::builtin_sites().iter().find(|b| b.name.eq_ignore_ascii_case(&site.name)) {
        None => "added",
        Some(b) if b == site => "same as built-in",
        Some(_) => "changes the built-in",
    }
}

impl App {
    /// Start adding a site from a link (from `:` when `open`, then going there).
    pub(super) fn add_site_from(&mut self, input: &str, open: bool) {
        let Some(link) = detect::link(input) else {
            return self.error("That isn't a link to a site: paste the address of any page of it");
        };
        let open = open.then(|| input.trim().to_string());
        let known = self.sites.iter().position(|s| self.site_hosts(s).iter().any(|h| *h == link.host.trim_start_matches("www.")));
        let Some(i) = known else {
            self.popup = Some(Popup::Adding(Adding::Looking { id: self.next_request(), host: link.host.clone(), open }));
            let (id, later) = (self.next_id, self.later());
            std::thread::spawn(move || {
                let res = detect::detect(&link).map(Detected::Site);
                later.run(move |app| app.detected(id, res));
            });
            return;
        };
        let site = &self.sites[i].cfg;
        let missing = match (&link.board, &site.boards) {
            (Some(b), Some(list)) if site.kind == SiteKind::Vichan && !list.iter().any(|l| l.uri() == b) => Some(b.clone()),
            _ => None,
        };
        let Some(board) = missing else {
            take_popup!(self, Adding);
            let name = site.name.clone();
            self.info(format!("{name} is already one of your sites"));
            if let Some(o) = open {
                self.goto_str(&o);
            }
            return;
        };
        self.popup = Some(Popup::Adding(Adding::Looking { id: self.next_request(), host: link.host.clone(), open }));
        let (id, later, base) = (self.next_id, self.later(), link.base.clone());
        std::thread::spawn(move || {
            let res = if detect::has_board(&base, &board) { Ok(Detected::Board(i, board)) } else { Err(anyhow::anyhow!("{base} has no /{board}/ (its catalog isn't there)")) };
            later.run(move |app| app.detected(id, res));
        });
    }

    fn next_request(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// The hosts a site's pages are on.
    fn site_hosts(&self, s: &Site) -> Vec<String> {
        crate::route::SiteInfo::new(&s.cfg, &s.backend.board_url("x")).hosts
    }

    /// What asking a site found, if it's still wanted.
    pub(super) fn detected(&mut self, id: u64, res: anyhow::Result<Detected>) {
        let Some(Popup::Adding(Adding::Looking { id: want, open, .. })) = &mut self.popup else { return };
        if *want != id {
            return;
        }
        let open = open.take();
        let next = match res {
            Ok(Detected::Site(site)) => Some(Adding::Site { name: self.free_name(&site.name), site, open }),
            Ok(Detected::Board(site, board)) => Some(Adding::Board { site, board, open }),
            Ok(Detected::Bar(site, bar)) => {
                let list = self.sites.get(site).and_then(|s| s.cfg.boards.clone()).unwrap_or_default();
                let update = BoardsUpdate::new(&list, bar);
                if update.is_empty() {
                    let name = self.sites.get(site).map_or(String::new(), |s| s.cfg.name.clone());
                    self.info(format!("{name}'s board list is up to date"));
                    None
                } else {
                    let name = self.sites.get(site).map_or(String::new(), |s| s.cfg.name.clone());
                    let mine = self.config_path.as_ref().and_then(|p| config::sites_in(p).ok()).is_some_and(|s| s.iter().any(|s| s.name.eq_ignore_ascii_case(&name)));
                    let builtin = !mine && config::builtin_sites().iter().any(|b| b.name.eq_ignore_ascii_case(&name));
                    Some(Adding::Boards { site, update, drop: false, builtin })
                }
            }
            Err(e) => {
                self.error(e);
                None
            }
        };
        self.popup = next.map(Popup::Adding);
    }

    /// `name`, or with a number after it if a site has it.
    fn free_name(&self, name: &str) -> String {
        let taken = |n: &str| self.sites.iter().any(|s| s.cfg.name.eq_ignore_ascii_case(n));
        (1..).map(|k| if k == 1 { name.to_string() } else { format!("{name}-{k}") }).find(|n| !taken(n)).unwrap_or_default()
    }

    /// Why a name can't be a new site's, if it can't.
    fn name_problem(&self, name: &str) -> Option<String> {
        if name.is_empty() {
            return Some("A site needs a name".into());
        }
        if name.contains(|c: char| c == '/' || c == '#' || c.is_whitespace()) {
            return Some("A site's name can't have /, # or spaces (it's typed after :, as name/board)".into());
        }
        if ["saved", "watched", "history"].contains(&name.to_lowercase().as_str()) {
            return Some(format!("\"{name}\" is taken by a list (:{name} opens it)"));
        }
        self.sites.iter().any(|s| s.cfg.name.eq_ignore_ascii_case(name)).then(|| format!("A site is already called {name}"))
    }

    /// Keys while adding a site.
    pub(super) fn on_adding_key(&mut self, key: KeyEvent) {
        let Some(adding) = take_popup!(self, Adding) else { return };
        let next = match (adding, key.code) {
            (_, KeyCode::Esc) => None,
            (Adding::Typing(text), KeyCode::Enter) => {
                self.add_site_from(&text, false);
                return;
            }
            (Adding::Typing(mut text), code) => {
                edit_text(&mut text, code);
                Some(Adding::Typing(text))
            }
            (Adding::Site { site, name, open }, KeyCode::Enter) => match self.name_problem(name.trim()) {
                Some(problem) => {
                    self.error(problem);
                    Some(Adding::Site { site, name, open })
                }
                None => return self.add_site(SiteConfig { name: name.trim().to_string(), ..site }, open),
            },
            (Adding::Site { site, mut name, open }, code) => {
                edit_text(&mut name, code);
                Some(Adding::Site { site, name, open })
            }
            (Adding::Board { site, board, open }, KeyCode::Enter) => return self.add_board(site, board, open),
            (Adding::Boards { site, update, drop, .. }, KeyCode::Enter) => return self.update_boards(site, &update, drop),
            (Adding::Boards { site, update, drop, builtin }, KeyCode::Char('d')) => Some(Adding::Boards { site, update, drop: !drop, builtin }),
            (other, _) => Some(other),
        };
        if let Some(a) = next {
            self.popup = Some(Popup::Adding(a));
        }
    }

    /// Paste into the link or the name being typed.
    pub(super) fn paste_adding(&mut self, text: &str) -> bool {
        match &mut self.popup {
            Some(Popup::Adding(Adding::Typing(t) | Adding::Site { name: t, .. })) => {
                t.push_str(text);
                true
            }
            Some(Popup::Adding(_)) => true,
            _ => false,
        }
    }

    fn add_site(&mut self, site: SiteConfig, open: Option<String>) {
        let what = format!("{} ({})", site.name, site.kind.label());
        let saved = self.try_save_config(&format!("the site {what}"), |d| config::add_site(d, &site));
        self.sites.push(Site { backend: backend::build(&site), cfg: site, boards: None });
        match open {
            Some(o) => self.goto_str(&o),
            None if saved => self.info(format!("Added {what}: it's on the home screen")),
            None => {}
        }
    }

    fn add_board(&mut self, i: usize, board: String, open: Option<String>) {
        let Some(s) = self.sites.get(i) else { return };
        let before = s.cfg.clone();
        let mut boards = before.boards.clone().unwrap_or_default();
        boards.push(BoardConfig::Uri(board.clone()));
        let what = format!("{}'s board /{board}/", before.name);
        let saved = self.try_save_config(&what, |d| config::set_site_boards(d, &before, &boards));
        let cfg = SiteConfig { boards: Some(boards), ..before };
        self.sites[i] = Site { backend: backend::build(&cfg), cfg, boards: None };
        match open {
            Some(o) => self.goto_str(&o),
            None if saved => self.info(format!("Added {what}")),
            None => {}
        }
    }

    /// Read a vichan site's board bar again (one page: its front page, or its first board's
    /// if that has none), to update its list.
    pub(super) fn refresh_board_list(&mut self, site: usize) {
        let Some(s) = self.sites.get(site) else { return };
        let (Some(list), Some(base)) = (s.cfg.boards.clone(), s.cfg.url.clone()) else {
            return self.info(format!("{} gets its boards from the site", s.cfg.name));
        };
        if s.cfg.kind != SiteKind::Vichan {
            return self.info(format!("{} gets its boards from the site", s.cfg.name));
        }
        let base = base.trim_end_matches('/').to_string();
        let host = crate::http::host(&base).to_string();
        self.popup = Some(Popup::Adding(Adding::Looking { id: self.next_request(), host: host.clone(), open: None }));
        let (id, later) = (self.next_id, self.later());
        let first = list.first().map(|b| b.uri().to_string());
        std::thread::spawn(move || {
            let bar = |path: &str| crate::http::get_text(&format!("{base}{path}")).map(|html| detect::boardlist(&html, &host)).unwrap_or_default();
            let mut found = bar("/");
            if found.is_empty()
                && let Some(b) = first
            {
                found = bar(&format!("/{}/index.html", crate::http::encode_segment(&b)));
            }
            let res = if found.is_empty() { Err(anyhow::anyhow!("{host}'s pages have no board list to read")) } else { Ok(Detected::Bar(site, found)) };
            later.run(move |app| app.detected(id, res));
        });
    }

    fn update_boards(&mut self, i: usize, update: &BoardsUpdate, drop: bool) {
        let Some(s) = self.sites.get(i) else { return };
        let before = s.cfg.clone();
        let boards = update.list(drop);
        let what = format!("{}'s board list", before.name);
        let saved = self.try_save_config(&what, |d| config::set_site_boards(d, &before, &boards));
        let cfg = SiteConfig { boards: Some(boards), ..before };
        self.sites[i] = Site { backend: backend::build(&cfg), cfg, boards: None };
        if saved {
            self.info(format!("Updated {what}"));
        }
    }

    /// `save_config` with an edit that can refuse; whether it was saved.
    fn try_save_config(&mut self, what: &str, f: impl FnOnce(&mut toml_edit::DocumentMut) -> anyhow::Result<()>) -> bool {
        let res = self
            .config_path
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no home directory to keep a config file in"))
            .and_then(|path| config::try_edit_at(&path, f).map(|()| super::tilde(&path.display().to_string())));
        match res {
            Ok(path) => {
                self.info(format!("Saved {what} in {path}"));
                true
            }
            Err(e) => {
                self.error(format!("Added {what} for now; couldn't save it: {e:#}"));
                false
            }
        }
    }

    /// Settings › Sites › Your sites: the config's `[[site]]` tables.
    pub(super) fn my_sites(&mut self) -> Option<MySites> {
        let Some(path) = self.config_path.clone() else {
            self.error("No config file: no home directory to keep one in");
            return None;
        };
        match config::sites_in(&path) {
            Ok(sites) => Some(MySites { list: ListState::default().with_selected(Some(0)), sites, armed: None }),
            Err(e) => {
                self.error(format!("{e:#}"));
                None
            }
        }
    }

    /// Keys in Your sites: `a` adds one, `x` twice removes one.
    pub(super) fn on_my_sites_key(&mut self, key: KeyEvent, mut m: MySites) -> Option<MySites> {
        let cur = m.list.selected().unwrap_or(0);
        if let Some(to) = list_move(key.code, cur, m.sites.len()) {
            m.list.select(Some(to));
            m.armed = None;
            return Some(m);
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left => None,
            KeyCode::Char('a') => {
                self.popup = Some(Popup::Adding(Adding::Typing(String::new())));
                None
            }
            // A vichan site: read its board list again.
            KeyCode::Char('r') => {
                let name = m.sites.get(cur).map(|s| s.name.clone()).unwrap_or_default();
                match self.sites.iter().position(|s| s.cfg.name.eq_ignore_ascii_case(&name)) {
                    Some(i) => {
                        self.refresh_board_list(i);
                        None
                    }
                    None => Some(m),
                }
            }
            KeyCode::Char('x') | KeyCode::Delete if m.armed == Some(cur) => {
                let Some(old) = m.sites.get(cur).cloned() else { return Some(m) };
                let what = format!("the site {}", old.name);
                let back = origin(&old) != "added";
                if self.try_save_removal(&what, cur, &old) {
                    if back {
                        self.info(format!("Removed your {}: the built-in one is back from the next start", old.name));
                    } else {
                        self.removed_sites.insert(old.name.clone());
                        self.info(format!("Removed {} from your config", old.name));
                    }
                }
                self.my_sites()
            }
            KeyCode::Char('x') | KeyCode::Delete if cur < m.sites.len() => {
                m.armed = Some(cur);
                let name = &m.sites[cur].name;
                self.info(format!("x again removes {name} from your config"));
                Some(m)
            }
            _ => Some(m),
        }
    }

    fn try_save_removal(&mut self, what: &str, i: usize, old: &SiteConfig) -> bool {
        let Some(path) = self.config_path.clone() else { return false };
        match config::try_edit_at(&path, |d| config::remove_site(d, i, old)) {
            Ok(()) => true,
            Err(e) => {
                self.error(format!("Couldn't remove {what}: {e:#}"));
                false
            }
        }
    }
}
