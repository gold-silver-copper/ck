use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::ListState;

use crate::backend::{self, Backend};
use crate::config::{self, Config, SiteConfig};
use crate::disk_cache::DiskCache;
use crate::download;
use crate::http;
use crate::images::Images;
use crate::keys::{Action, KeyMap, Scope};
use crate::model::{Attachment, Board, Link, Post};
use crate::store::{Store, ThreadKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Sites,
    Boards,
    Catalog,
    Thread,
    Watched,
    History,
}

/// Saved board lists older than this (seconds) are refreshed in the background.
const BOARDS_MAX_AGE: i64 = 24 * 3600;

/// Watched-thread refreshes running at once.
const MAX_REFRESHING: usize = 2;

/// Catalog sort orders, cycled with `s`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// The site's order (by last bump).
    #[default]
    Bump,
    Replies,
    Newest,
    Oldest,
}

impl Sort {
    pub fn next(self) -> Self {
        match self {
            Sort::Bump => Sort::Replies,
            Sort::Replies => Sort::Newest,
            Sort::Newest => Sort::Oldest,
            Sort::Oldest => Sort::Bump,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sort::Bump => "bump order",
            Sort::Replies => "most replies",
            Sort::Newest => "newest",
            Sort::Oldest => "oldest",
        }
    }
}

/// A row of the Sites view: the Watched and History lists come first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteRow {
    Watched,
    History,
    Site(usize),
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
    /// Posts numbered above this arrived since the previous visit (0: first visit, none are new).
    pub new_after: u64,
    /// After new posts arrive, keep this post (index, line offset into it) at the top of the view.
    pub anchor: Option<(usize, usize)>,
    /// Search query (as typed) and the posts matching it.
    pub search: String,
    pub matches: Vec<usize>,
    /// Posts whose spoilers are shown, or all of them.
    pub revealed: HashSet<usize>,
    pub reveal_all: bool,
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
    pub fn new(board: String, no: u64, posts: Vec<Post>) -> Self {
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
        Self {
            board,
            no,
            posts,
            index,
            backlinks,
            selected: 0,
            scroll: 0,
            jumps: Vec::new(),
            layout: None,
            viewport: 0,
            new_after: 0,
            anchor: None,
            search: String::new(),
            matches: Vec::new(),
            revealed: HashSet::new(),
            reveal_all: false,
        }
    }

    pub fn is_revealed(&self, i: usize) -> bool {
        self.reveal_all || self.revealed.contains(&i)
    }

    /// Recompute matches for the current query and re-render.
    pub fn set_search(&mut self, query: String) {
        let needle = query.to_lowercase();
        self.matches = if needle.is_empty() {
            Vec::new()
        } else {
            (0..self.posts.len()).filter(|&i| self.post_text(i).contains(&needle)).collect()
        };
        self.search = query;
        self.layout = None;
    }

    /// Lowercase searchable text of a post: name, subject, files and body (hidden spoilers excluded).
    fn post_text(&self, i: usize) -> String {
        let p = &self.posts[i];
        let mut s = format!("{} {} ", p.name, p.subject.as_deref().unwrap_or(""));
        for f in &p.files {
            s.push_str(&f.filename);
            s.push(' ');
        }
        if self.is_revealed(i) {
            for line in &p.body {
                s.extend(line.spans.iter().map(|s| s.content.as_ref()));
                s.push(' ');
            }
        } else {
            s.push_str(p.plain_text());
        }
        s.to_lowercase()
    }

    /// The next (or previous) matching post after the selection, wrapping around.
    fn next_match(&self, forward: bool) -> Option<usize> {
        if forward {
            self.matches.iter().find(|&&i| i > self.selected).or(self.matches.first()).copied()
        } else {
            self.matches.iter().rev().find(|&&i| i < self.selected).or(self.matches.last()).copied()
        }
    }

    pub fn is_new(&self, i: usize) -> bool {
        self.new_after > 0 && self.posts[i].no > self.new_after
    }

    /// The post at the top of the view and how many of its lines are scrolled past.
    fn top_anchor(&self) -> Option<(usize, usize)> {
        let l = self.layout.as_ref()?;
        let top = l.starts.partition_point(|&s| s <= self.scroll).saturating_sub(1);
        Some((top, self.scroll - l.starts[top]))
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

/// Wall clock for timestamps and "3h ago". Tests fix it (and format in UTC) so snapshots
/// don't depend on when or where they run.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clock {
    pub fixed: Option<i64>,
}

impl Clock {
    pub fn now(&self) -> i64 {
        self.fixed.unwrap_or_else(|| chrono::Utc::now().timestamp())
    }
}

/// Progress of the downloads started with `d`/`D`.
#[derive(Default)]
pub struct Downloads {
    pub total: usize,
    pub done: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Download jobs still running.
    pub running: usize,
    pub dir: Option<std::path::PathBuf>,
    pub last_error: Option<String>,
}

enum DlEvent {
    Done,
    Skipped,
    Failed(String),
    Finished,
}

/// Where the list or thread was last drawn, for mouse clicks.
#[derive(Debug, Clone, Copy)]
pub enum Hit {
    /// A list: its area, first visible item, and rows per item.
    List { area: Rect, offset: usize, item_height: u16 },
    Thread { area: Rect },
}

/// Popup with the posts the selected post quotes.
pub struct Preview {
    /// Indices of quoted posts in this thread.
    pub posts: Vec<usize>,
    /// Quoted post numbers that aren't in this thread.
    pub elsewhere: Vec<u64>,
    pub scroll: u16,
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
    /// The pages of a board list loaded so far; more are coming.
    BoardsPartial(u64, usize, Vec<Board>),
    /// A saved board list refreshed in the background.
    BoardsRefreshed(usize, Result<Vec<Board>>),
    Catalog(u64, Result<Vec<Post>>),
    CatalogPartial(u64, Vec<Post>),
    Thread(u64, Result<Vec<Post>>),
    /// A background refresh of a watched or open thread.
    Refreshed(ThreadKey, Result<Vec<Post>>),
    /// The thread a quoted post is in: (board, post, thread).
    Found(u64, Board, u64, Result<Option<u64>>),
    Download(DlEvent),
    Input(Event),
    /// Something else (a loaded image) needs a redraw.
    Wake,
}

pub struct App {
    pub sites: Vec<Site>,
    pub view: View,
    pub site_list: Picker,
    pub board_list: Picker,
    pub catalog_list: Picker,
    pub catalog_sort: Sort,
    pub compact: bool,
    pub watched_list: Picker,
    pub history_list: Picker,
    pub store: Store,
    /// Where `back` goes from a thread opened from Watched or History.
    return_to: Option<View>,
    refresh_thread: Duration,
    refresh_watched: Duration,
    thread_checked: Instant,
    watched_checked: HashMap<ThreadKey, Instant>,
    /// Background refreshes in flight.
    pub refreshing: HashSet<ThreadKey>,
    /// Sites whose saved board list is being refreshed in the background.
    boards_refreshing: HashSet<usize>,
    pub site: usize,
    pub board: Option<Board>,
    pub catalog: Vec<Post>,
    pub thread: Option<ThreadView>,
    /// True while typing into the filter.
    pub filtering: bool,
    /// Label of the in-flight request, if any.
    pub loading: Option<String>,
    pub status: Option<(String, bool)>,
    /// The status message as last seen by `expire_status`, and when it appeared.
    status_since: Option<(String, Instant)>,
    pub show_help: bool,
    pub help_scroll: u16,
    pub images: Images,
    pub viewer: Option<Viewer>,
    pub preview: Option<Preview>,
    /// True while typing a thread search.
    pub searching: bool,
    /// Threads left by following cross-thread links: (board, thread, selected post), for `u`.
    trail: Vec<(Board, u64, u64)>,
    /// Post to select once the loading thread arrives.
    pending_post: Option<u64>,
    /// Board the loaded catalog belongs to.
    catalog_board: String,
    /// After a thread 404'd: the same thread on the site's configured archive.
    archive_offer: Option<ThreadKey>,
    pub keys: KeyMap,
    pub clock: Clock,
    pub downloads: Downloads,
    download_dir: Option<String>,
    /// Set by the UI every frame.
    pub hit: Option<Hit>,
    /// Last left click: when, and the list index or thread post it hit.
    last_click: Option<(Instant, usize)>,
    pub tick: usize,
    pub quit: bool,
    req: u64,
    /// The thread number of the last thread load, for 404 handling.
    pending_thread: u64,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl App {
    pub fn new(cfg: Config, keys: KeyMap, picker: Option<ratatui_image::picker::Picker>, store: Store) -> Self {
        let refresh_thread = Duration::from_secs(cfg.refresh_thread_secs.max(10));
        let refresh_watched = Duration::from_secs(cfg.refresh_watched_secs.max(60));
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
            catalog_sort: Sort::default(),
            compact: store.settings.compact_catalog.unwrap_or(cfg.compact_catalog),
            watched_list: Picker::default(),
            history_list: Picker::default(),
            store,
            return_to: None,
            refresh_thread,
            refresh_watched,
            thread_checked: Instant::now(),
            watched_checked: HashMap::new(),
            refreshing: HashSet::new(),
            boards_refreshing: HashSet::new(),
            site: 0,
            board: None,
            catalog: Vec::new(),
            thread: None,
            filtering: false,
            loading: None,
            status: None,
            status_since: None,
            show_help: false,
            help_scroll: 0,
            images: Images::new(picker, {
                let tx = tx.clone();
                Arc::new(move || {
                    let _ = tx.send(Msg::Wake);
                })
            }, DiskCache::default_dir().map(|d| DiskCache::new(d, crate::disk_cache::BUDGET))),
            viewer: None,
            preview: None,
            searching: false,
            trail: Vec::new(),
            pending_post: None,
            catalog_board: String::new(),
            archive_offer: None,
            keys,
            clock: Clock::default(),
            downloads: Downloads::default(),
            download_dir: cfg.download_dir.clone(),
            hit: None,
            last_click: None,
            tick: 0,
            quit: false,
            req: 0,
            pending_thread: 0,
            tx,
            rx,
        };
        app.site_list.state.select(Some(0));
        app.watched_list.state.select(Some(0));
        app.history_list.state.select(Some(0));
        app
    }

    // ----- visible (filtered) items -----

    pub fn visible_sites(&self) -> Vec<SiteRow> {
        let rows: Vec<SiteRow> =
            [SiteRow::Watched, SiteRow::History].into_iter().chain((0..self.sites.len()).map(SiteRow::Site)).collect();
        let names = rows.iter().map(|r| match r {
            SiteRow::Watched => "Watched".to_string(),
            SiteRow::History => "History".to_string(),
            SiteRow::Site(i) => self.sites[*i].cfg.name.clone(),
        });
        filtered(&self.site_list.filter, names).into_iter().map(|i| rows[i]).collect()
    }

    pub fn visible_watched(&self) -> Vec<usize> {
        let items = self.store.watched.iter().map(|w| format!("{} {} {} {}", w.key.site, w.key.board, w.key.no, w.subject));
        filtered(&self.watched_list.filter, items)
    }

    pub fn visible_history(&self) -> Vec<usize> {
        let items = self.store.history.iter().map(|v| format!("{} {} {} {}", v.key.site, v.key.board, v.key.no, v.subject));
        filtered(&self.history_list.filter, items)
    }

    pub fn boards(&self) -> &[Board] {
        self.sites[self.site].boards.as_deref().unwrap_or(&[])
    }

    pub fn visible_boards(&self) -> Vec<usize> {
        filtered(&self.board_list.filter, self.boards().iter().map(|b| format!("{} {}", b.uri, b.title)))
    }

    pub fn visible_catalog(&self) -> Vec<usize> {
        let needle = self.catalog_list.filter.to_lowercase();
        let mut v: Vec<usize> = (0..self.catalog.len()).filter(|&i| self.catalog[i].search_text().contains(&needle)).collect();
        let c = &self.catalog;
        match self.catalog_sort {
            Sort::Bump => {}
            Sort::Replies => v.sort_by_key(|&i| std::cmp::Reverse(c[i].replies.unwrap_or(0))),
            Sort::Newest => v.sort_by_key(|&i| std::cmp::Reverse((c[i].time, c[i].no))),
            Sort::Oldest => v.sort_by_key(|&i| (c[i].time, c[i].no)),
        }
        v
    }

    fn picker(&mut self) -> Option<(&mut Picker, usize)> {
        let len = match self.view {
            View::Sites => self.visible_sites().len(),
            View::Boards => self.visible_boards().len(),
            View::Catalog => self.visible_catalog().len(),
            View::Watched => self.visible_watched().len(),
            View::History => self.visible_history().len(),
            View::Thread => return None,
        };
        let p = match self.view {
            View::Sites => &mut self.site_list,
            View::Boards => &mut self.board_list,
            View::Catalog => &mut self.catalog_list,
            View::Watched => &mut self.watched_list,
            View::History => &mut self.history_list,
            View::Thread => unreachable!(),
        };
        Some((p, len))
    }

    pub fn current_site(&self) -> &Site {
        &self.sites[self.site]
    }

    // ----- background loading -----

    /// Block until something happens (input, a finished request, a loaded image) or
    /// `timeout` passes, then handle everything pending. Returns whether anything arrived.
    pub fn wait(&mut self, timeout: Duration) -> bool {
        let got = match self.rx.recv_timeout(timeout) {
            Ok(msg) => {
                self.handle(msg);
                true
            }
            Err(_) => false,
        };
        self.poll();
        got
    }

    /// Handle everything pending without blocking. Queued input is all handled before the
    /// next draw, so held-down keys don't build up a lag.
    pub fn poll(&mut self) {
        self.images.poll();
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg);
        }
        self.background();
        self.expire_status(Instant::now());
    }

    /// How long the main loop may sleep: until the next animation frame, status expiry or
    /// due refresh, and at most a second (relative times like "5s ago" stay current).
    pub fn next_wake(&self, now: Instant) -> Duration {
        let mut wake = Duration::from_secs(1);
        let animating = self.loading.is_some() || !self.refreshing.is_empty() || self.downloads.running > 0 || self.viewer.is_some();
        if animating {
            wake = wake.min(Duration::from_millis(100));
        }
        let mut at = |t: Instant| wake = wake.min(t.saturating_duration_since(now));
        if let (Some((_, is_err)), Some((_, since))) = (&self.status, &self.status_since) {
            at(*since + Duration::from_secs(if *is_err { 5 } else { 2 }));
        }
        if self.view == View::Thread && self.thread.is_some() && self.loading.is_none() {
            at(self.thread_checked + self.refresh_thread);
        }
        // At capacity, a finished refresh wakes the loop anyway (and due ones mustn't spin it).
        if self.refreshing.len() < MAX_REFRESHING {
            for w in self.store.watched.iter().filter(|w| !w.dead && !self.refreshing.contains(&w.key)) {
                match self.watched_checked.get(&w.key) {
                    Some(t) => at(*t + self.refresh_watched),
                    None => at(now),
                }
            }
        }
        wake
    }

    /// Read terminal input on a thread, into the same channel as everything else. Start it
    /// only after image protocol detection, which reads stdin itself.
    pub fn listen_for_input(&self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = event::read() {
                if tx.send(Msg::Input(ev)).is_err() {
                    return;
                }
            }
        });
    }

    fn handle(&mut self, msg: Msg) {
        {
            match msg {
                Msg::Wake => {}
                Msg::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => self.on_key(key),
                Msg::Input(Event::Mouse(m)) => self.on_mouse(m, Instant::now()),
                Msg::Input(_) => {}
                Msg::Refreshed(key, res) => self.refreshed(key, res),
                Msg::Download(ev) => self.download_event(ev),
                Msg::Found(id, board, post, res) if id == self.req => {
                    self.loading = None;
                    match res {
                        Ok(Some(no)) => {
                            if let Some(t) = &self.thread {
                                self.trail.push((self.board.clone().unwrap_or(board.clone()), t.no, t.posts[t.selected].no));
                            }
                            self.open_thread_at(board, no, Some(post), true);
                        }
                        Ok(None) => {
                            let msg = format!("Post {post} isn't in this thread, and this site can't say which thread it's in");
                            self.status = Some((msg, true));
                        }
                        Err(e) => self.error(e),
                    }
                }
                Msg::Cached(id, age) if id == self.req && self.status.is_none() => {
                    self.status = Some((format!("Up to date (checked {}s ago)", age.as_secs()), false));
                }
                Msg::Boards(id, site, res) => {
                    if id == self.req {
                        self.loading = None;
                    }
                    match res {
                        Ok(b) => self.set_boards(site, b, true),
                        Err(e) => self.error(e),
                    }
                }
                Msg::BoardsPartial(id, site, b) if id == self.req => self.set_boards(site, b, false),
                Msg::BoardsRefreshed(site, res) => {
                    self.boards_refreshing.remove(&site);
                    // A failed background refresh keeps the saved list; there's nothing to say.
                    if let Ok(b) = res {
                        self.set_boards(site, b, true);
                    }
                }
                Msg::CatalogPartial(id, posts) if id == self.req => {
                    self.catalog = posts;
                    let len = self.visible_catalog().len();
                    self.catalog_list.clamp(len);
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
                    self.thread_checked = Instant::now();
                    match res {
                        Ok(posts) => self.set_thread(posts),
                        Err(e) if http::is_not_found(&e) => {
                            let Some(key) = self.board.as_ref().map(|b| self.key(&b.uri, self.pending_thread)) else {
                                return;
                            };
                            self.thread_gone(&key);
                            if let Some(w) = self.store.watched_mut(&key) {
                                w.dead = true;
                                self.save();
                            }
                        }
                        Err(e) => self.error(e),
                    }
                }
                _ => {} // stale response
            }
        }
    }

    /// Status messages replace the footer's key hints only briefly: info for 2 seconds,
    /// errors for 5. A new or changed message restarts the timer.
    fn expire_status(&mut self, now: Instant) {
        let Some((msg, is_err)) = &self.status else {
            self.status_since = None;
            return;
        };
        match &self.status_since {
            Some((seen, since)) if seen == msg => {
                let ttl = Duration::from_secs(if *is_err { 5 } else { 2 });
                if now.duration_since(*since) >= ttl {
                    self.status = None;
                    self.status_since = None;
                }
            }
            _ => self.status_since = Some((msg.clone(), now)),
        }
    }

    fn error(&mut self, e: anyhow::Error) {
        self.status = Some((format!("{e:#}"), true));
    }

    /// Run `job` on a thread, as the current request (shown as `label`). The job gets the
    /// request id and a sender for partial results.
    fn spawn<T: Send + 'static>(
        &mut self,
        label: String,
        job: impl FnOnce(&dyn Backend, u64, &Sender<Msg>) -> Result<T> + Send + 'static,
        wrap: impl FnOnce(u64, Result<T>) -> Msg + Send + 'static,
    ) {
        self.req += 1;
        let id = self.req;
        let backend = self.current_site().backend.clone();
        let tx = self.tx.clone();
        self.loading = Some(label);
        self.status = None;
        std::thread::spawn(move || {
            let res = job(&*backend, id, &tx);
            let cached = http::take_cached_age();
            let _ = tx.send(wrap(id, res));
            if let Some(age) = cached {
                let _ = tx.send(Msg::Cached(id, age));
            }
        });
    }

    /// Show a site's boards; a complete list is also saved for next time.
    fn set_boards(&mut self, site: usize, boards: Vec<Board>, complete: bool) {
        if complete && self.sites[site].cfg.boards.is_none() {
            let name = self.sites[site].cfg.name.clone();
            if let Err(e) = self.store.save_boards(&name, &boards, self.clock.now()) {
                self.status = Some((format!("Couldn't save the board list: {e:#}"), true));
            }
        }
        self.sites[site].boards = Some(boards);
        if site == self.site {
            let len = self.visible_boards().len();
            self.board_list.clamp(len);
        }
    }

    /// Refresh a saved board list at background priority, without a spinner.
    fn refresh_boards_in_background(&mut self, site: usize) {
        if !self.boards_refreshing.insert(site) {
            return;
        }
        let backend = self.sites[site].backend.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = http::background(|| backend.boards(&|_| {}));
            let _ = tx.send(Msg::BoardsRefreshed(site, res));
        });
    }

    fn load_boards(&mut self) {
        let site = self.site;
        let label = format!("Loading boards for {}", self.current_site().cfg.name);
        self.spawn(
            label,
            move |b, id, tx| {
                b.boards(&|so_far| {
                    let _ = tx.send(Msg::BoardsPartial(id, site, so_far.to_vec()));
                })
            },
            move |id, r| Msg::Boards(id, site, r),
        );
    }

    fn load_catalog(&mut self) {
        let Some(board) = self.board.clone() else { return };
        self.catalog_board = board.uri.clone();
        let job = move |b: &dyn Backend, id, tx: &Sender<Msg>| {
            b.catalog(&board.uri, &|so_far| {
                let _ = tx.send(Msg::CatalogPartial(id, so_far.to_vec()));
            })
        };
        self.spawn(format!("Loading /{}/", self.catalog_board), job, Msg::Catalog);
    }

    fn load_thread(&mut self, no: u64) {
        let Some(board) = self.board.clone() else { return };
        self.pending_thread = no;
        self.archive_offer = None;
        self.thread_checked = Instant::now();
        self.spawn(format!("Loading thread {no}"), move |b, _, _| b.thread(&board.uri, no), Msg::Thread);
    }

    fn key(&self, board: &str, no: u64) -> ThreadKey {
        ThreadKey { site: self.current_site().cfg.name.clone(), board: board.to_string(), no }
    }

    fn save(&mut self) {
        if let Err(e) = self.store.save() {
            self.status = Some((format!("Couldn't save watched threads: {e:#}"), true));
        }
    }

    fn set_thread(&mut self, posts: Vec<Post>) {
        let Some(board) = self.board.as_ref().map(|b| b.uri.clone()) else { return };
        let no = posts.first().map(|p| p.no).unwrap_or(0);
        let key = self.key(&board, no);
        let mut tv = ThreadView::new(board, no, posts);
        match self.thread.take().filter(|t| t.no == no && t.board == tv.board) {
            // On refresh, keep the selected post and what's at the top of the view.
            Some(old) => {
                tv.selected = tv.index.get(&old.posts[old.selected].no).copied().unwrap_or(0);
                tv.anchor = old.top_anchor().and_then(|(i, off)| Some((*tv.index.get(&old.posts[i].no)?, off)));
                tv.scroll = old.scroll;
                tv.viewport = old.viewport;
                tv.jumps = old.jumps;
                tv.new_after = old.new_after;
                // Revealed spoilers by post number, since indices can shift.
                tv.revealed = old.revealed.iter().filter_map(|&i| tv.index.get(&old.posts[i].no).copied()).collect();
                tv.reveal_all = old.reveal_all;
                tv.set_search(old.search);
            }
            None => {
                tv.new_after = self.store.last_seen(&key);
                if let Some(i) = self.pending_post.take().and_then(|no| tv.index.get(&no).copied()) {
                    tv.selected = i;
                }
            }
        }
        let max_no = tv.posts.iter().map(|p| p.no).max().unwrap_or(0);
        let subject = thread_subject(&tv.posts);
        self.store.visit(&key, &subject, tv.posts.len(), max_no, self.clock.now());
        self.save();
        self.thread = Some(tv);
    }

    // ----- watched threads and auto-refresh -----

    /// Start background refreshes that are due: the open thread every `refresh_thread`, and
    /// each watched thread every `refresh_watched`, a couple at a time.
    fn background(&mut self) {
        let open = self.thread.as_ref().filter(|_| self.view == View::Thread).map(|t| self.key(&t.board, t.no));
        if let Some(key) = &open
            && self.loading.is_none()
            && self.thread_checked.elapsed() >= self.refresh_thread
            && !self.refreshing.contains(key)
        {
            self.thread_checked = Instant::now();
            self.refresh_in_background(key.clone());
        }
        if self.refreshing.len() >= MAX_REFRESHING {
            return;
        }
        let due = self.store.watched.iter().find(|w| {
            !w.dead
                && Some(&w.key) != open.as_ref()
                && !self.refreshing.contains(&w.key)
                && self.watched_checked.get(&w.key).is_none_or(|t| t.elapsed() >= self.refresh_watched)
        });
        if let Some(key) = due.map(|w| w.key.clone()) {
            self.watched_checked.insert(key.clone(), Instant::now());
            self.refresh_in_background(key);
        }
    }

    fn refresh_in_background(&mut self, key: ThreadKey) {
        let Some(site) = self.sites.iter().find(|s| s.cfg.name == key.site) else { return };
        let backend = site.backend.clone();
        let tx = self.tx.clone();
        self.refreshing.insert(key.clone());
        std::thread::spawn(move || {
            let res = http::background(|| backend.thread(&key.board, key.no));
            let _ = tx.send(Msg::Refreshed(key, res));
        });
    }

    fn refreshed(&mut self, key: ThreadKey, res: Result<Vec<Post>>) {
        self.refreshing.remove(&key);
        let is_open = self.view == View::Thread
            && self.thread.as_ref().is_some_and(|t| self.key(&t.board, t.no) == key)
            && self.current_site().cfg.name == key.site;
        if is_open {
            // Count the interval from the response, so the next refresh is past the HTTP cache window.
            self.thread_checked = Instant::now();
        }
        match res {
            Ok(posts) if is_open => self.set_thread(posts),
            Ok(posts) => {
                let subject = thread_subject(&posts);
                let Some(w) = self.store.watched_mut(&key) else { return };
                let max_no = posts.iter().map(|p| p.no).max().unwrap_or(0);
                if w.last_seen == 0 {
                    w.last_seen = max_no;
                }
                w.unread = posts.iter().filter(|p| p.no > w.last_seen).count();
                w.posts = posts.len();
                w.dead = false;
                if w.subject.is_empty() {
                    w.subject = subject;
                }
                self.save();
            }
            Err(e) if http::is_not_found(&e) => {
                if let Some(w) = self.store.watched_mut(&key) {
                    w.dead = true;
                    self.save();
                }
                if is_open {
                    self.thread_gone(&key);
                }
            }
            // Other failures (network, rate limits) just wait for the next round.
            Err(e) if is_open => self.error(e),
            Err(_) => {}
        }
    }

    // ----- downloads and settings -----

    /// Save the selected post's files, or the whole thread's.
    fn download(&mut self, whole_thread: bool) {
        let Some(t) = &self.thread else { return };
        let posts: Vec<&Post> = if whole_thread { t.posts.iter().collect() } else { vec![&t.posts[t.selected]] };
        let dir = download::dir(self.download_dir.as_deref(), &self.current_site().cfg.name, &t.board, t.no);
        let jobs = download::jobs(&posts, &dir);
        if jobs.is_empty() {
            let msg = if whole_thread { "Thread has no files" } else { "Post has no file" };
            self.status = Some((msg.into(), false));
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.status = Some((format!("Couldn't create {}: {e}", dir.display()), true));
            return;
        }
        let d = &mut self.downloads;
        if d.running == 0 {
            *d = Downloads::default();
        }
        d.total += jobs.len();
        d.running += 1;
        d.dir = Some(dir);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            for (url, path) in jobs {
                let ev = if path.exists() {
                    DlEvent::Skipped
                } else {
                    match http::download_to(&url, &path) {
                        Ok(()) => DlEvent::Done,
                        Err(e) => DlEvent::Failed(format!("{e:#}")),
                    }
                };
                if tx.send(Msg::Download(ev)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::Download(DlEvent::Finished));
        });
    }

    fn download_event(&mut self, ev: DlEvent) {
        let d = &mut self.downloads;
        match ev {
            DlEvent::Done => d.done += 1,
            DlEvent::Skipped => d.skipped += 1,
            DlEvent::Failed(e) => {
                d.failed += 1;
                d.last_error = Some(e);
            }
            DlEvent::Finished => {
                d.running -= 1;
                if d.running == 0 {
                    let dir = d.dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                    let mut msg = format!("Downloaded {} file{} to {dir}", d.done, if d.done == 1 { "" } else { "s" });
                    if d.skipped > 0 {
                        msg.push_str(&format!(", {} already there", d.skipped));
                    }
                    if d.failed > 0 {
                        msg.push_str(&format!(", {} failed ({})", d.failed, d.last_error.as_deref().unwrap_or("")));
                    }
                    self.status = Some((msg, d.failed > 0));
                }
            }
        }
    }

    /// Toggle the one-line catalog layout and remember it: in config.toml if there is one
    /// (keeping its comments), else for the session; in the data directory if the config
    /// can't be edited.
    fn toggle_compact(&mut self) {
        self.compact = !self.compact;
        let state = if self.compact { "on" } else { "off" };
        let saved = match config::save_compact(self.compact) {
            Ok(true) => {
                self.store.settings.compact_catalog = None;
                "saved in config.toml".to_string()
            }
            Ok(false) => "for this session; there's no config file to save it in".into(),
            Err(e) => {
                self.store.settings.compact_catalog = Some(self.compact);
                self.save();
                format!("saved in the data directory; couldn't edit config.toml: {e:#}")
            }
        };
        self.status = Some((format!("Compact catalog {state} ({saved})"), false));
    }

    // ----- mouse -----

    /// Wheel scrolls, a click selects, a double click opens.
    pub fn on_mouse(&mut self, ev: MouseEvent, now: Instant) {
        http::user_input();
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
            let key = |c| KeyEvent::from(if c { KeyCode::Down } else { KeyCode::Up });
            if self.show_help || self.viewer.is_some() || self.preview.is_some() {
                self.on_key(key(down));
            } else if self.view == View::Thread {
                if let Some(t) = &mut self.thread {
                    t.scroll_lines(if down { 3 } else { -3 });
                }
            } else if !self.filtering && !self.searching {
                self.on_key(key(down));
            }
            return;
        }
        if ev.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        if self.show_help || self.preview.is_some() {
            // Clicking anywhere closes a popup.
            self.show_help = false;
            self.preview = None;
            return;
        }
        if self.viewer.is_some() || self.filtering || self.searching {
            return;
        }
        let Some(target) = self.click_target(ev.column, ev.row) else { return };
        let double = self.last_click.is_some_and(|(t, i)| i == target && now.duration_since(t) < Duration::from_millis(400));
        self.last_click = if double { None } else { Some((now, target)) };
        if self.view == View::Thread {
            if let Some(t) = &mut self.thread {
                t.selected = target;
            }
            if double {
                self.on_key(KeyEvent::from(KeyCode::Enter));
            }
        } else if let Some((p, len)) = self.picker()
            && target < len
        {
            p.state.select(Some(target));
            if double {
                self.enter();
            }
        }
    }

    /// The list row or thread post at a screen position.
    fn click_target(&self, col: u16, row: u16) -> Option<usize> {
        let pos = ratatui::layout::Position::new(col, row);
        match self.hit? {
            Hit::List { area, offset, item_height } if area.contains(pos) => {
                Some(offset + ((row - area.y) / item_height.max(1)) as usize)
            }
            Hit::Thread { area } if area.contains(pos) => {
                let t = self.thread.as_ref()?;
                let l = t.layout.as_ref()?;
                let line = t.scroll + (row - area.y) as usize;
                (line < l.lines.len()).then(|| l.starts.partition_point(|&s| s <= line).saturating_sub(1))
            }
            _ => None,
        }
    }

    /// A thread 404'd: say so, and offer the site's archive if it has one.
    fn thread_gone(&mut self, key: &ThreadKey) {
        let archive = self.sites.iter().find(|s| s.cfg.name == key.site).and_then(|s| s.cfg.archive.clone());
        match archive.filter(|a| self.sites.iter().any(|s| s.cfg.name == *a)) {
            Some(a) => {
                self.status = Some((format!("Thread was deleted or archived. Press a to open it in {a}"), true));
                self.archive_offer = Some(ThreadKey { site: a, board: key.board.clone(), no: key.no });
            }
            None => self.status = Some(("Thread was deleted or archived".into(), true)),
        }
    }

    fn toggle_watch(&mut self) {
        let (board, no, subject, posts, last_seen) = match (self.view, &self.board) {
            (View::Thread, _) => {
                let Some(t) = &self.thread else { return };
                let max_no = t.posts.iter().map(|p| p.no).max().unwrap_or(0);
                (t.board.clone(), t.no, thread_subject(&t.posts), t.posts.len(), max_no)
            }
            (View::Catalog, Some(b)) => {
                let Some(i) = self.selected_index() else { return };
                let op = &self.catalog[i];
                let posts = op.replies.map_or(1, |r| r as usize + 1);
                // Unknown until the first refresh, which then counts nothing as unread.
                (b.uri.clone(), op.no, thread_subject(std::slice::from_ref(op)), posts, 0)
            }
            _ => return,
        };
        let key = self.key(&board, no);
        let watching = self.store.toggle_watch(key.clone(), subject, posts, last_seen);
        self.watched_checked.remove(&key);
        self.status = Some((if watching { format!("Watching thread {no}") } else { format!("Stopped watching thread {no}") }, false));
        self.save();
    }

    /// Open a thread from Watched or History, switching site and board as needed.
    fn open_key(&mut self, key: ThreadKey) {
        let Some(site) = self.sites.iter().position(|s| s.cfg.name == key.site) else {
            self.status = Some((format!("No site named {} in the config", key.site), true));
            return;
        };
        if site != self.site {
            self.board_list = Picker::default();
            self.board_list.state.select(Some(0));
        }
        self.site = site;
        let board = self.boards().iter().find(|b| b.uri == key.board).cloned();
        self.board = Some(board.unwrap_or(Board { uri: key.board, title: String::new(), nsfw: None }));
        self.return_to = Some(self.view);
        self.thread = None;
        self.view = View::Thread;
        self.load_thread(key.no);
    }

    // ----- input -----

    pub fn on_key(&mut self, key: KeyEvent) {
        http::user_input();
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
        if self.preview.is_some() {
            self.on_preview_key(key.code);
            return;
        }
        if self.searching {
            self.on_search_key(key);
            return;
        }
        if self.filtering {
            self.on_filter_key(key);
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if let KeyCode::Char(c) = key.code
            && !ctrl
            && let Some(action) = self.keys.action(self.scope(), c)
        {
            self.act(action);
            return;
        }
        match key.code {
            KeyCode::F(5) => self.refresh(),
            KeyCode::Esc if self.view == View::Thread && self.thread.as_ref().is_some_and(|t| !t.search.is_empty()) => {
                if let Some(t) = &mut self.thread {
                    t.set_search(String::new());
                }
            }
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

    pub fn scope(&self) -> Scope {
        match self.view {
            View::Sites | View::Boards => Scope::Lists,
            View::Catalog => Scope::Catalog,
            View::Thread => Scope::Thread,
            View::Watched | View::History => Scope::Saved,
        }
    }

    /// Run a (remappable) command.
    fn act(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit = true,
            Action::Help => self.show_help = true,
            Action::Search if self.view == View::Thread => {
                if let Some(t) = &mut self.thread {
                    t.set_search(String::new());
                    self.searching = true;
                }
            }
            Action::Search => self.filtering = true,
            Action::Reload => self.refresh(),
            Action::Browser => self.open_in_browser(),
            Action::View => self.open_viewer(),
            Action::Watch => self.toggle_watch(),
            Action::Remove => self.remove_entry(),
            Action::Sort => {
                self.catalog_sort = self.catalog_sort.next();
                self.catalog_list.state.select(Some(0));
                self.status = Some((format!("Sorted by {}", self.catalog_sort.label()), false));
            }
            Action::Compact => self.toggle_compact(),
            Action::Download => self.download(false),
            Action::DownloadThread => self.download(true),
            Action::Archive => match self.archive_offer.take() {
                Some(key) => {
                    self.open_key(key);
                    self.return_to = None;
                }
                None => self.status = Some(("Nothing to open in an archive".into(), false)),
            },
            Action::OpenFile
            | Action::Replies
            | Action::JumpBack
            | Action::Unread
            | Action::Preview
            | Action::NextMatch
            | Action::PrevMatch
            | Action::Spoiler
            | Action::AllSpoilers => self.thread_action(action),
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
                    self.follow_link();
                }
            }
            _ => {}
        }
    }

    fn thread_action(&mut self, action: Action) {
        let search_key = self.keys.key(Action::Search);
        let Some(t) = &mut self.thread else { return };
        match action {
            Action::Preview => self.open_preview(),
            Action::NextMatch | Action::PrevMatch if t.matches.is_empty() => {
                let msg = if t.search.is_empty() { format!("No search; press {search_key} to search the thread") } else { "No matches".into() };
                self.status = Some((msg, false));
            }
            Action::NextMatch | Action::PrevMatch => {
                if let Some(i) = t.next_match(action == Action::NextMatch) {
                    t.select(i);
                    let k = t.matches.iter().position(|&m| m == i).unwrap_or(0);
                    self.status = Some((format!("Match {}/{} for \"{}\"", k + 1, t.matches.len(), t.search), false));
                }
            }
            Action::Spoiler => {
                let i = t.selected;
                if !t.revealed.remove(&i) {
                    t.revealed.insert(i);
                }
                t.layout = None;
            }
            Action::AllSpoilers => {
                t.reveal_all = !t.reveal_all;
                t.revealed.clear();
                t.layout = None;
                let msg = if t.reveal_all { "Showing all spoilers" } else { "Hiding spoilers" };
                self.status = Some((msg.into(), false));
            }
            Action::Replies => match t.backlinks[t.selected].first() {
                Some(&no) => {
                    t.jump_to(no);
                }
                None => self.status = Some(("No replies to this post".into(), false)),
            },
            Action::JumpBack => {
                if let Some(i) = t.jumps.pop() {
                    t.select(i);
                } else if let Some((board, no, post)) = self.trail.pop() {
                    // Back to the thread we came from by a cross-thread link.
                    self.open_thread_at(board, no, Some(post), false);
                }
            }
            Action::Unread => match (0..t.posts.len()).find(|&i| t.is_new(i)) {
                Some(i) => {
                    t.jumps.push(t.selected);
                    t.select(i);
                }
                None => self.status = Some(("No unread posts".into(), false)),
            },
            Action::OpenFile => match t.posts[t.selected].files.first().cloned() {
                Some(f) => self.open_file(&f),
                None => self.status = Some(("Post has no file".into(), false)),
            },
            _ => {}
        }
    }

    fn on_search_key(&mut self, key: KeyEvent) {
        let Some(t) = &mut self.thread else {
            self.searching = false;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                t.set_search(String::new());
                self.searching = false;
            }
            KeyCode::Enter => {
                self.searching = false;
                match t.next_match(true) {
                    Some(i) => {
                        t.select(i);
                        self.status = Some((format!("{} posts match \"{}\" (n/N to move)", t.matches.len(), t.search), false));
                    }
                    None if t.search.is_empty() => {}
                    None => self.status = Some((format!("No posts match \"{}\"", t.search), false)),
                }
            }
            KeyCode::Backspace => {
                let mut q = t.search.clone();
                q.pop();
                t.set_search(q);
            }
            KeyCode::Char(c) => {
                let q = format!("{}{c}", t.search);
                t.set_search(q);
            }
            _ => {}
        }
    }

    fn open_preview(&mut self) {
        let Some(t) = &self.thread else { return };
        let p = &t.posts[t.selected];
        let posts: Vec<usize> = p.quotes.iter().filter_map(|q| t.index.get(q).copied()).collect();
        let elsewhere: Vec<u64> = p.links.iter().filter_map(|l| l.post).filter(|n| !t.index.contains_key(n)).collect();
        if posts.is_empty() && elsewhere.is_empty() {
            self.status = Some(("Post quotes nothing".into(), false));
            return;
        }
        self.preview = Some(Preview { posts, elsewhere, scroll: 0 });
    }

    fn on_preview_key(&mut self, code: KeyCode) {
        let Some(p) = &mut self.preview else { return };
        match code {
            KeyCode::Char('j') | KeyCode::Down => p.scroll = p.scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => p.scroll = p.scroll.saturating_sub(1),
            KeyCode::Char(' ') | KeyCode::PageDown => p.scroll = p.scroll.saturating_add(10),
            KeyCode::PageUp => p.scroll = p.scroll.saturating_sub(10),
            KeyCode::Char('g') => p.scroll = 0,
            // Jump to the (first) quoted post.
            KeyCode::Enter => {
                let first = p.posts.first().copied();
                self.preview = None;
                if let (Some(i), Some(t)) = (first, &mut self.thread) {
                    t.jumps.push(t.selected);
                    t.select(i);
                }
            }
            _ => self.preview = None,
        }
    }

    /// Follow the selected post's first link that leads out of this thread, preferring links
    /// to posts over links to boards.
    fn follow_link(&mut self) {
        let (Some(t), Some(board)) = (&self.thread, self.board.clone()) else { return };
        let here = |l: &Link| l.board.as_ref().is_none_or(|b| *b == board.uri);
        let leaves = |l: &&Link| {
            let in_thread = here(l)
                && (l.thread == Some(t.no) || (l.thread.is_none() && l.post.is_some_and(|p| t.index.contains_key(&p))));
            !in_thread
        };
        let links = &t.posts[t.selected].links;
        let out = links.iter().filter(leaves).find(|l| l.post.is_some()).or_else(|| links.iter().find(leaves));
        let Some(link) = out.cloned() else {
            self.status = Some(("Post quotes nothing in this thread".into(), false));
            return;
        };
        let target = match &link.board {
            Some(uri) if *uri != board.uri => self.find_board(uri),
            _ => board.clone(),
        };
        match (link.thread, link.post) {
            (Some(no), post) => {
                let from = (board, t.no, t.posts[t.selected].no);
                self.trail.push(from);
                self.open_thread_at(target, no, post, true);
            }
            (None, None) => {
                // A board link: open its catalog.
                self.board = Some(target);
                self.catalog.clear();
                self.catalog_list = Picker::default();
                self.catalog_list.state.select(Some(0));
                self.return_to = None;
                self.view = View::Catalog;
                self.load_catalog();
            }
            (None, Some(post)) => {
                // Ask the engine which thread the post is in (only some can).
                let uri = target.uri.clone();
                let label = format!("Looking up post {post}");
                self.spawn(label, move |b, _, _| b.find_thread(&uri, post), move |id, r| Msg::Found(id, target, post, r));
            }
        }
    }

    /// A board by URI from the site's board list, or a bare one if the list isn't loaded.
    fn find_board(&self, uri: &str) -> Board {
        let known = self.boards().iter().find(|b| b.uri == uri).cloned();
        known.unwrap_or(Board { uri: uri.to_string(), title: String::new(), nsfw: None })
    }

    /// Open a thread on the current site, selecting `post` when it arrives.
    fn open_thread_at(&mut self, board: Board, no: u64, post: Option<u64>, announce: bool) {
        if announce {
            self.status = Some((format!("Opening /{}/{no} (u goes back)", board.uri), false));
        }
        self.board = Some(board);
        self.pending_post = post;
        self.thread = None;
        self.view = View::Thread;
        self.load_thread(no);
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

    fn remove_entry(&mut self) {
        let Some(i) = self.selected_index() else { return };
        match self.view {
            View::Watched => {
                let w = self.store.watched.remove(i);
                self.status = Some((format!("Stopped watching thread {}", w.key.no), false));
            }
            View::History => {
                self.store.history.remove(i);
            }
            _ => return,
        }
        self.save();
        if let Some((p, len)) = self.picker() {
            p.clamp(len);
        }
    }

    /// Index of the selected item in the current list's underlying data (not for Sites).
    fn selected_index(&self) -> Option<usize> {
        match self.view {
            View::Sites | View::Thread => None,
            View::Boards => self.board_list.state.selected().and_then(|i| self.visible_boards().get(i).copied()),
            View::Catalog => self.catalog_list.state.selected().and_then(|i| self.visible_catalog().get(i).copied()),
            View::Watched => self.watched_list.state.selected().and_then(|i| self.visible_watched().get(i).copied()),
            View::History => self.history_list.state.selected().and_then(|i| self.visible_history().get(i).copied()),
        }
    }

    fn enter(&mut self) {
        if self.view == View::Sites {
            match self.site_list.state.selected().and_then(|i| self.visible_sites().get(i).copied()) {
                Some(SiteRow::Watched) => self.view = View::Watched,
                Some(SiteRow::History) => self.view = View::History,
                Some(SiteRow::Site(i)) => self.enter_site(i),
                None => {}
            }
            return;
        }
        let Some(i) = self.selected_index() else { return };
        match self.view {
            View::Sites | View::Thread => {}
            View::Watched => self.open_key(self.store.watched[i].key.clone()),
            View::History => self.open_key(self.store.history[i].key.clone()),
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
                self.return_to = None;
                self.view = View::Thread;
                self.load_thread(no);
            }
        }
    }

    fn enter_site(&mut self, i: usize) {
        if i != self.site {
            self.board_list = Picker::default();
        }
        self.site = i;
        self.view = View::Boards;
        if self.board_list.state.selected().is_none() {
            self.board_list.state.select(Some(0));
        }
        if self.current_site().boards.is_some() {
            return;
        }
        // Board lists from the config need no request; fetched ones are saved, shown at
        // once next time, and refreshed in the background once a day.
        let cfg = &self.current_site().cfg;
        let saved = if cfg.boards.is_none() { self.store.load_boards(&cfg.name) } else { None };
        match saved {
            Some((boards, fetched)) => {
                self.sites[i].boards = Some(boards);
                if self.clock.now() - fetched > BOARDS_MAX_AGE {
                    self.refresh_boards_in_background(i);
                }
            }
            None => self.load_boards(),
        }
    }

    fn back(&mut self) {
        self.view = match self.view {
            View::Sites | View::Boards | View::Watched | View::History => View::Sites,
            View::Catalog => View::Boards,
            View::Thread => self.return_to.take().unwrap_or(View::Catalog),
        };
        self.trail.clear();
        // After following links to another board, the loaded catalog is for the old one.
        if self.view == View::Catalog && self.board.as_ref().is_some_and(|b| b.uri != self.catalog_board) {
            self.catalog.clear();
            self.catalog_list = Picker::default();
            self.catalog_list.state.select(Some(0));
            self.load_catalog();
            return;
        }
        // Navigating away cancels any in-flight request (its response will be ignored).
        if self.loading.is_some() {
            self.req += 1;
            self.loading = None;
        }
    }

    fn refresh(&mut self) {
        match self.view {
            View::Sites | View::Watched | View::History => {}
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
        let key_url = |key: &ThreadKey| {
            let site = self.sites.iter().find(|s| s.cfg.name == key.site)?;
            Some(site.backend.thread_url(&key.board, key.no))
        };
        let url = match (self.view, &self.board) {
            (View::Watched, _) => self.selected_index().and_then(|i| key_url(&self.store.watched[i].key)),
            (View::History, _) => self.selected_index().and_then(|i| key_url(&self.store.history[i].key)),
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

/// A thread's subject for lists: its subject, or the start of the OP's text.
fn thread_subject(posts: &[Post]) -> String {
    let Some(op) = posts.first() else { return String::new() };
    op.subject.clone().unwrap_or_else(|| op.plain_text().chars().take(80).collect())
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
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use ratatui::text::Line;

    use super::*;

    /// An app over the default config; nothing here touches the network.
    pub fn test_app() -> App {
        let cfg: Config = toml::from_str(crate::config::DEFAULT_CONFIG).unwrap();
        App::new(cfg, KeyMap::default(), None, Store::default())
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
    }

    #[test]
    fn finds_programs_on_path() {
        assert!(super::on_path("sh"));
        assert!(!super::on_path("ck-no-such-program"));
    }

    #[test]
    fn mouse_wheel_click_and_double_click() {
        let mut app = test_app();
        app.hit = Some(Hit::List { area: Rect::new(1, 2, 60, 10), offset: 0, item_height: 1 });
        let t0 = Instant::now();
        app.on_mouse(mouse(MouseEventKind::ScrollDown, 5, 5), t0);
        assert_eq!(app.site_list.state.selected(), Some(1));
        app.on_mouse(mouse(MouseEventKind::ScrollUp, 5, 5), t0);
        assert_eq!(app.site_list.state.selected(), Some(0));

        // Row 3 is the second item (History).
        let left = MouseEventKind::Down(MouseButton::Left);
        app.on_mouse(mouse(left, 5, 3), t0);
        assert_eq!(app.site_list.state.selected(), Some(1));
        assert_eq!(app.view, View::Sites);
        // A slow second click is just another click; a quick one opens.
        app.on_mouse(mouse(left, 5, 3), t0 + Duration::from_secs(1));
        assert_eq!(app.view, View::Sites);
        app.on_mouse(mouse(left, 5, 3), t0 + Duration::from_millis(1200));
        assert_eq!(app.view, View::History);

        // Clicks outside the list do nothing.
        app.view = View::Sites;
        app.on_mouse(mouse(left, 5, 30), t0);
        assert_eq!(app.site_list.state.selected(), Some(1));
    }

    #[test]
    fn mouse_click_selects_thread_post() {
        let mut app = test_app();
        let post = |no| Post { no, body: vec![Line::raw("a"), Line::raw("b")], ..Default::default() };
        let mut t = ThreadView::new("g".into(), 1, vec![post(1), post(2), post(3)]);
        // Each post: header, two lines, a blank.
        t.layout = Some(ThreadLayout { width: 40, lines: vec![Line::raw(""); 12], starts: vec![0, 4, 8, 12], thumbs: vec![] });
        t.viewport = 10;
        app.thread = Some(t);
        app.view = View::Thread;
        app.hit = Some(Hit::Thread { area: Rect::new(0, 1, 40, 10) });
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 1 + 9), Instant::now());
        assert_eq!(app.thread.as_ref().unwrap().selected, 2);
        app.on_mouse(mouse(MouseEventKind::ScrollDown, 3, 3), Instant::now());
        assert_eq!(app.thread.as_ref().unwrap().scroll, 2);
    }

    #[test]
    fn sleeps_until_the_next_thing_to_do() {
        let mut app = test_app();
        let now = Instant::now();
        // Idle: at most a second.
        assert_eq!(app.next_wake(now), Duration::from_secs(1));
        // A spinner animates.
        app.loading = Some("Loading".into());
        assert_eq!(app.next_wake(now), Duration::from_millis(100));
        app.loading = None;
        // A status message wakes the loop when it's due to disappear.
        app.status = Some(("hi".into(), false));
        app.status_since = Some(("hi".into(), now - Duration::from_millis(1700)));
        assert_eq!(app.next_wake(now), Duration::from_millis(300));
        app.status = None;
        app.status_since = None;
        // A watched thread that was never refreshed is due now.
        app.store.watched.push(crate::store::Watched {
            key: ThreadKey { site: "4chan".into(), board: "g".into(), no: 1 },
            subject: String::new(),
            posts: 1,
            last_seen: 1,
            unread: 0,
            dead: false,
        });
        assert_eq!(app.next_wake(now), Duration::ZERO);
        // But while the maximum number of refreshes is running, due ones don't spin the loop.
        for no in [2, 3] {
            app.refreshing.insert(ThreadKey { site: "4chan".into(), board: "g".into(), no });
        }
        assert_eq!(app.next_wake(now), Duration::from_millis(100));
    }

    #[test]
    fn wakes_as_soon_as_a_message_arrives() {
        let mut app = test_app();
        let tx = app.tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let _ = tx.send(Msg::Wake);
        });
        let start = Instant::now();
        assert!(app.wait(Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_millis(500), "{:?}", start.elapsed());
        // With nothing to wait for, it times out.
        assert!(!app.wait(Duration::from_millis(10)));
    }

    #[test]
    fn partial_pages_show_while_loading_continues() {
        let mut app = test_app();
        let board = |uri: &str| Board { uri: uri.into(), title: String::new(), nsfw: None };
        app.req = 7;
        app.loading = Some("Loading boards".into());
        app.handle(Msg::BoardsPartial(7, 0, vec![board("a")]));
        assert_eq!(app.sites[0].boards.as_ref().unwrap().len(), 1);
        assert!(app.loading.is_some());
        // A stale request's pages are ignored.
        app.handle(Msg::BoardsPartial(6, 0, vec![board("x"), board("y"), board("z")]));
        assert_eq!(app.sites[0].boards.as_ref().unwrap().len(), 1);
        app.handle(Msg::Boards(7, 0, Ok(vec![board("a"), board("b")])));
        assert_eq!(app.sites[0].boards.as_ref().unwrap().len(), 2);
        assert!(app.loading.is_none());

        app.req = 8;
        app.loading = Some("Loading /a/".into());
        app.handle(Msg::CatalogPartial(8, vec![Post { no: 1, ..Default::default() }]));
        assert_eq!((app.catalog.len(), app.loading.is_some()), (1, true));
    }

    #[test]
    fn status_messages_expire() {
        let mut app = test_app();
        let t0 = Instant::now();
        app.status = Some(("No unread posts".into(), false));
        app.expire_status(t0);
        app.expire_status(t0 + Duration::from_millis(1500));
        assert!(app.status.is_some());
        app.expire_status(t0 + Duration::from_secs(2));
        assert!(app.status.is_none());

        // Errors stay longer, and a new message restarts the timer.
        app.status = Some(("Rate limited".into(), true));
        app.expire_status(t0);
        app.expire_status(t0 + Duration::from_secs(4));
        assert!(app.status.is_some());
        app.status = Some(("Thread was deleted or archived".into(), true));
        app.expire_status(t0 + Duration::from_secs(4));
        app.expire_status(t0 + Duration::from_secs(8));
        assert!(app.status.is_some());
        app.expire_status(t0 + Duration::from_secs(9));
        assert!(app.status.is_none());
    }

    #[test]
    fn filtering_a_big_catalog_is_fast() {
        let mut app = test_app();
        let text = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(20);
        app.catalog = (0..300)
            .map(|i| Post { no: i, subject: Some(format!("thread {i}")), body: vec![Line::raw(text.clone())], ..Default::default() })
            .collect();
        app.catalog_list.filter = "thread 29".into();
        assert_eq!(app.visible_catalog().len(), 11); // 29, 290..299
        app.catalog_sort = Sort::Replies;
        let start = Instant::now();
        for _ in 0..100 {
            std::hint::black_box(app.visible_catalog());
        }
        let per_call = start.elapsed() / 100;
        eprintln!("filter + sort of 300 threads: {per_call:?}");
        // A frame is ~16ms; even unoptimized, filtering should take a small fraction of it.
        assert!(per_call < Duration::from_millis(3), "filtering took {per_call:?}");
    }
}
