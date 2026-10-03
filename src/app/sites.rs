//! Adding sites: from a link to any page of one (pasted after `:`, or typed in Settings ›
//! Sites), ck asks the site what it runs and, once you've named it, adds a `[[site]]` to
//! the config. A link to a board a vichan site you have doesn't list adds the board.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;

use super::{App, Msg, Site, edit_text, list_move};
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
}

/// What asking a site found.
pub enum Detected {
    Site(SiteConfig),
    /// That this site (by index) has this board.
    Board(usize, String),
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
            self.adding = Some(Adding::Looking { id: self.next_request(), host: link.host.clone(), open });
            let (id, tx) = (self.next_id, self.tx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Detected(id, detect::detect(&link).map(Detected::Site)));
            });
            return;
        };
        let site = &self.sites[i].cfg;
        let missing = match (&link.board, &site.boards) {
            (Some(b), Some(list)) if site.kind == SiteKind::Vichan && !list.iter().any(|l| board_uri(l) == b) => Some(b.clone()),
            _ => None,
        };
        let Some(board) = missing else {
            self.adding = None;
            let name = site.name.clone();
            self.info(format!("{name} is already one of your sites"));
            if let Some(o) = open {
                self.goto_str(&o);
            }
            return;
        };
        self.adding = Some(Adding::Looking { id: self.next_request(), host: link.host.clone(), open });
        let (id, tx, base) = (self.next_id, self.tx.clone(), link.base.clone());
        std::thread::spawn(move || {
            let res = if detect::has_board(&base, &board) { Ok(Detected::Board(i, board)) } else { Err(anyhow::anyhow!("{base} has no /{board}/ (its catalog isn't there)")) };
            let _ = tx.send(Msg::Detected(id, res));
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
        let Some(Adding::Looking { id: want, open, .. }) = &mut self.adding else { return };
        if *want != id {
            return;
        }
        let open = open.take();
        self.adding = match res {
            Ok(Detected::Site(site)) => Some(Adding::Site { name: self.free_name(&site.name), site, open }),
            Ok(Detected::Board(site, board)) => Some(Adding::Board { site, board, open }),
            Err(e) => {
                self.error(e);
                None
            }
        };
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
        let Some(adding) = self.adding.take() else { return };
        self.adding = match (adding, key.code) {
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
            (other, _) => Some(other),
        };
    }

    /// Paste into the link or the name being typed.
    pub(super) fn paste_adding(&mut self, text: &str) -> bool {
        match &mut self.adding {
            Some(Adding::Typing(t) | Adding::Site { name: t, .. }) => {
                t.push_str(text);
                true
            }
            Some(_) => true,
            None => false,
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
                self.adding = Some(Adding::Typing(String::new()));
                None
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

fn board_uri(b: &BoardConfig) -> &str {
    match b {
        BoardConfig::Uri(uri) | BoardConfig::Full { uri, .. } => uri,
    }
}
