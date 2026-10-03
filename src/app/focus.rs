//! Context: the focused part of a post (`tab`), what `enter` and the verbs do with it, the
//! menu of everything that can be done with what's selected (`.`, right-click), and link
//! hints (`f`) that label what's on screen to jump to it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;

use super::{App, Hit, Part, SiteRow, View, Viewer, list_move};
use crate::download;
use crate::keys::{Action, Scope};
use crate::model::{Link, Target};
use crate::ui::{INDENT, PAD};

/// One row of the menu: `enter` (what it does here), or an action.
#[derive(Debug, Clone)]
pub enum MenuItem {
    Enter(String),
    Act(Action, String),
}

/// The menu for what's selected: its title, rows, and where it was drawn.
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
    pub list: ListState,
    pub area: Rect,
}

/// Labels on what's on screen; typing one picks it.
pub struct Hints {
    pub targets: Vec<HintTarget>,
    pub typed: String,
}

pub struct HintTarget {
    pub label: String,
    pub x: u16,
    pub y: u16,
    pub to: HintTo,
}

#[derive(Debug, Clone)]
pub enum HintTo {
    /// An entry of the thread, or one of its parts.
    Thread(usize, Option<Part>),
    /// A row of the list (or a card of the grid).
    Row(usize),
}

/// Hint labels: one letter while they last, then two.
fn labels(n: usize) -> Vec<String> {
    const KEYS: &[u8] = b"asdfghjklqwertyuiopzxcvbnm";
    let one = |i: usize| (KEYS[i % KEYS.len()] as char).to_string();
    if n <= KEYS.len() {
        (0..n).map(one).collect()
    } else {
        (0..n).map(|i| format!("{}{}", one(i / KEYS.len()), one(i))).collect()
    }
}

impl App {
    /// The focused part of the selected post, in a thread.
    pub fn focused(&self) -> Option<&Part> {
        self.tab.thread.as_ref().filter(|_| self.tab.view == View::Thread && self.tab.gallery.is_none())?.focus.as_ref()
    }

    /// `enter` on a part: an image in the viewer, a quote's post, a URL in the browser, the
    /// replies under the post.
    pub(super) fn activate(&mut self, part: Part) {
        match part {
            Part::File(k) => self.view_file(k),
            Part::Link(Target::Url(u)) => self.open_url(&u),
            Part::Link(Target::Quote(l)) => self.go_to_quote(l),
            Part::Replies => self.act(Action::Expand),
        }
    }

    /// File `k` of the selected post: in the viewer, or opened outside when images are off.
    pub(super) fn view_file(&mut self, k: usize) {
        let Some(files) = self.selected_post().map(|p| p.files.clone()) else { return };
        let Some(file) = files.get(k).cloned() else { return };
        if !self.images.enabled() {
            return self.open_file(&file);
        }
        if self.thread_viewer(k) {
            return;
        }
        let link = self.selected_link();
        self.tab.viewer = Some(Viewer::new(files, k, link));
    }

    /// A quote link: the post, when it's in this thread (`u` comes back); else where it leads.
    fn go_to_quote(&mut self, l: Link) {
        let board = self.tab.board.as_ref().map(|b| b.uri.clone());
        if let Some(t) = &mut self.tab.thread
            && let Some(n) = l.post
            && l.board.as_ref().is_none_or(|b| Some(b) == board.as_ref())
            && l.thread.is_none_or(|th| th == t.no)
            && t.jump_to(n)
        {
            return;
        }
        self.follow(l);
    }

    /// What `y` copies and `o` opens when a part has focus: a file's URL, a link, a quoted
    /// post's address.
    pub(super) fn focused_url(&self) -> Option<(&'static str, String)> {
        let part = self.focused()?;
        let p = self.selected_post()?;
        Some(match part {
            Part::File(k) => ("file URL", p.files.get(*k)?.url.clone()),
            Part::Link(Target::Url(u)) => ("link", u.clone()),
            Part::Link(Target::Quote(l)) => ("link", self.quote_url(l)?),
            Part::Replies => return None,
        })
    }

    /// Where a quote link leads, as a web address.
    fn quote_url(&self, l: &Link) -> Option<String> {
        let t = self.tab.thread.as_ref()?;
        let board = l.board.clone().or_else(|| self.tab.board.as_ref().map(|b| b.uri.clone()))?;
        let backend = &self.current_site().backend;
        let here = l.board.is_none() && l.post.is_some_and(|p| t.index.contains_key(&p));
        Some(match (l.thread, l.post) {
            (Some(th), Some(p)) => backend.post_url(&board, th, p),
            (None, Some(p)) if here => backend.post_url(&board, t.no, p),
            (Some(th), None) => backend.thread_url(&board, th),
            _ => backend.board_url(&board),
        })
    }

    /// `d` on a focused file: just that one.
    pub(super) fn download_focused(&mut self) -> bool {
        let Some(&Part::File(k)) = self.focused() else { return false };
        let Some(t) = &self.tab.thread else { return false };
        let Some(p) = t.current() else { return false };
        let Some(url) = p.files.get(k).map(|f| f.url.clone()) else { return false };
        let dir = download::dir(self.download_dir.as_deref(), &self.current_site().cfg.name, &t.board, t.no);
        let jobs = download::jobs(&[p], &dir).into_iter().filter(|(u, _)| *u == url).collect();
        self.start_download(jobs, dir, "No file to save");
        true
    }

    /// `tab` / `shift-tab` in a thread.
    pub(super) fn step_part(&mut self, forward: bool) {
        if let Some(t) = &mut self.tab.thread
            && !t.step_part(forward)
        {
            self.info(if forward { "No more images or links below" } else { "No more images or links above" });
        }
    }

    /// The part (or just the entry) of the thread drawn at a screen position.
    pub(super) fn thread_part_at(&self, col: u16, row: u16) -> Option<(usize, Option<Part>)> {
        let Some(Hit::Thread { area }) = self.hit else { return None };
        if !area.contains(Position::new(col, row)) {
            return None;
        }
        let t = self.tab.thread.as_ref()?;
        let l = t.layout.as_ref()?;
        let i = t.scroll + (row - area.y) as usize;
        let (e, _) = l.line(i)?;
        let in_block = i - l.starts.get(e)?;
        let x0 = area.x + INDENT * t.entries.get(e)?.depth as u16 + PAD;
        let part = col.checked_sub(x0).and_then(|x| {
            l.spots.get(e)?.iter().find(|s| s.line == in_block && x >= s.col && x < s.col + s.width).map(|s| s.part.clone())
        });
        Some((e, part))
    }

    // ----- the menu -----

    pub(super) fn open_menu(&mut self) {
        let (title, items) = self.menu_items();
        if items.is_empty() {
            return;
        }
        self.menu = Some(Menu { title, items, list: ListState::default().with_selected(Some(0)), area: Rect::default() });
    }

    /// `enter` here, in a few words, when it does something.
    fn enter_label(&self) -> Option<String> {
        let t = self.tab.thread.as_ref()?;
        let p = t.current()?;
        Some(match t.focus.as_ref() {
            Some(Part::File(k)) => {
                let f = p.files.get(*k)?;
                if self.images.enabled() { format!("view {}", f.filename) } else { format!("open {}", f.filename) }
            }
            Some(Part::Link(Target::Url(u))) => format!("open {}", crate::ui::truncate(u, 50)),
            Some(Part::Link(Target::Quote(l))) => match (l.board.as_deref(), l.post) {
                (None, Some(n)) => format!("go to >>{n}"),
                (Some(b), Some(n)) => format!("go to >>>/{b}/{n}"),
                (Some(b), None) => format!("go to /{b}/"),
                (None, None) => return None,
            },
            Some(Part::Replies) if t.expanded.contains(&t.entries.get(t.entry())?.path) => "hide the replies".into(),
            Some(Part::Replies) => "show the replies under it".into(),
            None if p.quotes.iter().any(|q| t.index.contains_key(q)) => "go to the post it quotes".into(),
            None if self.outgoing_link().is_some() => "follow its link".into(),
            None => return None,
        })
    }

    /// Everything that can be done with what's selected, most specific first.
    fn menu_items(&self) -> (String, Vec<MenuItem>) {
        use Action as A;
        let act = |a: Action, label: &str| MenuItem::Act(a, label.to_string());
        let mut items: Vec<MenuItem> = Vec::new();
        let mut title = String::new();
        let watching = |no: u64| self.tab.board.as_ref().is_some_and(|b| self.store.watched(&self.key(&b.uri, no)).is_some());
        if let Some(v) = &self.tab.viewer {
            title = v.files.get(v.index).map(|f| f.filename.clone()).unwrap_or_default();
            items.push(act(A::OpenFile, "open it outside ck"));
            items.push(act(A::ImageSearch, "search for this image"));
            items.push(act(A::Copy, "copy the file's URL"));
            items.push(act(A::CopyLink, "copy the post's link"));
        } else if self.tab.view == View::Thread && self.tab.gallery.is_some() {
            title = "Gallery".into();
            items.push(MenuItem::Enter("view it".into()));
            items.push(act(A::Download, "save it"));
            items.push(act(A::Copy, "copy the file's URL"));
            items.push(act(A::CopyLink, "copy the post's link"));
        } else {
            match self.tab.view {
                View::Thread => {
                    if let Some(t) = &self.tab.thread
                        && let Some(p) = t.current()
                    {
                        let focus = t.focus.as_ref();
                        title = match focus {
                            Some(Part::File(k)) => p.files.get(*k).map_or(String::new(), |f| f.filename.clone()),
                            Some(Part::Link(Target::Url(u))) => crate::ui::truncate(u, 50),
                            Some(Part::Link(Target::Quote(_))) | Some(Part::Replies) | None => format!("No.{}", p.no),
                        };
                        if let Some(label) = self.enter_label() {
                            items.push(MenuItem::Enter(label));
                        }
                        match focus {
                            Some(Part::File(k)) => {
                                items.push(act(A::Download, "save this file"));
                                items.push(act(A::Copy, "copy the file's URL"));
                                items.push(act(A::Browser, "open the file in the browser"));
                                if p.files.get(*k).is_some_and(|f| f.is_image()) {
                                    items.push(act(A::ImageSearch, "search for this image"));
                                }
                            }
                            Some(Part::Link(Target::Url(_))) => items.push(act(A::Copy, "copy the URL")),
                            Some(Part::Link(Target::Quote(_))) => {
                                items.push(act(A::Copy, "copy its address"));
                                items.push(act(A::Browser, "open it in the browser"));
                            }
                            Some(Part::Replies) | None => {}
                        }
                        if !p.files.is_empty() {
                            items.push(act(A::View, "view the post's images"));
                            if !matches!(focus, Some(Part::File(_))) {
                                items.push(act(A::Download, "save the post's files"));
                            }
                        }
                        if focus.is_none() {
                            items.push(act(A::Copy, "copy the post's text"));
                        }
                        items.push(act(A::CopyLink, "copy the post's link"));
                        if focus.is_none() {
                            items.push(act(A::Browser, "open the post in the browser"));
                        }
                        if !p.anchors.is_empty() || !p.files.is_empty() {
                            items.push(act(A::Links, "list its links and files"));
                        }
                        if !p.quotes.is_empty() {
                            items.push(act(A::Preview, "preview the posts it quotes"));
                        }
                        if t.backlinks.get(t.selected).is_some_and(|b| !b.is_empty()) {
                            items.push(act(A::Replies, "go to the first reply"));
                            let open = t.entries.get(t.entry()).is_some_and(|e| t.expanded.contains(&e.path));
                            items.push(act(A::Expand, if open { "hide its replies" } else { "show its replies under it" }));
                        }
                        if p.body.iter().flat_map(|l| &l.spans).any(|s| crate::markup::is_spoiler(s.style)) {
                            items.push(act(A::Spoiler, "show its spoilers"));
                        }
                        items.push(act(A::Mine, if t.mine.contains(&p.no) { "it's not yours" } else { "mark it as yours" }));
                        let hidden = t.marks.get(t.selected).is_some_and(|m| m.hidden.is_some());
                        items.push(act(A::Hide, if hidden { "unhide it" } else { "hide it" }));
                        items.push(act(A::Filter, "hide or highlight posts like it…"));
                        if t.conversation.is_some() {
                            items.push(act(A::Conversation, "the whole thread again"));
                        } else if t.backlinks[t.selected].len() + p.quotes.iter().filter(|q| t.index.contains_key(q)).count() > 0 {
                            items.push(act(A::Conversation, "its conversation alone"));
                        }
                        items.push(act(A::Watch, if watching(t.no) { "stop watching the thread" } else { "watch the thread" }));
                        items.push(act(A::Follow, "follow the thread as a general"));
                        if t.posts.iter().any(|p| !p.files.is_empty()) {
                            items.push(act(A::Gallery, "all the thread's files"));
                            items.push(act(A::DownloadThread, "save all the thread's files"));
                        }
                        if !t.jumps.is_empty() || !self.tab.trail.is_empty() {
                            items.push(act(A::JumpBack, "go back"));
                        }
                        if (0..t.posts.len()).any(|i| t.is_new(i)) {
                            items.push(act(A::Unread, "the first unread post"));
                        }
                        if self.outgoing_link().is_some() {
                            items.push(act(A::NewTab, "open its link in a new tab"));
                        }
                        if self.tab.archive_offer.is_some() {
                            items.push(act(A::Archive, "open the thread in the archive"));
                        }
                        items.push(act(A::Search, "search the thread"));
                        items.push(act(A::Hints, "pick a link or post by its label"));
                        items.push(act(A::Export, "save the thread as HTML and JSON"));
                        match self.tab.offline {
                            Some(o) if o.dead => {}
                            Some(_) => items.push(act(A::Reload, "open the live thread")),
                            None => items.push(act(A::Reload, "reload")),
                        }
                    }
                }
                View::Catalog => {
                    if let Some(p) = self.selected_post() {
                        title = format!("No.{}", p.no);
                        items.push(MenuItem::Enter("open the thread".into()));
                        items.push(act(A::NewTab, "open it in a new tab"));
                        if !p.files.is_empty() {
                            items.push(act(A::View, "view its images"));
                        }
                        items.push(act(A::Watch, if watching(p.no) { "stop watching it" } else { "watch it" }));
                        items.push(act(A::Follow, "follow it as a general"));
                        items.push(act(A::Hide, "hide it (or unhide)"));
                        items.push(act(A::Filter, "hide or highlight threads like it…"));
                        if !p.anchors.is_empty() || !p.files.is_empty() {
                            items.push(act(A::Links, "list its links and files"));
                        }
                        items.push(act(A::Copy, "copy its text"));
                        items.push(act(A::CopyLink, "copy its link"));
                        items.push(act(A::Browser, "open it in the browser"));
                    }
                    items.push(act(A::ShowHidden, if self.show_hidden { "leave out hidden threads" } else { "show hidden threads" }));
                    let sort = self.tab.catalog_sort;
                    items.push(MenuItem::Act(A::Sort, format!("sort by {} (now {})", sort.next().as_str(), sort.as_str())));
                    let layout = self.layout();
                    items.push(MenuItem::Act(A::Compact, format!("{} layout (now {})", layout.next().as_str(), layout.as_str())));
                    items.push(act(A::Favorite, "favorite the board (or not)"));
                    if self.archive_site().is_some() {
                        items.push(act(A::ArchiveSearch, "search the board's archive"));
                    }
                    items.push(act(A::Search, "filter the threads"));
                    items.push(act(A::Hints, "pick a thread by its label"));
                    items.push(act(A::Reload, "reload"));
                }
                View::Sites => {
                    match self.selected_site_row() {
                        Some(SiteRow::Watched) => items.push(MenuItem::Enter("open Watched".into())),
                        Some(SiteRow::History) => items.push(MenuItem::Enter("open History".into())),
                        Some(SiteRow::Saved) => items.push(MenuItem::Enter("open Saved".into())),
                        Some(SiteRow::Favorite(_)) => {
                            items.push(MenuItem::Enter("open the board".into()));
                            items.push(MenuItem::Act(A::Remove, "take it off the favorites".into()));
                        }
                        Some(SiteRow::Recent(_)) => {
                            items.push(MenuItem::Enter("open the board".into()));
                            items.push(MenuItem::Act(A::Remove, "forget it".into()));
                        }
                        Some(SiteRow::Site(i)) => {
                            let name = self.sites.get(i).map_or("", |s| s.cfg.name.as_str());
                            items.push(MenuItem::Enter(format!("open {name}'s boards")));
                            let hidden = self.is_site_hidden(i);
                            items.push(MenuItem::Act(A::Remove, if hidden { "show it on the home screen" } else { "hide it from the home screen" }.into()));
                            items.push(MenuItem::Act(A::Browser, format!("open {name} in the browser")));
                        }
                        Some(SiteRow::HiddenSites) => {
                            items.push(MenuItem::Enter(if self.show_hidden_sites { "hide the hidden sites" } else { "show the hidden sites" }.into()));
                        }
                        None => {}
                    }
                    items.push(act(A::Search, "filter"));
                    items.push(act(A::Hints, "pick a row by its label"));
                }
                View::Boards => {
                    if let Some(i) = self.selected_index() {
                        let uri = self.boards().get(i).map_or(String::new(), |b| b.uri.clone());
                        title = format!("/{uri}/");
                        items.push(MenuItem::Enter(format!("open /{uri}/")));
                        items.push(act(A::Favorite, "favorite it (or not)"));
                        items.push(act(A::Browser, "open it in the browser"));
                    }
                    items.push(act(A::Search, "filter the boards"));
                    items.push(act(A::Hints, "pick a board by its label"));
                    items.push(act(A::Reload, "reload"));
                }
                View::Watched | View::History => {
                    if self.selected_index().is_some() {
                        items.push(MenuItem::Enter("open the thread".into()));
                        items.push(act(A::NewTab, "open it in a new tab"));
                        items.push(act(A::Remove, if self.tab.view == View::Watched { "stop watching it" } else { "forget it" }));
                        items.push(act(A::Copy, "copy its subject and link"));
                        items.push(act(A::CopyLink, "copy its link"));
                        items.push(act(A::Browser, "open it in the browser"));
                        items.push(act(A::Follow, "follow it as a general"));
                    }
                    items.push(act(A::Search, "filter"));
                    items.push(act(A::Hints, "pick a thread by its label"));
                }
                View::Saved => {
                    if let Some(i) = self.selected_index() {
                        title = format!("No.{}", self.store.saved[i].key.no);
                        items.push(MenuItem::Enter("read the saved copy".into()));
                        items.push(act(A::Remove, "remove the saved copy"));
                        items.push(act(A::Copy, "copy its subject and link"));
                        items.push(act(A::CopyLink, "copy its link"));
                        items.push(act(A::Browser, "open it in the browser"));
                    }
                    items.push(act(A::Search, "filter"));
                    items.push(act(A::Hints, "pick a thread by its label"));
                }
                View::Search => {
                    if self.selected_index().is_some() {
                        items.push(MenuItem::Enter("open the thread".into()));
                    }
                    if self.more_results() {
                        items.push(MenuItem::Act(A::NextMatch, "more results".into()));
                    }
                }
                View::Settings => items.push(MenuItem::Enter("change it".into())),
            }
        }
        if self.tabs.len() > 1 {
            items.push(act(A::NextTab, "next tab"));
            items.push(act(A::PrevTab, "previous tab"));
            items.push(act(A::CloseTab, "close the tab"));
        }
        items.push(act(A::Goto, "go to a URL or site/board/thread"));
        items.push(act(A::Settings, "settings"));
        items.push(act(A::Help, "every key"));
        items.push(act(A::Quit, "quit"));
        (title, items)
    }

    /// The key that runs a menu row, as shown.
    pub fn menu_key(&self, item: &MenuItem) -> String {
        match item {
            MenuItem::Enter(_) => "enter".into(),
            MenuItem::Act(a, _) => self.keys.key(*a),
        }
    }

    pub(super) fn on_menu_key(&mut self, key: KeyEvent) {
        let scope = if self.tab.viewer.is_some() { Scope::Viewer } else { self.scope() };
        let action = self.keys.action(scope, &key);
        let Some(m) = &mut self.menu else { return };
        let cur = m.list.selected().unwrap_or(0);
        if let Some(to) = list_move(key.code, cur, m.items.len()) {
            m.list.select(Some(to));
            return;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Right => self.run_menu_item(cur),
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => self.menu = None,
            _ => {
                // A row's own key runs it.
                let row = m.items.iter().position(|it| matches!(it, MenuItem::Act(a, _) if Some(*a) == action));
                match (action, row) {
                    (_, Some(i)) => self.run_menu_item(i),
                    (Some(Action::Menu), None) => self.menu = None,
                    _ => {}
                }
            }
        }
    }

    /// Run row `i`, as if its key were pressed with the menu closed.
    fn run_menu_item(&mut self, i: usize) {
        let Some(m) = self.menu.take() else { return };
        let key = match m.items.get(i) {
            Some(MenuItem::Enter(_)) => KeyEvent::from(KeyCode::Enter),
            Some(MenuItem::Act(a, _)) => match self.keys.keys(*a).first() {
                Some(k) => KeyEvent::new(k.code, k.mods),
                None => return,
            },
            None => return,
        };
        self.on_key(key);
    }

    /// A click on a row runs it; anywhere else closes the menu.
    pub(super) fn on_menu_click(&mut self, col: u16, row: u16) {
        let Some(m) = &self.menu else { return };
        let i = m.list.offset() + row.saturating_sub(m.area.y) as usize;
        if m.area.contains(Position::new(col, row)) && i < m.items.len() {
            self.run_menu_item(i);
        } else {
            self.menu = None;
        }
    }

    // ----- link hints -----

    /// `f`: a label on every part and post (or row) on screen.
    pub(super) fn open_hints(&mut self) {
        let mut at: Vec<(u16, u16, HintTo)> = Vec::new();
        match self.hit {
            Some(Hit::Thread { area }) if self.tab.gallery.is_none() => {
                let Some(t) = &self.tab.thread else { return };
                let Some(l) = &t.layout else { return };
                for row in 0..area.height {
                    let i = t.scroll + row as usize;
                    let Some((e, _)) = l.line(i) else { break };
                    let in_block = i - l.starts[e];
                    let x0 = area.x + INDENT * t.entries.get(e).map_or(0, |e| e.depth) as u16 + PAD;
                    let y = area.y + row;
                    // The post itself, at its header; each part just before it (where
                    // there's usually a space), so what it labels stays readable.
                    if in_block == 1 {
                        at.push((x0.saturating_sub(2), y, HintTo::Thread(e, None)));
                    }
                    for s in l.spots.get(e).into_iter().flat_map(|s| s.iter()).filter(|s| s.line == in_block) {
                        at.push(((x0 + s.col).saturating_sub(1), y, HintTo::Thread(e, Some(s.part.clone()))));
                    }
                }
            }
            Some(Hit::List { area, offset, item_height }) => {
                let len = self.picker_len();
                for r in 0..(area.height / item_height.max(1)) as usize {
                    if offset + r >= len {
                        break;
                    }
                    at.push((area.x, area.y + r as u16 * item_height.max(1), HintTo::Row(offset + r)));
                }
            }
            Some(Hit::Grid { area, offset, cols, cell }) => {
                let len = self.picker_len();
                for r in 0..(area.height / cell.1.max(1)) as usize {
                    for c in 0..cols {
                        let i = offset + r * cols + c;
                        if i < len {
                            at.push((area.x + c as u16 * cell.0, area.y + r as u16 * cell.1, HintTo::Row(i)));
                        }
                    }
                }
            }
            _ => {}
        }
        if at.is_empty() {
            self.info("Nothing to pick here");
            return;
        }
        let targets = labels(at.len()).into_iter().zip(at).map(|(label, (x, y, to))| HintTarget { label, x, y, to }).collect();
        self.hints = Some(Hints { targets, typed: String::new() });
    }

    fn picker_len(&mut self) -> usize {
        self.picker().map_or(0, |(_, len)| len)
    }

    pub(super) fn on_hints_key(&mut self, key: KeyEvent) {
        let Some(h) = &mut self.hints else { return };
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                h.typed.push(c.to_ascii_lowercase());
                let matching: Vec<usize> = (0..h.targets.len()).filter(|&i| h.targets[i].label.starts_with(&h.typed)).collect();
                match matching.as_slice() {
                    // Not a label: the key is ignored.
                    [] => {
                        h.typed.pop();
                    }
                    [i] if h.targets[*i].label == h.typed => {
                        let to = h.targets[*i].to.clone();
                        self.hints = None;
                        self.go_hint(to);
                    }
                    _ => {}
                }
            }
            KeyCode::Backspace if !h.typed.is_empty() => {
                h.typed.pop();
            }
            _ => self.hints = None,
        }
    }

    /// A picked hint: a part is focused and opened, a post selected, a row opened.
    fn go_hint(&mut self, to: HintTo) {
        match to {
            HintTo::Thread(e, part) => {
                let Some(t) = &mut self.tab.thread else { return };
                t.set_cursor(e);
                t.focus = part.clone();
                t.layout = None;
                t.follow_focus = true;
                if let Some(part) = part {
                    self.activate(part);
                }
            }
            HintTo::Row(i) => {
                if let Some((p, len)) = self.picker()
                    && i < len
                {
                    p.state.select(Some(i));
                    self.on_key(KeyEvent::from(KeyCode::Enter));
                }
            }
        }
    }
}
