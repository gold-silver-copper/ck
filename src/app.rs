use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;
use ratatui::widgets::ListState;

use crate::backend::{self, Backend};
use crate::config::{Config, SiteConfig};
use crate::http;
use crate::images::Images;
use crate::model::{Attachment, Board, Post};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Sites,
    Boards,
    Catalog,
    Thread,
}

pub struct Site {
    pub cfg: SiteConfig,
    pub backend: Arc<dyn Backend>,
    pub boards: Option<Vec<Board>>,
}

/// A filterable list with a selection.
#[derive(Default)]
pub struct Picker {
    pub state: ListState,
    pub filter: String,
}

impl Picker {
    fn move_by(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.state.select(None);
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        self.state.select(Some((cur + delta).clamp(0, len as isize - 1) as usize));
    }

    fn clamp(&mut self, len: usize) {
        self.move_by(0, len);
    }
}

pub struct ThreadView {
    pub board: String,
    pub no: u64,
    pub posts: Vec<Post>,
    pub index: HashMap<u64, usize>,
    /// For each post, the posts that quote it.
    pub backlinks: Vec<Vec<u64>>,
    pub selected: usize,
    pub scroll: usize,
    jumps: Vec<usize>,
    /// Rendered layout, rebuilt by the UI when the width changes.
    pub layout: Option<ThreadLayout>,
    pub viewport: usize,
}

pub struct ThreadLayout {
    pub width: u16,
    pub lines: Vec<Line<'static>>,
    /// `starts[i]` is the first line of post `i`; has one extra entry for the end.
    pub starts: Vec<usize>,
    /// `(line, post)` for each post drawn with a thumbnail in the left column.
    pub thumbs: Vec<(usize, usize)>,
}

impl ThreadView {
    fn new(board: String, no: u64, posts: Vec<Post>) -> Self {
        let index: HashMap<u64, usize> = posts.iter().enumerate().map(|(i, p)| (p.no, i)).collect();
        let mut backlinks = vec![Vec::new(); posts.len()];
        for p in &posts {
            for q in &p.quotes {
                if let Some(&i) = index.get(q)
                    && !backlinks[i].contains(&p.no)
                {
                    backlinks[i].push(p.no);
                }
            }
        }
        Self { board, no, posts, index, backlinks, selected: 0, scroll: 0, jumps: Vec::new(), layout: None, viewport: 0 }
    }

    fn select(&mut self, i: usize) {
        self.selected = i.min(self.posts.len().saturating_sub(1));
        self.scroll_to_selected();
    }

    /// Adjust scroll so the selected post is visible (its top, if it's taller than the view).
    pub fn scroll_to_selected(&mut self) {
        let Some(l) = &self.layout else { return };
        let (start, end) = (l.starts[self.selected], l.starts[self.selected + 1]);
        if start < self.scroll {
            self.scroll = start;
        } else if end > self.scroll + self.viewport {
            self.scroll = start.min(end.saturating_sub(self.viewport));
        }
    }

    /// Scroll by lines, then select the post at the top of the view.
    fn scroll_lines(&mut self, delta: isize) {
        let Some(l) = &self.layout else { return };
        let max = l.lines.len().saturating_sub(self.viewport);
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
        let top = l.starts.partition_point(|&s| s <= self.scroll).saturating_sub(1);
        // Prefer a post whose header is on screen.
        self.selected = if l.starts[top] < self.scroll && top + 1 < self.posts.len() && l.starts[top + 1] < self.scroll + self.viewport {
            top + 1
        } else {
            top
        };
    }

    fn jump_to(&mut self, no: u64) -> bool {
        match self.index.get(&no) {
            Some(&i) => {
                self.jumps.push(self.selected);
                self.select(i);
                true
            }
            None => false,
        }
    }
}

/// Full-screen viewer over one post's files.
pub struct Viewer {
    pub files: Vec<Attachment>,
    pub index: usize,
}

enum Msg {
    /// The request was answered from the cache without hitting the network.
    Cached(u64, Duration),
    Boards(u64, usize, Result<Vec<Board>>),
    Catalog(u64, Result<Vec<Post>>),
    Thread(u64, Result<Vec<Post>>),
}

pub struct App {
    pub sites: Vec<Site>,
    pub view: View,
    pub site_list: Picker,
    pub board_list: Picker,
    pub catalog_list: Picker,
    pub site: usize,
    pub board: Option<Board>,
    pub catalog: Vec<Post>,
    pub thread: Option<ThreadView>,
    /// True while typing into the filter.
    pub filtering: bool,
    /// Label of the in-flight request, if any.
    pub loading: Option<String>,
    pub status: Option<(String, bool)>,
    pub show_help: bool,
    pub help_scroll: u16,
    pub images: Images,
    pub viewer: Option<Viewer>,
    pub tick: usize,
    pub quit: bool,
    req: u64,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl App {
    pub fn new(cfg: Config, picker: Option<ratatui_image::picker::Picker>) -> Self {
        let sites = cfg
            .sites
            .into_iter()
            .map(|cfg| Site { backend: backend::build(&cfg), cfg, boards: None })
            .collect();
        let (tx, rx) = channel();
        let mut app = Self {
            sites,
            view: View::Sites,
            site_list: Picker::default(),
            board_list: Picker::default(),
            catalog_list: Picker::default(),
            site: 0,
            board: None,
            catalog: Vec::new(),
            thread: None,
            filtering: false,
            loading: None,
            status: None,
            show_help: false,
            help_scroll: 0,
            images: Images::new(picker),
            viewer: None,
            tick: 0,
            quit: false,
            req: 0,
            tx,
            rx,
        };
        app.site_list.state.select(Some(0));
        app
    }

    // ----- visible (filtered) items -----

    pub fn visible_sites(&self) -> Vec<usize> {
        filtered(&self.site_list.filter, self.sites.iter().map(|s| s.cfg.name.clone()))
    }

    pub fn boards(&self) -> &[Board] {
        self.sites[self.site].boards.as_deref().unwrap_or(&[])
    }

    pub fn visible_boards(&self) -> Vec<usize> {
        filtered(&self.board_list.filter, self.boards().iter().map(|b| format!("{} {}", b.uri, b.title)))
    }

    pub fn visible_catalog(&self) -> Vec<usize> {
        filtered(
            &self.catalog_list.filter,
            self.catalog.iter().map(|p| format!("{} {}", p.subject.as_deref().unwrap_or(""), p.plain_text())),
        )
    }

    fn picker(&mut self) -> Option<(&mut Picker, usize)> {
        let len = match self.view {
            View::Sites => self.visible_sites().len(),
            View::Boards => self.visible_boards().len(),
            View::Catalog => self.visible_catalog().len(),
            View::Thread => return None,
        };
        let p = match self.view {
            View::Sites => &mut self.site_list,
            View::Boards => &mut self.board_list,
            View::Catalog => &mut self.catalog_list,
            View::Thread => unreachable!(),
        };
        Some((p, len))
    }

    pub fn current_site(&self) -> &Site {
        &self.sites[self.site]
    }

    // ----- background loading -----

    pub fn poll(&mut self) {
        self.images.poll();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Cached(id, age) if id == self.req && self.status.is_none() => {
                    self.status = Some((format!("Up to date (checked {}s ago)", age.as_secs()), false));
                }
                Msg::Boards(id, site, res) => {
                    if id == self.req {
                        self.loading = None;
                    }
                    match res {
                        Ok(b) => {
                            self.sites[site].boards = Some(b);
                            if site == self.site {
                                let len = self.visible_boards().len();
                                self.board_list.clamp(len);
                            }
                        }
                        Err(e) => self.error(e),
                    }
                }
                Msg::Catalog(id, res) if id == self.req => {
                    self.loading = None;
                    match res {
                        Ok(posts) => {
                            self.catalog = posts;
                            let len = self.visible_catalog().len();
                            self.catalog_list.clamp(len);
                        }
                        Err(e) => self.error(e),
                    }
                }
                Msg::Thread(id, res) if id == self.req => {
                    self.loading = None;
                    match res {
                        Ok(posts) => self.set_thread(posts),
                        Err(e) if http::is_not_found(&e) => {
                            self.status = Some(("Thread was deleted or archived".into(), true));
                        }
                        Err(e) => self.error(e),
                    }
                }
                _ => {} // stale response
            }
        }
    }

    fn error(&mut self, e: anyhow::Error) {
        self.status = Some((format!("{e:#}"), true));
    }

    fn spawn<T: Send + 'static>(
        &mut self,
        label: String,
        job: impl FnOnce(&dyn Backend) -> Result<T> + Send + 'static,
        wrap: impl FnOnce(u64, Result<T>) -> Msg + Send + 'static,
    ) {
        self.req += 1;
        let id = self.req;
        let backend = self.current_site().backend.clone();
        let tx = self.tx.clone();
        self.loading = Some(label);
        self.status = None;
        std::thread::spawn(move || {
            let res = job(&*backend);
            let cached = http::take_cached_age();
            let _ = tx.send(wrap(id, res));
            if let Some(age) = cached {
                let _ = tx.send(Msg::Cached(id, age));
            }
        });
    }

    fn load_boards(&mut self) {
        let site = self.site;
        let label = format!("Loading boards for {}", self.current_site().cfg.name);
        self.spawn(label, |b| b.boards(), move |id, r| Msg::Boards(id, site, r));
    }

    fn load_catalog(&mut self) {
        let Some(board) = self.board.clone() else { return };
        self.spawn(format!("Loading /{}/", board.uri), move |b| b.catalog(&board.uri), Msg::Catalog);
    }

    fn load_thread(&mut self, no: u64) {
        let Some(board) = self.board.clone() else { return };
        self.spawn(format!("Loading thread {no}"), move |b| b.thread(&board.uri, no), Msg::Thread);
    }

    fn set_thread(&mut self, posts: Vec<Post>) {
        let Some(board) = self.board.as_ref().map(|b| b.uri.clone()) else { return };
        let no = posts.first().map(|p| p.no).unwrap_or(0);
        let mut tv = ThreadView::new(board, no, posts);
        // On refresh, keep position.
        if let Some(old) = self.thread.take().filter(|t| t.no == no && t.board == tv.board) {
            tv.selected = tv.index.get(&old.posts[old.selected].no).copied().unwrap_or(0);
            tv.scroll = old.scroll;
            tv.viewport = old.viewport;
        }
        self.thread = Some(tv);
    }

    // ----- input -----

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.show_help {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => self.help_scroll = self.help_scroll.saturating_add(1),
                KeyCode::Char('k') | KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                _ => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
            }
            return;
        }
        if self.viewer.is_some() {
            self.on_viewer_key(key.code);
            return;
        }
        if self.filtering {
            self.on_filter_key(key);
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('/') if self.view != View::Thread => self.filtering = true,
            KeyCode::Char('r') | KeyCode::F(5) => self.refresh(),
            KeyCode::Char('o') => self.open_in_browser(),
            KeyCode::Char('v') if matches!(self.view, View::Catalog | View::Thread) => self.open_viewer(),
            KeyCode::Esc => {
                if let Some((p, _)) = self.picker().filter(|(p, _)| !p.filter.is_empty()) {
                    p.filter.clear();
                    p.state.select(Some(0));
                } else {
                    self.back();
                }
            }
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => self.back(),
            _ if self.view == View::Thread => self.on_thread_key(key.code, ctrl),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.enter(),
            code => {
                let Some((p, len)) = self.picker() else { return };
                match code {
                    KeyCode::Char('j') | KeyCode::Down => p.move_by(1, len),
                    KeyCode::Char('k') | KeyCode::Up => p.move_by(-1, len),
                    KeyCode::Char('d') if ctrl => p.move_by(10, len),
                    KeyCode::Char('u') if ctrl => p.move_by(-10, len),
                    KeyCode::PageDown => p.move_by(10, len),
                    KeyCode::PageUp => p.move_by(-10, len),
                    KeyCode::Char('g') | KeyCode::Home => p.state.select(Some(0)),
                    KeyCode::Char('G') | KeyCode::End => p.move_by(isize::MAX / 2, len),
                    _ => {}
                }
            }
        }
    }

    fn on_filter_key(&mut self, key: KeyEvent) {
        let Some((p, _)) = self.picker() else {
            self.filtering = false;
            return;
        };
        match key.code {
            KeyCode::Esc => p.filter.clear(),
            KeyCode::Backspace => {
                p.filter.pop();
            }
            KeyCode::Char(c) => p.filter.push(c),
            _ => {}
        }
        p.state.select(Some(0));
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            self.filtering = false;
        }
        if let Some((p, len)) = self.picker() {
            p.clamp(len);
        }
    }

    fn on_thread_key(&mut self, code: KeyCode, ctrl: bool) {
        let Some(t) = &mut self.thread else { return };
        let half = (t.viewport / 2).max(1) as isize;
        match code {
            KeyCode::Char('j') | KeyCode::Down => t.select(t.selected + 1),
            KeyCode::Char('k') | KeyCode::Up => t.select(t.selected.saturating_sub(1)),
            KeyCode::Char('J') => t.scroll_lines(1),
            KeyCode::Char('K') => t.scroll_lines(-1),
            KeyCode::Char('d') if ctrl => t.scroll_lines(half),
            KeyCode::Char('u') if ctrl => t.scroll_lines(-half),
            KeyCode::PageDown | KeyCode::Char(' ') => t.scroll_lines(half * 2 - 1),
            KeyCode::PageUp => t.scroll_lines(-(half * 2 - 1)),
            KeyCode::Char('g') | KeyCode::Home => t.select(0),
            KeyCode::Char('G') | KeyCode::End => t.select(usize::MAX),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                let quotes = t.posts[t.selected].quotes.clone();
                if !quotes.into_iter().any(|q| t.jump_to(q)) {
                    self.status = Some(("Post quotes nothing in this thread".into(), false));
                }
            }
            KeyCode::Char('b') => {
                let replies = t.backlinks[t.selected].clone();
                match replies.first() {
                    Some(&no) => {
                        t.jump_to(no);
                    }
                    None => self.status = Some(("No replies to this post".into(), false)),
                }
            }
            KeyCode::Char('u') => {
                if let Some(i) = t.jumps.pop() {
                    t.select(i);
                }
            }
            KeyCode::Char('i') => match t.posts[t.selected].files.first().cloned() {
                Some(f) => self.open_file(&f),
                None => self.status = Some(("Post has no file".into(), false)),
            },
            _ => {}
        }
    }

    fn on_viewer_key(&mut self, code: KeyCode) {
        let Some(v) = &mut self.viewer else { return };
        let n = v.files.len();
        match code {
            KeyCode::Esc | KeyCode::Char('q' | 'v') => self.viewer = None,
            KeyCode::Char('h' | 'k') | KeyCode::Left | KeyCode::Up => v.index = (v.index + n - 1) % n,
            KeyCode::Char('l' | 'j' | ' ') | KeyCode::Right | KeyCode::Down => v.index = (v.index + 1) % n,
            KeyCode::Char('i') | KeyCode::Enter => {
                let f = v.files[v.index].clone();
                self.open_file(&f);
            }
            _ => {}
        }
    }

    /// The post whose files `v`, `i`, `d` act on: the selected catalog entry or thread post.
    fn selected_post(&self) -> Option<&Post> {
        match self.view {
            View::Catalog => self.selected_index().map(|i| &self.catalog[i]),
            View::Thread => self.thread.as_ref().map(|t| &t.posts[t.selected]),
            _ => None,
        }
    }

    fn open_viewer(&mut self) {
        if !self.images.enabled() {
            self.status = Some(("Images are off (images = \"off\" in the config); i opens the file".into(), false));
            return;
        }
        match self.selected_post().map(|p| p.files.clone()) {
            Some(files) if !files.is_empty() => self.viewer = Some(Viewer { files, index: 0 }),
            _ => self.status = Some(("Post has no file".into(), false)),
        }
    }

    /// Open a file externally: videos in mpv when it's installed, everything else in the default opener.
    fn open_file(&mut self, f: &Attachment) {
        if f.is_video() && on_path("mpv") {
            let mut cmd = std::process::Command::new("mpv");
            cmd.arg(&f.url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
            self.status = Some(match cmd.spawn() {
                Ok(_) => (format!("Playing {} in mpv", f.filename), false),
                Err(e) => (format!("Couldn't start mpv: {e}"), true),
            });
        } else {
            self.open_url(&f.url);
        }
    }

    fn selected_index(&self) -> Option<usize> {
        match self.view {
            View::Sites => self.site_list.state.selected().and_then(|i| self.visible_sites().get(i).copied()),
            View::Boards => self.board_list.state.selected().and_then(|i| self.visible_boards().get(i).copied()),
            View::Catalog => self.catalog_list.state.selected().and_then(|i| self.visible_catalog().get(i).copied()),
            View::Thread => None,
        }
    }

    fn enter(&mut self) {
        let Some(i) = self.selected_index() else { return };
        match self.view {
            View::Sites => {
                if i != self.site {
                    self.board_list = Picker::default();
                }
                self.site = i;
                self.view = View::Boards;
                if self.board_list.state.selected().is_none() {
                    self.board_list.state.select(Some(0));
                }
                if self.current_site().boards.is_none() {
                    self.load_boards();
                }
            }
            View::Boards => {
                let board = self.boards()[i].clone();
                self.catalog.clear();
                self.catalog_list = Picker::default();
                self.catalog_list.state.select(Some(0));
                self.board = Some(board);
                self.view = View::Catalog;
                self.load_catalog();
            }
            View::Catalog => {
                let no = self.catalog[i].no;
                self.thread = None;
                self.view = View::Thread;
                self.load_thread(no);
            }
            View::Thread => {}
        }
    }

    fn back(&mut self) {
        self.view = match self.view {
            View::Sites => View::Sites,
            View::Boards => View::Sites,
            View::Catalog => View::Boards,
            View::Thread => View::Catalog,
        };
        // Navigating away cancels any in-flight request (its response will be ignored).
        if self.loading.is_some() {
            self.req += 1;
            self.loading = None;
        }
    }

    fn refresh(&mut self) {
        match self.view {
            View::Sites => {}
            View::Boards => self.load_boards(),
            View::Catalog => self.load_catalog(),
            View::Thread => {
                if let Some(no) = self.thread.as_ref().map(|t| t.no) {
                    self.load_thread(no);
                }
            }
        }
    }

    fn open_in_browser(&mut self) {
        let backend = self.current_site().backend.clone();
        let url = match (self.view, &self.board) {
            (View::Boards, _) => self.selected_index().map(|i| backend.board_url(&self.boards()[i].uri)),
            (View::Catalog, Some(b)) => self.selected_index().map(|i| backend.thread_url(&b.uri, self.catalog[i].no)),
            (View::Thread, Some(b)) => self.thread.as_ref().map(|t| {
                let mut url = backend.thread_url(&b.uri, t.no);
                url.push_str(&format!("#{}", t.posts[t.selected].no));
                url
            }),
            _ => None,
        };
        if let Some(url) = url {
            self.open_url(&url);
        }
    }

    fn open_url(&mut self, url: &str) {
        self.status = Some(match open::that_detached(url) {
            Ok(()) => (format!("Opened {url}"), false),
            Err(e) => (format!("Couldn't open {url}: {e}"), true),
        });
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn filtered(filter: &str, items: impl Iterator<Item = String>) -> Vec<usize> {
    let needle = filter.to_lowercase();
    items
        .enumerate()
        .filter(|(_, s)| needle.is_empty() || s.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_programs_on_path() {
        assert!(super::on_path("sh"));
        assert!(!super::on_path("ck-no-such-program"));
    }
}
