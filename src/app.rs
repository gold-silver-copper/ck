use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
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
pub use crate::config::Sort;
use crate::config::{CatalogLayout, ColorMode, Config, ImagesMode, SiteConfig};
use crate::disk_cache::DiskCache;
use crate::download;
use crate::filter::{Filters, Mark};
use crate::http;
use crate::images::Images;
use crate::keys::{Action, KeyMap, Scope};
use crate::model::{Attachment, Board, Link, Post};
use crate::store::{Store, ThreadKey};
use crate::theme::{self, ThemeDef, ThemeSetting};

mod gallery;
mod generals;
mod goto;
mod home;
mod links;
mod search;
mod session;
mod tabs;
mod settings;
pub use gallery::Gallery;
pub use home::BoardRef;
pub use links::{ImageSearchPanel, LinkItem, LinksPanel};
pub use search::Search;
pub use tabs::{MAX_TABS, Tab};
pub use settings::{Popup as SettingsPopup, SECTIONS as SETTING_SECTIONS, key_rows, rows as setting_rows, tilde};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Sites,
    Boards,
    Catalog,
    Thread,
    Watched,
    History,
    Settings,
    /// Archive search results.
    Search,
}

/// Saved board lists older than this (seconds) are refreshed in the background.
const BOARDS_MAX_AGE: i64 = 24 * 3600;

/// Background changes to the data directory are written at most this often.
const SAVE_EVERY: Duration = Duration::from_secs(2);

/// Watched-thread refreshes running at once.
const MAX_REFRESHING: usize = 2;


/// A row of the home screen (the Sites view): Watched and History, favorite boards, then
/// the sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteRow {
    Watched,
    History,
    Favorite(usize),
    /// An index into `store.recent_boards`.
    Recent(usize),
    Site(usize),
    /// "N hidden sites": enter shows them (or hides them again).
    HiddenSites,
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

/// One post as shown in a thread: at the top level, or inline under a post it replies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub post: usize,
    /// 0 at the top level; each `e` adds a level.
    pub depth: u8,
    /// Post numbers from the top-level post down to this one.
    pub path: Vec<u64>,
}

/// How deep replies can be expanded inline.
const MAX_DEPTH: u8 = 4;

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
    /// Rendered layout, rebuilt by the UI when the width changes, from cached post lines.
    pub layout: Option<ThreadLayout>,
    pub cache: LineCache,
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
    /// The posts as shown, with replies expanded inline (`e`) where asked.
    pub entries: Vec<Entry>,
    /// Entries (by path) whose replies are expanded.
    pub expanded: HashSet<Vec<u64>>,
    /// The selected entry; `selected` is its post.
    cursor: usize,
    /// What filters and hiding say about each post, and whether hidden ones are shown.
    pub marks: Vec<Mark>,
    pub show_hidden: bool,
    /// Posts marked as yours.
    pub mine: HashSet<u64>,
}

pub struct ThreadLayout {
    pub width: u16,
    /// Each entry's lines: padding, the post, padding, and the gap below it.
    pub blocks: Vec<Rc<[Line<'static>]>>,
    /// `starts[i]` is the first line of entry `i`; has one extra item for the end.
    pub starts: Vec<usize>,
    /// `(line, entry)` for each entry drawn with a thumbnail in the left column.
    pub thumbs: Vec<(usize, usize)>,
}

impl ThreadLayout {
    pub fn len(&self) -> usize {
        self.starts.last().copied().unwrap_or(0)
    }

    /// Line `i`, and the entry it's in.
    pub fn line(&self, i: usize) -> Option<(usize, &Line<'static>)> {
        let e = self.starts.partition_point(|&s| s <= i).checked_sub(1)?;
        Some((e, self.blocks.get(e)?.get(i - self.starts.get(e)?)?))
    }
}

/// A post's lines as last laid out, by (post, text width, with a thumbnail), with a hash of
/// what else they show (time, marks, highlights, ...): reused while that's the same.
pub type LineCache = HashMap<(u64, u16, bool), (u64, Rc<[Line<'static>]>)>;

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
        let entries = (0..posts.len()).map(|i| Entry { post: i, depth: 0, path: vec![posts[i].no] }).collect();
        Self {
            entries,
            expanded: HashSet::new(),
            cursor: 0,
            board,
            no,
            posts,
            index,
            backlinks,
            selected: 0,
            scroll: 0,
            jumps: Vec::new(),
            layout: None,
            cache: LineCache::new(),
            viewport: 0,
            new_after: 0,
            anchor: None,
            search: String::new(),
            matches: Vec::new(),
            revealed: HashSet::new(),
            reveal_all: false,
            marks: Vec::new(),
            show_hidden: false,
            mine: HashSet::new(),
        }
    }

    /// The post is collapsed to a line: hidden, not shown anyway, and not the OP.
    pub fn is_collapsed(&self, i: usize) -> bool {
        i > 0 && !self.show_hidden && self.marks.get(i).is_some_and(|m| m.hidden.is_some())
    }

    /// The selected post.
    pub fn current(&self) -> Option<&Post> {
        self.posts.get(self.selected)
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

    /// The selected entry: the cursor if it's on the selected post, else the post's
    /// top-level entry (code that sets `selected` directly lands there).
    pub fn entry(&self) -> usize {
        match self.entries.get(self.cursor) {
            Some(e) if e.post == self.selected => self.cursor,
            _ => self.entries.iter().position(|e| e.depth == 0 && e.post == self.selected).unwrap_or(0),
        }
    }

    /// Select an entry (without scrolling).
    pub fn set_cursor(&mut self, e: usize) {
        if let Some(entry) = self.entries.get(e) {
            self.cursor = e;
            self.selected = entry.post;
        }
    }

    /// Rebuild `entries` from `expanded`, keeping the cursor on the same path if it's still there.
    fn rebuild_entries(&mut self) {
        let path = self.entries.get(self.entry()).map(|e| e.path.clone());
        let mut out = Vec::with_capacity(self.posts.len());
        for i in 0..self.posts.len() {
            let path = vec![self.posts[i].no];
            out.push(Entry { post: i, depth: 0, path: path.clone() });
            self.push_replies(&mut out, i, path, 1);
        }
        self.entries = out;
        self.layout = None;
        match path.and_then(|p| self.entries.iter().position(|e| e.path == p)) {
            Some(e) => self.set_cursor(e),
            None => self.cursor = 0,
        }
    }

    fn push_replies(&self, out: &mut Vec<Entry>, post: usize, path: Vec<u64>, depth: u8) {
        if depth > MAX_DEPTH || !self.expanded.contains(&path) {
            return;
        }
        for no in &self.backlinks[post] {
            // A post can't contain itself (quote loops).
            let Some(&j) = self.index.get(no).filter(|_| !path.contains(no)) else { continue };
            let mut p = path.clone();
            p.push(*no);
            out.push(Entry { post: j, depth, path: p.clone() });
            self.push_replies(out, j, p, depth + 1);
        }
    }

    /// `e`: show or hide the selected entry's replies under it. Returns what happened.
    fn toggle_expanded(&mut self) -> Result<bool, &'static str> {
        let e = &self.entries[self.entry()];
        if self.backlinks[e.post].is_empty() {
            return Err("No replies to this post");
        }
        if e.depth >= MAX_DEPTH {
            return Err("Replies are expanded as deep as they go here");
        }
        let path = e.path.clone();
        let open = !self.expanded.remove(&path);
        if open {
            self.expanded.insert(path);
        }
        self.rebuild_entries();
        Ok(open)
    }

    /// The entry at the top of the view and how many of its lines are scrolled past.
    fn top_anchor(&self) -> Option<(usize, usize)> {
        let l = self.layout.as_ref()?;
        let top = l.starts.partition_point(|&s| s <= self.scroll).saturating_sub(1);
        Some((top, self.scroll - l.starts[top]))
    }

    /// Select a post's top-level entry.
    fn select(&mut self, i: usize) {
        let i = i.min(self.posts.len().saturating_sub(1));
        let e = self.entries.iter().position(|e| e.depth == 0 && e.post == i).unwrap_or(0);
        self.select_entry(e);
    }

    fn select_entry(&mut self, e: usize) {
        self.set_cursor(e.min(self.entries.len().saturating_sub(1)));
        self.scroll_to_selected();
    }

    /// Adjust scroll so the selected entry is visible (its top, if it's taller than the view).
    pub fn scroll_to_selected(&mut self) {
        let e = self.entry();
        let Some(l) = &self.layout else { return };
        let (start, end) = (l.starts[e], l.starts[e + 1]);
        if start < self.scroll {
            self.scroll = start;
        } else if end > self.scroll + self.viewport {
            self.scroll = start.min(end.saturating_sub(self.viewport));
        }
    }

    /// Scroll by lines, then select the entry at the top of the view.
    fn scroll_lines(&mut self, delta: isize) {
        let Some(l) = &self.layout else { return };
        let max = l.len().saturating_sub(self.viewport);
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
        let top = l.starts.partition_point(|&s| s <= self.scroll).saturating_sub(1);
        // Prefer an entry whose header is on screen.
        let e = if l.starts[top] < self.scroll && top + 1 < self.entries.len() && l.starts[top + 1] < self.scroll + self.viewport {
            top + 1
        } else {
            top
        };
        self.set_cursor(e);
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
    /// The catalog grid: its area, first visible item, columns, and cell size.
    Grid { area: Rect, offset: usize, cols: usize, cell: (u16, u16) },
    /// The settings screen, laid out as `settings::rows()` from `offset`.
    Settings { area: Rect, offset: usize },
}

/// A message in the footer, for a moment (errors a little longer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

impl Status {
    /// How long it replaces the key hints.
    fn ttl(&self) -> Duration {
        Duration::from_secs(if self.error { 5 } else { 2 })
    }
}

/// New posts in a watched thread, for a notification.
struct Note {
    key: ThreadKey,
    subject: String,
    new: usize,
    /// Of those, replies to your posts.
    replies: usize,
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
    /// Link to the post the files are from.
    pub link: Option<String>,
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
    /// A page of archive search results.
    Search(u64, u32, Result<crate::backend::SearchPage>),
    /// The catalog of a followed general's board, to find its next thread.
    GeneralCatalog(ThreadKey, Result<Vec<Post>>),
    /// The thread a quoted post is in: (board, post, thread).
    Found(u64, Board, u64, Result<Option<u64>>),
    Download(DlEvent),
    Input(Event),
    /// Something else (a loaded image) needs a redraw.
    Wake,
}

impl Msg {
    /// The request a response is for.
    fn id(&self) -> Option<u64> {
        match self {
            Msg::Cached(id, _)
            | Msg::Boards(id, ..)
            | Msg::BoardsPartial(id, ..)
            | Msg::Catalog(id, _)
            | Msg::CatalogPartial(id, _)
            | Msg::Thread(id, _)
            | Msg::Search(id, ..)
            | Msg::Found(id, ..) => Some(*id),
            _ => None,
        }
    }
}

pub struct App {
    pub sites: Vec<Site>,
    pub view: View,
    pub site_list: Picker,
    /// Favorite boards (from the config), and board titles for the home screen.
    pub favorites: Vec<BoardRef>,
    pub home_titles: HashMap<String, String>,
    /// Sites left off the home screen (from the config), and whether they're shown anyway.
    pub hidden_sites: std::collections::BTreeSet<String>,
    pub show_hidden_sites: bool,
    pub board_list: Picker,
    pub catalog_list: Picker,
    pub catalog_sort: Sort,
    /// The catalog layout for boards without their own (`catalog_layout` in the config).
    pub default_layout: CatalogLayout,
    /// Columns of the catalog grid as last drawn (0: not a grid).
    pub grid_cols: usize,
    pub settings: settings::Settings,
    pub settings_list: Picker,
    /// The current theme's name, and the config's custom themes.
    pub theme_name: String,
    pub themes: BTreeMap<String, ThemeDef>,
    pub color_mode: ColorMode,
    /// Draw 24-bit colors (else the nearest of 256).
    pub truecolor: bool,
    pub images_mode: ImagesMode,
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
    /// The newest post seen in each watched thread by a refresh this session; notifications
    /// are for posts past it.
    notified_max: HashMap<ThreadKey, u64>,
    /// Followed generals: when each one's board was last searched, and searches running.
    generals_checked: HashMap<ThreadKey, Instant>,
    generals_searching: HashSet<ThreadKey>,
    /// Notifications waiting to be sent together, and since when.
    notes: Vec<Note>,
    notes_since: Option<Instant>,
    pub notify_mode: crate::notify::NotifyMode,
    pub notify_command: Option<Vec<String>>,
    /// Notifications sent (the last few), for the record.
    pub notified: Vec<String>,
    /// Sites whose saved board list is being refreshed in the background.
    boards_refreshing: HashSet<usize>,
    pub site: usize,
    pub board: Option<Board>,
    pub catalog: Vec<Post>,
    /// What filters and hiding say about each catalog thread.
    pub catalog_marks: Vec<Mark>,
    /// Catalog threads that weren't there on the previous visit.
    pub catalog_new: HashSet<u64>,
    pub filters: Filters,
    /// Show hidden threads and posts (dimmed) instead of leaving them out.
    pub show_hidden: bool,
    pub thread: Option<ThreadView>,
    /// True while typing into the filter.
    pub filtering: bool,
    /// Label of the in-flight request, if any.
    pub loading: Option<String>,
    pub status: Option<Status>,
    /// The status message as last seen by `expire_status`, and when it appeared.
    status_since: Option<(String, Instant)>,
    pub show_help: bool,
    pub help_scroll: u16,
    pub images: Images,
    pub viewer: Option<Viewer>,
    pub preview: Option<Preview>,
    pub links: Option<LinksPanel>,
    /// Reverse image search engines (`R`), and the panel choosing one.
    pub image_search: Vec<crate::config::ImageSearch>,
    pub image_search_panel: Option<ImageSearchPanel>,
    /// The thread's files as a grid (`V`), over the thread.
    pub gallery: Option<Gallery>,
    /// Save where you are and start there next time.
    pub restore_session: bool,
    /// The data directory has changes to write, and when it was last written.
    save_pending: bool,
    saved_at: Instant,
    /// The session as last saved, and when that was checked.
    session_saved: (Option<crate::store::Session>, Instant),
    /// Thread to select in the catalog once it loads (restoring a session).
    pending_catalog: Option<u64>,
    /// The open thread is being restored from the last session.
    restoring: bool,
    /// Archive search: its results, the list over them, and the query being typed.
    pub search: Option<Search>,
    pub search_list: Picker,
    pub search_input: Option<String>,
    /// True while typing a thread search.
    pub searching: bool,
    /// What's typed after `:`, while it's being typed.
    pub goto: Option<String>,
    /// Threads left by following cross-thread links: (site, board, thread, selected post), for `u`.
    trail: Vec<(usize, Board, u64, u64)>,
    /// Post to select once the loading thread arrives.
    pending_post: Option<u64>,
    /// Board the loaded catalog belongs to.
    catalog_board: String,
    /// The board whose catalog is loaded, to return to from a thread opened on another board
    /// (an overboard's threads live on their own boards).
    catalog_of: Option<Board>,
    /// The open thread was opened from the catalog (not by following a link).
    from_catalog: bool,
    /// After a thread 404'd: the same thread on the site's configured archive.
    archive_offer: Option<ThreadKey>,
    pub keys: KeyMap,
    /// The last text copied to the clipboard, and the last URL opened.
    pub copied: Option<String>,
    pub opened: Option<String>,
    /// The config file that settings are saved to (tests point it elsewhere).
    pub config_path: Option<std::path::PathBuf>,
    pub clock: Clock,
    pub downloads: Downloads,
    download_dir: Option<String>,
    /// Set by the UI every frame.
    pub hit: Option<Hit>,
    /// Where each tab's chip was drawn, for clicks.
    pub tab_chips: Vec<(Rect, usize)>,
    /// Last left click: when, and the list index or thread post it hit.
    last_click: Option<(Instant, usize)>,
    pub tick: usize,
    pub quit: bool,
    /// The other tabs (the active one's slot holds nothing useful), and which is active.
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// The last request id given out (ids are unique across tabs).
    next_id: u64,
    /// The active tab's request in flight (0: none).
    req: u64,
    /// The thread number of the last thread load, for 404 handling.
    pending_thread: u64,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl App {
    pub fn new(cfg: Config, keys: KeyMap, picker: Option<ratatui_image::picker::Picker>, store: Store) -> Self {
        // ck 0.2's [theme] table of overrides becomes a theme of its own, "legacy".
        let mut themes = cfg.themes.clone();
        let theme_name = match &cfg.theme {
            None => theme::DEFAULT_THEME.to_string(),
            Some(ThemeSetting::Name(n)) => n.clone(),
            Some(ThemeSetting::Legacy(old)) => {
                let colors = old
                    .iter()
                    .map(|(k, v)| {
                        let role = theme::LEGACY_KEYS.iter().find(|(l, _)| l == k).map_or(k.as_str(), |(_, r)| r);
                        (role.to_string(), v.clone())
                    })
                    .collect();
                let def = ThemeDef { base: Some(theme::DEFAULT_THEME.into()), colors, ..Default::default() };
                themes.insert("legacy".into(), def);
                "legacy".into()
            }
        };
        let layout = cfg.layout();
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
            favorites: cfg.favorites.iter().filter_map(|f| BoardRef::parse(f)).collect(),
            home_titles: HashMap::new(),
            hidden_sites: cfg.hidden_sites.iter().cloned().collect(),
            show_hidden_sites: false,
            board_list: Picker::default(),
            catalog_list: Picker::default(),
            catalog_sort: Sort::default(),
            grid_cols: 0,
            default_layout: store.settings.catalog_layout.unwrap_or(match store.settings.compact_catalog {
                Some(true) => CatalogLayout::Compact,
                Some(false) => CatalogLayout::Cards,
                None => layout,
            }),
            settings: settings::Settings::default(),
            settings_list: Picker::default(),
            theme_name,
            themes,
            color_mode: cfg.color,
            truecolor: cfg.color.truecolor(),
            images_mode: cfg.images,
            watched_list: Picker::default(),
            history_list: Picker::default(),
            store,
            return_to: None,
            refresh_thread,
            refresh_watched,
            thread_checked: Instant::now(),
            watched_checked: HashMap::new(),
            refreshing: HashSet::new(),
            notified_max: HashMap::new(),
            generals_checked: HashMap::new(),
            generals_searching: HashSet::new(),
            notes: Vec::new(),
            notes_since: None,
            notify_mode: cfg.notify,
            notify_command: cfg.notify_command.clone(),
            notified: Vec::new(),
            boards_refreshing: HashSet::new(),
            site: 0,
            board: None,
            catalog: Vec::new(),
            catalog_marks: Vec::new(),
            catalog_new: HashSet::new(),
            filters: Filters::new(&cfg.filters).unwrap_or_default(),
            show_hidden: false,
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
            links: None,
            image_search: if cfg.image_search.is_empty() { crate::config::ImageSearch::defaults() } else { cfg.image_search.clone() },
            image_search_panel: None,
            gallery: None,
            restore_session: cfg.restore_session,
            save_pending: false,
            saved_at: Instant::now(),
            session_saved: (None, Instant::now()),
            pending_catalog: None,
            restoring: false,
            search: None,
            search_list: Picker::default(),
            search_input: None,
            searching: false,
            goto: None,
            trail: Vec::new(),
            pending_post: None,
            catalog_board: String::new(),
            catalog_of: None,
            from_catalog: false,
            archive_offer: None,
            keys,
            copied: None,
            opened: None,
            config_path: Config::path(),
            clock: Clock::default(),
            downloads: Downloads::default(),
            download_dir: cfg.download_dir.clone(),
            hit: None,
            tab_chips: Vec::new(),
            last_click: None,
            tick: 0,
            quit: false,
            tabs: vec![Tab::new(0)],
            active: 0,
            next_id: 0,
            req: 0,
            pending_thread: 0,
            tx,
            rx,
        };
        app.site_list.state.select(Some(0));
        app.load_home_titles();
        app.watched_list.state.select(Some(0));
        app.history_list.state.select(Some(0));
        app.settings_list.state.select(Some(0));
        app
    }

    // ----- visible (filtered) items -----

    pub fn visible_sites(&self) -> Vec<SiteRow> {
        let rows: Vec<SiteRow> = [SiteRow::Watched, SiteRow::History]
            .into_iter()
            .chain((0..self.favorites.len()).map(SiteRow::Favorite))
            .chain(self.recent_rows().into_iter().map(SiteRow::Recent))
            .chain((0..self.sites.len()).filter(|&i| self.show_hidden_sites || !self.is_site_hidden(i)).map(SiteRow::Site))
            .chain((!self.hidden_sites.is_empty()).then_some(SiteRow::HiddenSites))
            .collect();
        let name = |k: usize| match &rows[k] {
            SiteRow::Watched => "Watched".to_string(),
            SiteRow::History => "History".to_string(),
            SiteRow::Favorite(i) => {
                let f = &self.favorites[*i];
                format!("{} /{}/ {}", f.site, f.board, self.board_title(f))
            }
            SiteRow::Recent(i) => {
                let r = self.recent_board(*i);
                r.map_or(String::new(), |r| format!("{} /{}/ {}", r.site, r.board, self.board_title(&r)))
            }
            SiteRow::Site(i) => self.sites[*i].cfg.name.clone(),
            SiteRow::HiddenSites => "hidden sites".to_string(),
        };
        filtered(&self.site_list.filter, rows.len(), name).into_iter().map(|i| rows[i]).collect()
    }

    pub fn visible_watched(&self) -> Vec<usize> {
        let w = &self.store.watched;
        filtered(&self.watched_list.filter, w.len(), |i| format!("{} {} {} {}", w[i].key.site, w[i].key.board, w[i].key.no, w[i].subject))
    }

    pub fn visible_history(&self) -> Vec<usize> {
        let h = &self.store.history;
        filtered(&self.history_list.filter, h.len(), |i| format!("{} {} {} {}", h[i].key.site, h[i].key.board, h[i].key.no, h[i].subject))
    }

    pub fn boards(&self) -> &[Board] {
        self.sites[self.site].boards.as_deref().unwrap_or(&[])
    }

    pub fn visible_boards(&self) -> Vec<usize> {
        let b = self.boards();
        filtered(&self.board_list.filter, b.len(), |i| format!("{} {}", b[i].uri, b[i].title))
    }

    pub fn visible_catalog(&self) -> Vec<usize> {
        let needle = self.catalog_list.filter.to_lowercase();
        let shown = |i: usize| self.show_hidden || self.catalog_marks.get(i).is_none_or(|m| m.hidden.is_none());
        let mut v: Vec<usize> =
            (0..self.catalog.len()).filter(|&i| shown(i) && self.catalog[i].search_text().contains(&needle)).collect();
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
            View::Settings => settings::items().len(),
            View::Search => self.search.as_ref().map_or(0, |s| s.hits.len()),
            View::Thread => return None,
        };
        let p = match self.view {
            View::Sites => &mut self.site_list,
            View::Boards => &mut self.board_list,
            View::Catalog => &mut self.catalog_list,
            View::Watched => &mut self.watched_list,
            View::History => &mut self.history_list,
            View::Settings => &mut self.settings_list,
            View::Search => &mut self.search_list,
            View::Thread => return None,
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
        self.flush_notes(Instant::now());
        self.save_session(Some(Instant::now()));
        if self.save_pending && self.saved_at.elapsed() >= SAVE_EVERY {
            self.save_now();
        }
        self.expire_status(Instant::now());
    }

    /// Send waiting notifications together: once no refresh is running, or 3s after the
    /// first, so several threads' news makes one notification.
    fn flush_notes(&mut self, now: Instant) {
        let Some(since) = self.notes_since else { return };
        if !self.refreshing.is_empty() && now.duration_since(since) < Duration::from_secs(3) {
            return;
        }
        let notes = std::mem::take(&mut self.notes);
        self.notes_since = None;
        let method = crate::notify::method(self.notify_mode, self.notify_command.as_deref(), &|k| std::env::var(k).ok());
        let place = |n: &Note| format!("/{}/ {}", n.key.board, n.subject.chars().take(60).collect::<String>());
        let replies: Vec<&Note> = notes.iter().filter(|n| n.replies > 0).collect();
        let mut messages = Vec::new();
        match replies.as_slice() {
            [] => {}
            [n] if n.replies == 1 => messages.push(format!("New reply to your post in {}", place(n))),
            [n] => messages.push(format!("{} new replies to your posts in {}", n.replies, place(n))),
            many => {
                let total: usize = many.iter().map(|n| n.replies).sum();
                messages.push(format!("{total} new replies to your posts in {} threads", many.len()));
            }
        }
        let others: Vec<&Note> = notes.iter().filter(|n| n.new > n.replies).collect();
        match others.as_slice() {
            [] => {}
            [n] => {
                let k = n.new - n.replies;
                messages.push(format!("{k} new post{} in {}", if k == 1 { "" } else { "s" }, place(n)));
            }
            many => messages.push(format!("{} watched threads have new posts", many.len())),
        }
        for m in &messages {
            if let Err(e) = crate::notify::send(&method, "ck", m) {
                self.error(format!("Couldn't notify: {e:#}"));
            }
        }
        if let Some(m) = messages.first() {
            self.status.get_or_insert_with(|| Status { text: m.clone(), error: false });
        }
        self.notified.extend(messages);
        let len = self.notified.len();
        self.notified.drain(..len.saturating_sub(20));
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
        if let Some(since) = self.notes_since {
            at(since + Duration::from_secs(3));
        }
        if self.save_pending {
            at(self.saved_at + SAVE_EVERY);
        }
        // An animated GIF in the viewer: its next frame.
        if let Some(t) = self.images.next_frame() {
            at(t);
        }
        if let (Some(s), Some((_, since))) = (&self.status, &self.status_since) {
            at(*since + s.ttl());
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
        // A response for another tab is handled there.
        if let Some(i) = msg.id().filter(|&id| id != self.req).and_then(|id| self.tab_of(id)) {
            self.handle_in_tab(i, msg);
            return;
        }
        {
            match msg {
                Msg::Wake => {}
                Msg::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => self.on_key(key),
                Msg::Input(Event::Mouse(m)) => self.on_mouse(m, Instant::now()),
                Msg::Input(Event::Paste(text)) => self.paste(&text),
                Msg::Input(_) => {}
                Msg::Refreshed(key, res) => self.refreshed(key, res),
                Msg::GeneralCatalog(key, res) => self.general_catalog(key, res),
                Msg::Download(ev) => self.download_event(ev),
                Msg::Found(id, board, post, res) if id == self.req => {
                    self.loading = None;
                    match res {
                        Ok(Some(no)) => {
                            if let Some(t) = &self.thread {
                                self.trail.push((self.site, self.board.clone().unwrap_or(board.clone()), t.no, t.current().map_or(t.no, |p| p.no)));
                            }
                            self.open_thread_at(board, no, Some(post), true);
                        }
                        Ok(None) => {
                            let msg = format!("Post {post} isn't in this thread, and this site can't say which thread it's in");
                            self.error(msg);
                        }
                        Err(e) => self.error(e),
                    }
                }
                Msg::Search(id, page, res) if id == self.req => {
                    self.loading = None;
                    self.search_results(page, res);
                }
                Msg::Cached(id, age) if id == self.req && self.status.is_none() => {
                    self.info(format!("Up to date (checked {}s ago)", age.as_secs()));
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
                    self.remark_catalog();
                    let len = self.visible_catalog().len();
                    self.catalog_list.clamp(len);
                }
                Msg::Catalog(id, res) if id == self.req => {
                    self.loading = None;
                    match res {
                        Ok(posts) => {
                            self.catalog = posts;
                            self.remark_catalog();
                            self.catalog_seen();
                            if let Some(no) = self.pending_catalog.take()
                                && let Some(i) = self.visible_catalog().iter().position(|&k| self.catalog[k].no == no)
                            {
                                self.catalog_list.state.select(Some(i));
                            }
                            let len = self.visible_catalog().len();
                            self.catalog_list.clamp(len);
                        }
                        Err(e) => self.error(e),
                    }
                }
                Msg::Thread(id, res) if id == self.req => {
                    self.loading = None;
                    self.thread_checked = Instant::now();
                    let restoring = std::mem::take(&mut self.restoring);
                    match res {
                        Ok(posts) => self.set_thread(posts),
                        // Last session's thread is gone: its catalog instead.
                        Err(e) if restoring && http::is_not_found(&e) => {
                            self.view = View::Catalog;
                            self.load_catalog();
                            self.info("The thread you had open last time is gone (archived or deleted)");
                        }
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
        let Some(status) = &self.status else {
            self.status_since = None;
            return;
        };
        match &self.status_since {
            Some((seen, since)) if *seen == status.text => {
                if now.duration_since(*since) >= status.ttl() {
                    self.status = None;
                    self.status_since = None;
                }
            }
            _ => self.status_since = Some((status.text.clone(), now)),
        }
    }

    /// Say something in the footer for a moment.
    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: false });
    }

    /// Say something went wrong (shown a little longer).
    pub fn error(&mut self, e: impl std::fmt::Display) {
        self.status = Some(Status { text: format!("{e:#}"), error: true });
    }

    /// Run `job` on a thread, as the current request (shown as `label`). The job gets the
    /// request id and a sender for partial results.
    fn spawn<T: Send + 'static>(
        &mut self,
        label: String,
        job: impl FnOnce(&dyn Backend, u64, &Sender<Msg>) -> Result<T> + Send + 'static,
        wrap: impl FnOnce(u64, Result<T>) -> Msg + Send + 'static,
    ) {
        self.next_id += 1;
        self.req = self.next_id;
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
                self.error(format!("Couldn't save the board list: {e:#}"));
            }
        }
        self.sites[site].boards = Some(boards);
        if site == self.site {
            let len = self.visible_boards().len();
            self.board_list.clamp(len);
        }
        if complete {
            self.note_titles(site);
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
        self.catalog_of = Some(board.clone());
        self.apply_board_sort();
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

    /// Save soon: background changes (refreshes, visits) are written together, every few
    /// seconds and on quit.
    fn save(&mut self) {
        self.save_pending = true;
    }

    /// Save now (what the user just did).
    pub fn save_now(&mut self) {
        self.save_pending = false;
        self.saved_at = Instant::now();
        if let Err(e) = self.store.save() {
            self.error(format!("Couldn't save watched threads: {e:#}"));
        }
    }

    fn set_thread(&mut self, posts: Vec<Post>) {
        let Some(board) = self.board.as_ref().map(|b| b.uri.clone()) else { return };
        let no = posts.first().map(|p| p.no).unwrap_or(0);
        let key = self.key(&board, no);
        let mut tv = ThreadView::new(board, no, posts);
        // On a refresh, the newest post already shown.
        let mut shown_max = None;
        match self.thread.take().filter(|t| t.no == no && t.board == tv.board) {
            // On refresh, keep the selected post and what's at the top of the view.
            Some(old) => {
                shown_max = old.posts.iter().map(|p| p.no).max();
                tv.selected = tv.index.get(&old.posts[old.selected].no).copied().unwrap_or(0);
                // Expanded replies, the selected entry and the one at the top stay put.
                tv.expanded = old.expanded.clone();
                let cursor_path = old.entries.get(old.entry()).map(|e| e.path.clone());
                tv.rebuild_entries();
                if let Some(e) = cursor_path.and_then(|p| tv.entries.iter().position(|e| e.path == p)) {
                    tv.set_cursor(e);
                }
                let at = |p: &Vec<u64>| tv.entries.iter().position(|e| e.path == *p);
                tv.anchor = old.top_anchor().and_then(|(i, off)| Some((at(&old.entries.get(i)?.path)?, off)));
                tv.scroll = old.scroll;
                tv.viewport = old.viewport;
                tv.cache = old.cache;
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
        // A visit is an open, or a refresh that brought new posts.
        if shown_max.is_none_or(|m| max_no > m) {
            self.store.visit(&key, &subject, tv.posts.len(), max_no, self.clock.now());
        }
        self.store.opened(&key.site, &key.board, key.no, tv.posts.len().saturating_sub(1) as u32, self.clock.now());
        if let Some(w) = self.store.watched_mut(&key) {
            generals::note_limit(w, &tv.posts);
        }
        self.save();
        self.thread = Some(tv);
        self.remark_thread();
    }

    // ----- filters and hiding -----

    /// What filters and hiding by hand say about posts (on the board `board_of` gives each).
    fn marks(&self, posts: &[Post], board_of: impl Fn(&Post) -> String) -> Vec<Mark> {
        let site = &self.current_site().cfg.name;
        let mut hidden: HashMap<String, HashSet<u64>> = HashMap::new();
        let mark = |p: &Post| {
            let board = board_of(p);
            let mut m = self.filters.check(site, &board, p);
            let by_hand = hidden.entry(board).or_insert_with_key(|b| self.store.hidden_on(site, b));
            if m.hidden.is_none() && by_hand.contains(&p.no) {
                m.hidden = Some(String::new());
            }
            m
        };
        posts.iter().map(mark).collect()
    }

    /// Note which catalog threads are new since the last visit (and remember them all).
    fn catalog_seen(&mut self) {
        let nos: Vec<u64> = self.catalog.iter().map(|p| p.no).collect();
        let site = self.current_site().cfg.name.clone();
        let board = self.catalog_board.clone();
        self.catalog_new = self.store.catalog_seen(&site, &board, &nos, self.clock.now());
        self.store.board_opened(&site, &board);
        self.note_titles(self.site);
        self.save();
    }

    /// Replies a catalog thread has gained since it was last opened.
    pub fn new_replies(&self, p: &Post) -> Option<u32> {
        let seen = self.store.replies_seen(&self.current_site().cfg.name, &self.board_of(p), p.no)?;
        p.replies.filter(|&r| r > seen).map(|r| r - seen)
    }

    pub fn remark_catalog(&mut self) {
        let marks = self.marks(&self.catalog, |p| self.board_of(p));
        self.catalog_marks = marks;
    }

    /// The board a catalog thread is on (overboards mix boards).
    pub fn board_of(&self, p: &Post) -> String {
        let loaded = Some(self.catalog_board.clone()).filter(|b| !b.is_empty());
        p.board.clone().or(loaded).or_else(|| self.board.as_ref().map(|b| b.uri.clone())).unwrap_or_default()
    }

    pub fn remark_thread(&mut self) {
        let Some(t) = &self.thread else { return };
        let marks = self.marks(&t.posts, |_| t.board.clone());
        let mine = self.store.watched(&self.key(&t.board, t.no)).map(|w| w.mine.iter().copied().collect()).unwrap_or_default();
        let show = self.show_hidden;
        if let Some(t) = &mut self.thread {
            t.marks = marks;
            t.mine = mine;
            t.show_hidden = show;
            t.layout = None;
        }
    }

    /// `H`: hide or unhide the selected thread (catalog) or post (thread) by hand.
    fn toggle_hidden(&mut self) {
        let (board, no, what, mark) = match self.view {
            View::Catalog => {
                let Some(i) = self.selected_index() else { return };
                let p = &self.catalog[i];
                (self.board_of(p), p.no, "thread", self.catalog_marks.get(i).cloned())
            }
            View::Thread => {
                let Some(t) = &self.thread else { return };
                let what = if t.selected == 0 { "thread" } else { "post" };
                (t.board.clone(), t.current().map_or(t.no, |p| p.no), what, t.marks.get(t.selected).cloned())
            }
            _ => return,
        };
        if let Some(label) = mark.and_then(|m| m.hidden).filter(|l| !l.is_empty()) {
            let msg = format!("Hidden by the filter \"{label}\"; change [[filter]] in the config to show it");
            self.info(msg);
            return;
        }
        let site = self.current_site().cfg.name.clone();
        let hidden = self.store.toggle_hidden(&site, &board, no);
        self.save_now();
        let show = self.keys.key(Action::ShowHidden);
        self.info(if hidden { format!("Hid {what} {no} ({show} shows hidden ones)") } else { format!("Unhid {what} {no}") });
        self.remark_catalog();
        self.remark_thread();
        if let Some((p, len)) = self.picker() {
            p.clamp(len);
        }
    }

    /// `Z`: show hidden threads and posts (dimmed), or leave them out again.
    fn toggle_show_hidden(&mut self) {
        // Keep the same thread selected in the catalog.
        let keep = self.selected_index().filter(|_| self.view == View::Catalog);
        self.show_hidden = !self.show_hidden;
        self.remark_thread();
        if let Some(i) = keep
            && let Some(pos) = self.visible_catalog().iter().position(|&v| v == i)
        {
            self.catalog_list.state.select(Some(pos));
        }
        if let Some((p, len)) = self.picker() {
            p.clamp(len);
        }
        let msg = if self.show_hidden { "Showing hidden threads and posts" } else { "Leaving out hidden threads and posts" };
        self.info(msg);
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
        self.check_generals(Instant::now());
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
                let prev = self.notified_max.get(&key).copied();
                let Some(w) = self.store.watched_mut(&key) else { return };
                let max_no = posts.iter().map(|p| p.no).max().unwrap_or(0);
                if w.last_seen == 0 {
                    w.last_seen = max_no;
                }
                let mine = w.mine.clone();
                let to_you = |p: &Post| p.quotes.iter().any(|q| mine.contains(q));
                let unread: Vec<&Post> = posts.iter().filter(|p| p.no > w.last_seen).collect();
                w.unread = unread.len();
                w.replies = unread.iter().filter(|p| to_you(p)).count();
                // Tell about posts newer than this session's last refresh (not on the first
                // one, which may find posts from long ago).
                let fresh: Vec<&&Post> = unread.iter().filter(|p| prev.is_some_and(|m| p.no > m)).collect();
                let note = Note {
                    key: key.clone(),
                    subject: if w.subject.is_empty() { subject.clone() } else { w.subject.clone() },
                    new: fresh.len(),
                    replies: fresh.iter().filter(|p| to_you(p)).count(),
                };
                w.posts = posts.len();
                w.dead = false;
                generals::note_limit(w, &posts);
                if w.subject.is_empty() {
                    w.subject = subject;
                }
                self.notified_max.insert(key, max_no);
                if note.new > 0 {
                    self.notes_since.get_or_insert_with(Instant::now);
                    self.notes.push(note);
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
        let posts: Vec<&Post> = if whole_thread { t.posts.iter().collect() } else { t.current().into_iter().collect() };
        let dir = download::dir(self.download_dir.as_deref(), &self.current_site().cfg.name, &t.board, t.no);
        let jobs = download::jobs(&posts, &dir);
        self.start_download(jobs, dir, if whole_thread { "Thread has no files" } else { "Post has no file" });
    }

    /// `E`: save the thread as thread.html and thread.json in its download folder.
    fn export_thread(&mut self) {
        let (Some(t), Some(b)) = (&self.thread, &self.board) else { return };
        let site = self.current_site();
        let dir = download::dir(self.download_dir.as_deref(), &site.cfg.name, &t.board, t.no);
        let url = site.backend.thread_url(&b.uri, t.no);
        let about = crate::export::About { site: &site.cfg.name, board: &t.board, thread: t.no, url: &url, saved: self.clock.now() };
        match crate::export::save(&t.posts, &about, &theme::theme(), &dir) {
            Ok(()) => self.info(format!("Saved thread.html and thread.json in {}", tilde(&dir.display().to_string()))),
            Err(e) => self.error(format!("Couldn't save the thread: {e:#}")),
        }
    }

    /// Fetch `(url, path)` jobs into `dir` in the background.
    fn start_download(&mut self, jobs: Vec<(String, std::path::PathBuf)>, dir: std::path::PathBuf, none: &str) {
        if jobs.is_empty() {
            self.info(none);
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.error(format!("Couldn't create {}: {e}", dir.display()));
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
                    self.status = Some(Status { text: msg, error: d.failed > 0 });
                }
            }
        }
    }

    /// The open catalog's board, as `site/board` (for its own sort and layout).
    fn board_key(&self) -> String {
        let board = Some(self.catalog_board.clone()).filter(|b| !b.is_empty()).or_else(|| self.board.as_ref().map(|b| b.uri.clone()));
        format!("{}/{}", self.current_site().cfg.name, board.unwrap_or_default())
    }

    /// The catalog layout here: the board's own, or the default.
    pub fn layout(&self) -> CatalogLayout {
        self.store.board_prefs.get(&self.board_key()).and_then(|p| p.layout).unwrap_or(self.default_layout)
    }

    /// `c` in a catalog: cycle this board's layout (cards, compact, grid), remembered for it.
    fn cycle_layout(&mut self) {
        let layout = self.layout().next();
        let key = self.board_key();
        self.store.board_prefs.entry(key).or_default().layout = Some(layout);
        self.save_now();
        let board = self.board.as_ref().map_or(String::new(), |b| format!(" for /{}/", b.uri));
        self.info(format!("Layout{board}: {} (the default is in Settings)", layout.as_str()));
    }

    /// The board's own sort, when its catalog opens.
    fn apply_board_sort(&mut self) {
        self.catalog_sort = self.store.board_prefs.get(&self.board_key()).and_then(|p| p.sort).unwrap_or_default();
    }

    /// The default catalog layout (Settings): saved in config.toml (keeping its comments), or
    /// in the data directory if the config can't be edited.
    pub fn cycle_default_layout(&mut self) {
        self.default_layout = self.default_layout.next();
        let name = self.default_layout.as_str();
        match self.edit_config(|d| {
            d["catalog_layout"] = toml_edit::value(name);
            d.remove("compact_catalog");
        }) {
            Ok(path) => {
                self.store.settings.compact_catalog = None;
                self.store.settings.catalog_layout = None;
                self.info(format!("Default catalog layout: {name} (saved in {path})"));
            }
            Err(e) => {
                self.store.settings.compact_catalog = None;
                self.store.settings.catalog_layout = Some(self.default_layout);
                self.save_now();
                self.info(format!("Default catalog layout: {name} (kept in the data directory: {e:#})"));
            }
        }
    }

    // ----- mouse -----

    /// Wheel scrolls, a click selects, a double click opens.
    pub fn on_mouse(&mut self, ev: MouseEvent, now: Instant) {
        http::user_input();
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
            let key = |c| KeyEvent::from(if c { KeyCode::Down } else { KeyCode::Up });
            if self.show_help || self.viewer.is_some() || self.preview.is_some() || self.links.is_some() || self.image_search_panel.is_some() {
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
        let pos = ratatui::layout::Position::new(ev.column, ev.row);
        if let Some(&(_, i)) = self.tab_chips.iter().find(|(r, _)| r.contains(pos)) {
            self.switch_tab(i);
            return;
        }
        if self.links.is_some() {
            self.on_links_click(ev.column, ev.row, now);
            return;
        }
        if self.image_search_panel.is_some() {
            self.on_image_search_click(ev.column, ev.row);
            return;
        }
        if self.gallery.is_some() && self.view == View::Thread && self.viewer.is_none() {
            let Some(target) = self.click_target(ev.column, ev.row) else { return };
            let double = self.last_click.is_some_and(|(t, i)| i == target && now.duration_since(t) < Duration::from_millis(400));
            self.last_click = if double { None } else { Some((now, target)) };
            if let Some(g) = &mut self.gallery
                && target < g.files.len()
            {
                g.state.select(Some(target));
                if double {
                    self.view_from_gallery(target);
                }
            }
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
                t.set_cursor(target);
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
            Hit::Settings { area, offset } if area.contains(pos) => settings::rows().get(offset + (row - area.y) as usize)?.ok(),
            Hit::Grid { area, offset, cols, cell } if area.contains(pos) => {
                let c = ((col - area.x) / cell.0) as usize;
                (c < cols).then(|| offset + ((row - area.y) / cell.1) as usize * cols + c)
            }
            Hit::Thread { area } if area.contains(pos) => {
                let t = self.thread.as_ref()?;
                let l = t.layout.as_ref()?;
                let line = t.scroll + (row - area.y) as usize;
                l.line(line).map(|(e, _)| e)
            }
            _ => None,
        }
    }

    /// A thread 404'd: say so, and offer the site's archive if it has one.
    fn thread_gone(&mut self, key: &ThreadKey) {
        let archive = self.sites.iter().find(|s| s.cfg.name == key.site).and_then(|s| s.cfg.archive.clone());
        match archive.filter(|a| self.sites.iter().any(|s| s.cfg.name == *a)) {
            Some(a) => {
                self.error(format!("Thread was deleted or archived. Press a to open it in {a}"));
                self.archive_offer = Some(ThreadKey { site: a, board: key.board.clone(), no: key.no });
            }
            None => self.error("Thread was deleted or archived"),
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
                let board = op.board.clone().unwrap_or_else(|| b.uri.clone());
                // Unknown until the first refresh, which then counts nothing as unread.
                (board, op.no, thread_subject(std::slice::from_ref(op)), posts, 0)
            }
            _ => return,
        };
        let key = self.key(&board, no);
        let watching = self.store.toggle_watch(key.clone(), subject, posts, last_seen);
        self.watched_checked.remove(&key);
        self.info(if watching { format!("Watching thread {no}") } else { format!("Stopped watching thread {no}") });
        self.save_now();
    }

    /// `m`: mark the selected post as yours (or not), to hear about replies to it. The
    /// thread is watched if it isn't.
    fn toggle_mine(&mut self) {
        let Some(t) = &self.thread else { return };
        let key = self.key(&t.board, t.no);
        let Some(no) = t.current().map(|p| p.no) else { return };
        if self.store.watched(&key).is_none() {
            let max_no = t.posts.iter().map(|p| p.no).max().unwrap_or(0);
            self.store.toggle_watch(key.clone(), thread_subject(&t.posts), t.posts.len(), max_no);
        }
        let Some(w) = self.store.watched_mut(&key) else { return };
        let mine = match w.mine.iter().position(|&n| n == no) {
            Some(i) => {
                w.mine.remove(i);
                false
            }
            None => {
                w.mine.push(no);
                true
            }
        };
        self.info(if mine { format!("Marked No.{no} as yours; replies to it will be counted and notified") } else { format!("No.{no} isn't marked as yours any more") });
        self.save_now();
        self.remark_thread();
    }

    /// Open a thread from Watched or History, switching site and board as needed.
    fn open_key(&mut self, key: ThreadKey) {
        let Some(site) = self.sites.iter().position(|s| s.cfg.name == key.site) else {
            self.error(format!("No site named {} in the config", key.site));
            return;
        };
        self.switch_site(site);
        let board = self.boards().iter().find(|b| b.uri == key.board).cloned();
        self.board = Some(board.unwrap_or(Board { uri: key.board, title: String::new(), nsfw: None }));
        self.return_to = Some(self.view);
        self.from_catalog = false;
        self.gallery = None;
        self.thread = None;
        self.view = View::Thread;
        self.load_thread(key.no);
    }

    /// Make `site` the current one, with its board list if it's saved (else fetched in the
    /// background), so going back to Boards shows it.
    fn switch_site(&mut self, site: usize) {
        if site == self.site {
            return;
        }
        self.board_list = Picker::default();
        self.board_list.state.select(Some(0));
        self.site = site;
        if self.sites[site].boards.is_none() {
            let cfg = &self.sites[site].cfg;
            if let Some(b) = &cfg.boards {
                self.sites[site].boards = Some(b.iter().map(backend::to_board).collect());
            } else if let Some((boards, _)) = self.store.load_boards(&cfg.name) {
                self.sites[site].boards = Some(boards);
            } else {
                self.refresh_boards_in_background(site);
            }
        }
    }

    // ----- input -----

    pub fn on_key(&mut self, key: KeyEvent) {
        http::user_input();
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.on_settings_popup_key(key) {
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
        if self.image_search_panel.is_some() {
            self.on_image_search_key(key.code);
            return;
        }
        if self.viewer.is_some() {
            match self.keys.action(Scope::Viewer, &key) {
                Some(action) => self.act(action),
                None => self.on_viewer_key(key.code),
            }
            return;
        }
        if self.preview.is_some() {
            self.on_preview_key(key.code);
            return;
        }
        if self.gallery.is_some() && self.view == View::Thread {
            self.on_gallery_key(key);
            return;
        }
        if self.links.is_some() {
            self.on_links_key(key.code);
            return;
        }
        if self.goto.is_some() {
            self.on_goto_key(key);
            return;
        }
        if self.search_input.is_some() {
            self.on_search_input_key(key);
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
        if self.view == View::Search && self.keys.keys(Action::NextMatch).contains(&crate::keys::Key::from_event(&key)) {
            if self.more_results() {
                self.load_search_page();
            }
            return;
        }
        if let Some(action) = self.keys.action(self.scope(), &key) {
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
            _ if self.view == View::Catalog && self.grid_cols > 0 && self.on_grid_key(key.code) => {}
            KeyCode::Char(c @ '1'..='9') if self.view == View::Sites => self.open_favorite(c as usize - '1' as usize),
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
                if self.view == View::Search {
                    self.search_moved();
                }
            }
        }
    }

    pub fn scope(&self) -> Scope {
        match self.view {
            View::Sites | View::Boards | View::Settings | View::Search => Scope::Lists,
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
            Action::Settings => self.open_settings(),
            Action::Search if self.view == View::Settings => {}
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
            Action::Goto => self.goto = Some(String::new()),
            Action::Links => self.open_links(),
            Action::Hide => self.toggle_hidden(),
            Action::Mine => self.toggle_mine(),
            Action::Gallery => self.open_gallery(),
            Action::Export => self.export_thread(),
            Action::ArchiveSearch => self.start_archive_search(),
            Action::ImageSearch => self.open_image_search(),
            Action::NewTab => self.new_tab(),
            Action::Favorite => self.toggle_favorite(),
            Action::Follow => self.toggle_follow(),
            Action::NextTab => self.cycle_tab(true),
            Action::PrevTab => self.cycle_tab(false),
            Action::CloseTab => self.close_tab(),
            Action::Expand => {
                if let Some(t) = &mut self.thread {
                    match t.toggle_expanded() {
                        Ok(_) => {}
                        Err(msg) => self.info(msg),
                    }
                }
            }
            Action::ShowHidden => self.toggle_show_hidden(),
            Action::Copy => self.copy(false),
            Action::CopyLink => self.copy(true),
            Action::Sort => {
                self.catalog_sort = self.catalog_sort.next();
                self.catalog_list.state.select(Some(0));
                // Remembered for this board.
                let key = self.board_key();
                self.store.board_prefs.entry(key).or_default().sort = (self.catalog_sort != Sort::Bump).then_some(self.catalog_sort);
                self.save_now();
                self.info(format!("Sorted by {}", self.catalog_sort.label()));
            }
            Action::Compact => self.cycle_layout(),
            Action::Download => self.download(false),
            Action::DownloadThread => self.download(true),
            Action::Archive => match self.archive_offer.take() {
                Some(key) => {
                    self.open_key(key);
                    self.return_to = None;
                }
                None => self.info("Nothing to open in an archive"),
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

    /// Moving in the catalog grid: j/k by rows, h/l by columns (h in the first column goes
    /// back, as in lists). Returns whether the key was one of those.
    fn on_grid_key(&mut self, code: KeyCode) -> bool {
        let cols = self.grid_cols as isize;
        let len = self.visible_catalog().len();
        let cur = self.catalog_list.state.selected().unwrap_or(0) as isize;
        let delta = match code {
            KeyCode::Char('j') | KeyCode::Down => cols,
            KeyCode::Char('k') | KeyCode::Up => -cols,
            KeyCode::Char('l') | KeyCode::Right if (cur + 1) % cols != 0 => 1,
            KeyCode::Char('l') | KeyCode::Right => 0,
            KeyCode::Char('h') | KeyCode::Left if cur % cols != 0 => -1,
            _ => return false,
        };
        // Down from the last full row goes to the last thread.
        if delta == cols && cur + cols >= len as isize && cur / cols < (len as isize - 1) / cols {
            self.catalog_list.state.select(Some(len - 1));
        } else if (0..len as isize).contains(&(cur + delta)) {
            self.catalog_list.state.select(Some((cur + delta) as usize));
        }
        true
    }

    fn on_thread_key(&mut self, code: KeyCode, ctrl: bool) {
        let Some(t) = &mut self.thread else { return };
        let half = (t.viewport / 2).max(1) as isize;
        match code {
            KeyCode::Char('j') | KeyCode::Down => t.select_entry(t.entry() + 1),
            KeyCode::Char('k') | KeyCode::Up => t.select_entry(t.entry().saturating_sub(1)),
            KeyCode::Char('J') => t.scroll_lines(1),
            KeyCode::Char('K') => t.scroll_lines(-1),
            KeyCode::Char('d') if ctrl => t.scroll_lines(half),
            KeyCode::Char('u') if ctrl => t.scroll_lines(-half),
            KeyCode::PageDown | KeyCode::Char(' ') => t.scroll_lines(half * 2 - 1),
            KeyCode::PageUp => t.scroll_lines(-(half * 2 - 1)),
            KeyCode::Char('g') | KeyCode::Home => t.select_entry(0),
            KeyCode::Char('G') | KeyCode::End => t.select_entry(usize::MAX),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                let quotes = t.current().map(|p| p.quotes.clone()).unwrap_or_default();
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
                self.info(msg);
            }
            Action::NextMatch | Action::PrevMatch => {
                if let Some(i) = t.next_match(action == Action::NextMatch) {
                    t.select(i);
                    let k = t.matches.iter().position(|&m| m == i).unwrap_or(0);
                    let msg = format!("Match {}/{} for \"{}\"", k + 1, t.matches.len(), t.search);
                    self.info(msg);
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
                self.info(msg);
            }
            Action::Replies => match t.backlinks[t.selected].first() {
                Some(&no) => {
                    t.jump_to(no);
                }
                None => self.info("No replies to this post"),
            },
            Action::JumpBack => {
                if let Some(i) = t.jumps.pop() {
                    t.select(i);
                } else if let Some((site, board, no, post)) = self.trail.pop() {
                    // Back to the thread we came from by a cross-thread link.
                    self.switch_site(site);
                    self.open_thread_at(board, no, Some(post), false);
                }
            }
            Action::Unread => match (0..t.posts.len()).find(|&i| t.is_new(i)) {
                Some(i) => {
                    t.jumps.push(t.selected);
                    t.select(i);
                }
                None => self.info("No unread posts"),
            },
            Action::OpenFile => match t.current().and_then(|p| p.files.first()).cloned() {
                Some(f) => self.open_file(&f),
                None => self.info("Post has no file"),
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
                let msg = match t.next_match(true) {
                    Some(i) => {
                        t.select(i);
                        format!("{} posts match \"{}\" (n/N to move)", t.matches.len(), t.search)
                    }
                    None if t.search.is_empty() => return,
                    None => format!("No posts match \"{}\"", t.search),
                };
                self.info(msg);
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
        let Some(p) = t.current() else { return };
        let posts: Vec<usize> = p.quotes.iter().filter_map(|q| t.index.get(q).copied()).collect();
        let elsewhere: Vec<u64> = p.links.iter().filter_map(|l| l.post).filter(|n| !t.index.contains_key(n)).collect();
        if posts.is_empty() && elsewhere.is_empty() {
            self.info("Post quotes nothing");
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
        match self.outgoing_link() {
            Some(link) => self.follow(link),
            None => self.info("Post quotes nothing in this thread"),
        }
    }

    /// The selected post's first link out of this thread (to a post rather than a board, if
    /// it has both).
    fn outgoing_link(&self) -> Option<Link> {
        let (Some(t), Some(board)) = (&self.thread, &self.board) else { return None };
        let here = |l: &Link| l.board.as_ref().is_none_or(|b| *b == board.uri);
        let leaves = |l: &&Link| {
            let in_thread = here(l)
                && (l.thread == Some(t.no) || (l.thread.is_none() && l.post.is_some_and(|p| t.index.contains_key(&p))));
            !in_thread
        };
        let links = &t.current()?.links;
        links.iter().filter(leaves).find(|l| l.post.is_some()).or_else(|| links.iter().find(leaves)).cloned()
    }

    /// Go where a quote link leads: a thread (remembered for `u`), a board, or a post whose
    /// thread the engine is asked for.
    fn follow(&mut self, link: Link) {
        let Some(board) = self.board.clone() else { return };
        let target = match &link.board {
            Some(uri) if *uri != board.uri => self.find_board(uri),
            _ => board.clone(),
        };
        match (link.thread, link.post) {
            (Some(no), post) => {
                if let Some(t) = self.thread.as_ref().filter(|_| self.view == View::Thread) {
                    self.trail.push((self.site, board, t.no, t.current().map_or(t.no, |p| p.no)));
                }
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
        self.from_catalog = false;
        self.gallery = None;
        if announce {
            self.info(format!("Opening /{}/{no} (u goes back)", board.uri));
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
            KeyCode::Esc | KeyCode::Char('q' | 'v') => {
                // Back in the gallery, on the file last viewed.
                if let Some(g) = &mut self.gallery {
                    g.state.select(Some(v.index));
                }
                self.viewer = None;
            }
            KeyCode::Char('h' | 'k') | KeyCode::Left | KeyCode::Up => v.index = (v.index + n - 1) % n,
            // Space pauses an animated GIF; otherwise it's the next file.
            KeyCode::Char(' ') if self.images.toggle_pause(&v.files[v.index].url) => {
                let paused = self.images.is_paused(&v.files[v.index].url);
                self.info(if paused { "Paused (space plays)" } else { "Playing" });
            }
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
            View::Thread => self.thread.as_ref().and_then(ThreadView::current),
            _ => None,
        }
    }

    fn open_viewer(&mut self) {
        if !self.images.enabled() {
            self.info("Images are off (images = \"off\" in the config); i opens the file");
            return;
        }
        let link = self.selected_link();
        match self.selected_post().map(|p| p.files.clone()) {
            Some(files) if !files.is_empty() => self.viewer = Some(Viewer { files, index: 0, link }),
            _ => self.info("Post has no file"),
        }
    }

    /// Open a file externally: videos in mpv when it's installed, everything else in the default opener.
    pub fn open_file(&mut self, f: &Attachment) {
        if f.is_video() && on_path("mpv") {
            let mut cmd = std::process::Command::new("mpv");
            cmd.arg(&f.url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
            match cmd.spawn() {
                Ok(_) => self.info(format!("Playing {} in mpv", f.filename)),
                Err(e) => self.error(format!("Couldn't start mpv: {e}")),
            }
        } else {
            self.open_url(&f.url);
        }
    }

    fn remove_entry(&mut self) {
        if self.view == View::Sites {
            match self.selected_site_row() {
                Some(SiteRow::Favorite(i)) => {
                    let f = self.favorites.remove(i);
                    self.save_favorites(&format!("/{}/ off the favorites", f.board));
                }
                Some(SiteRow::Recent(i)) => {
                    self.store.recent_boards.remove(i);
                    self.save_now();
                    let len = self.visible_sites().len();
                    self.site_list.clamp(len);
                }
                Some(SiteRow::Site(i)) => self.toggle_site_hidden(i),
                _ => {}
            }
            return;
        }
        let Some(i) = self.selected_index() else { return };
        match self.view {
            View::Watched => {
                let w = self.store.watched.remove(i);
                self.info(format!("Stopped watching thread {}", w.key.no));
            }
            View::History => {
                self.store.history.remove(i);
            }
            _ => return,
        }
        self.save_now();
        if let Some((p, len)) = self.picker() {
            p.clamp(len);
        }
    }

    /// Index of the selected item in the current list's underlying data (not for Sites).
    fn selected_index(&self) -> Option<usize> {
        match self.view {
            View::Sites | View::Thread | View::Settings | View::Search => None,
            View::Boards => self.board_list.state.selected().and_then(|i| self.visible_boards().get(i).copied()),
            View::Catalog => self.catalog_list.state.selected().and_then(|i| self.visible_catalog().get(i).copied()),
            View::Watched => self.watched_list.state.selected().and_then(|i| self.visible_watched().get(i).copied()),
            View::History => self.history_list.state.selected().and_then(|i| self.visible_history().get(i).copied()),
        }
    }

    fn enter(&mut self) {
        if self.view == View::Settings {
            self.activate_setting();
            return;
        }
        if self.view == View::Search {
            self.open_search_hit();
            return;
        }
        if self.view == View::Sites {
            match self.site_list.state.selected().and_then(|i| self.visible_sites().get(i).copied()) {
                Some(SiteRow::Watched) => self.view = View::Watched,
                Some(SiteRow::History) => self.view = View::History,
                Some(SiteRow::Favorite(i)) => self.open_favorite(i),
                Some(SiteRow::Recent(i)) => {
                    if let Some(b) = self.recent_board(i) {
                        self.open_board(&b);
                    }
                }
                Some(SiteRow::Site(i)) => self.enter_site(i),
                Some(SiteRow::HiddenSites) => {
                    self.show_hidden_sites = !self.show_hidden_sites;
                    let len = self.visible_sites().len();
                    self.site_list.clamp(len);
                }
                None => {}
            }
            return;
        }
        let Some(i) = self.selected_index() else { return };
        match self.view {
            View::Sites | View::Thread | View::Settings | View::Search => {}
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
                // On an overboard the thread lives on its own board.
                if let Some(uri) = self.catalog[i].board.clone().filter(|b| *b != self.catalog_board) {
                    self.board = Some(self.find_board(&uri));
                }
                self.thread = None;
                self.return_to = None;
                self.from_catalog = true;
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
        self.gallery = None;
        self.view = match self.view {
            View::Sites | View::Boards | View::Watched | View::History => View::Sites,
            View::Settings => self.settings.back.take().unwrap_or(View::Sites),
            View::Search => self.close_search(),
            View::Catalog => View::Boards,
            View::Thread => self.return_to.take().unwrap_or(View::Catalog),
        };
        self.trail.clear();
        // Back to the catalog the thread was opened from (an overboard's, maybe).
        if self.view == View::Catalog && std::mem::take(&mut self.from_catalog) {
            self.board = self.catalog_of.clone();
        }
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
            self.req = 0;
            self.loading = None;
        }
    }

    fn refresh(&mut self) {
        match self.view {
            View::Sites | View::Watched | View::History | View::Settings => {}
            View::Search => {
                if let Some(s) = &mut self.search {
                    s.hits.clear();
                    s.pages = 0;
                }
                self.load_search_page();
            }
            View::Boards => self.load_boards(),
            View::Catalog => self.load_catalog(),
            View::Thread => {
                if let Some(no) = self.thread.as_ref().map(|t| t.no) {
                    self.load_thread(no);
                }
            }
        }
    }

    /// Copy the selected thing's text (or file URL in the viewer), or with `link` its URL.
    fn copy(&mut self, link: bool) {
        let what = if let Some(v) = &self.viewer {
            let post_link = v.link.clone().or_else(|| self.gallery_link(v.index));
            if link { post_link.map(|l| ("link", l)) } else { Some(("file URL", v.files[v.index].url.clone())) }
        } else if link {
            self.selected_link().map(|l| ("link", l))
        } else {
            let saved = |key: &ThreadKey, subject: &str| {
                let url = self.thread_link(key, None).unwrap_or_default();
                format!("{subject}\n{url}").trim().to_string()
            };
            match self.view {
                View::Thread => self.thread.as_ref().and_then(ThreadView::current).map(|p| ("text", copy_text(p, false))),
                View::Catalog => self.selected_post().map(|p| ("text", copy_text(p, true))),
                View::Watched => self.selected_index().map(|i| ("text", saved(&self.store.watched[i].key, &self.store.watched[i].subject))),
                View::History => self.selected_index().map(|i| ("text", saved(&self.store.history[i].key, &self.store.history[i].subject))),
                _ => None,
            }
        };
        let Some((what, text)) = what else { return };
        if text.is_empty() {
            self.info("Nothing to copy: the post has no text");
            return;
        }
        self.copy_text(what, text);
    }

    pub fn copy_text(&mut self, what: &str, text: String) {
        match crate::clipboard::copy(&text) {
            Ok(()) if what == "text" => self.info(format!("Copied {} characters", text.chars().count())),
            Ok(()) => self.info(format!("Copied {what}: {text}")),
            Err(e) => self.error(format!("Couldn't copy: {e:#}")),
        }
        self.copied = Some(text);
    }

    /// A link to a thread (and post) on its site.
    fn thread_link(&self, key: &ThreadKey, post: Option<u64>) -> Option<String> {
        let site = self.sites.iter().find(|s| s.cfg.name == key.site)?;
        Some(match post {
            Some(p) if p != key.no => site.backend.post_url(&key.board, key.no, p),
            _ => site.backend.thread_url(&key.board, key.no),
        })
    }

    /// The link to what's selected: a post in a thread, a catalog thread, a saved thread, a board.
    fn selected_link(&self) -> Option<String> {
        let backend = &self.current_site().backend;
        match (self.view, &self.board) {
            (View::Watched, _) => self.selected_index().and_then(|i| self.thread_link(&self.store.watched[i].key, None)),
            (View::History, _) => self.selected_index().and_then(|i| self.thread_link(&self.store.history[i].key, None)),
            (View::Boards, _) => self.selected_index().map(|i| backend.board_url(&self.boards()[i].uri)),
            (View::Catalog, Some(b)) => self.selected_index().map(|i| {
                let p = &self.catalog[i];
                backend.thread_url(p.board.as_deref().unwrap_or(&b.uri), p.no)
            }),
            (View::Thread, Some(b)) => self.thread.as_ref().and_then(|t| self.thread_link(&self.key(&b.uri, t.no), Some(t.current()?.no))),
            _ => None,
        }
    }

    fn open_in_browser(&mut self) {
        if let Some(url) = self.selected_link() {
            self.open_url(&url);
        }
    }

    pub fn open_url(&mut self, url: &str) {
        self.opened = Some(url.to_string());
        if cfg!(test) {
            return;
        }
        match open::that_detached(url) {
            Ok(()) => self.info(format!("Opened {url}")),
            Err(e) => self.error(format!("Couldn't open {url}: {e}")),
        }
    }
}

/// A post's text as plain text, line by line (spoilers included); with `subject`, the
/// subject first.
pub fn copy_text(p: &Post, subject: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(s) = p.subject.as_ref().filter(|_| subject) {
        lines.push(s.clone());
    }
    lines.extend(p.body.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>().trim_end().to_string()));
    lines.join("\n").trim().to_string()
}

/// A thread's subject for lists: its subject, or the start of the OP's text.
pub fn thread_subject(posts: &[Post]) -> String {
    let Some(op) = posts.first() else { return String::new() };
    op.subject.clone().unwrap_or_else(|| op.plain_text().chars().take(80).collect())
}

pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// The indices of `n` items whose `text` contains the filter (any case). The text is only
/// made while there's a filter.
fn filtered(filter: &str, n: usize, text: impl Fn(usize) -> String) -> Vec<usize> {
    let needle = filter.to_lowercase();
    (0..n).filter(|&i| needle.is_empty() || text(i).to_lowercase().contains(&needle)).collect()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use ratatui::text::Line;

    use super::*;
    use crate::keys::ACTIONS;

    /// An app over the default config; nothing here touches the network.
    pub fn test_app() -> App {
        let cfg: Config = toml::from_str(crate::config::DEFAULT_CONFIG).unwrap();
        let mut app = App::new(cfg, KeyMap::default(), None, Store::default());
        app.config_path = None;
        app
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
        let block: Rc<[Line]> = vec![Line::raw(""); 4].into();
        t.layout = Some(ThreadLayout { width: 40, blocks: vec![block.clone(), block.clone(), block], starts: vec![0, 4, 8, 12], thumbs: vec![] });
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
        app.info("hi");
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
            mine: Vec::new(),
            replies: 0,
            general: None,
            at_limit: false,
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
    fn overboard_threads_open_on_their_board_and_back_returns() {
        // A local site that refuses connections: nothing leaves the machine.
        let cfg: Config = toml::from_str("[[site]]\nname = \"t\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:9\"\nboards = [\"ob\"]").unwrap();
        let mut app = App::new(cfg, KeyMap::default(), None, Store::default());
        app.config_path = None;
        app.board = Some(Board { uri: "ob".into(), title: "Overboard".into(), nsfw: None });
        app.load_catalog();
        app.catalog = vec![Post { no: 5, board: Some("tech".into()), ..Default::default() }];
        app.catalog_list.state.select(Some(0));
        app.view = View::Catalog;
        app.enter();
        assert_eq!((app.view, app.board.as_ref().unwrap().uri.as_str()), (View::Thread, "tech"));
        assert_eq!(app.pending_thread, 5);
        app.back();
        assert_eq!((app.view, app.board.as_ref().unwrap().uri.as_str()), (View::Catalog, "ob"));
        // The overboard's catalog is still there; nothing was reloaded.
        assert_eq!(app.catalog.len(), 1);
    }

    #[test]
    fn key_editor_rebinds_saves_and_refuses_clashes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app();
        app.config_path = Some(dir.path().join("config.toml"));
        let press = |app: &mut App, code| app.on_key(KeyEvent::from(code));
        app.open_settings();
        app.settings_list.state.select(Some(settings::items().iter().position(|&i| i == settings::Item::Keys).unwrap()));
        app.activate_setting();
        // Move to `watch` and rebind it to W.
        let rows = settings::key_rows();
        let watch = rows.iter().position(|r| *r == Ok(ACTIONS.iter().position(|e| e.0 == Action::Watch).unwrap())).unwrap();
        while app.settings.popup.as_ref().is_some_and(|p| !matches!(p, SettingsPopup::Keys { list, .. } if list.selected() == Some(watch))) {
            press(&mut app, KeyCode::Down);
        }
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('W'));
        assert_eq!(app.keys.label(Action::Watch), "W");
        // `a` adds a second key.
        press(&mut app, KeyCode::Char('a'));
        app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT));
        assert_eq!(app.keys.label(Action::Watch), "W, alt-w");
        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(text.contains(r#"watch = ["W", "alt-w"]"#), "{text}");
        // A key another command uses in the same view is refused.
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(app.keys.label(Action::Watch), "W, alt-w");
        assert!(app.status.as_ref().is_some_and(|s| s.error && s.text.contains("'v'")), "{:?}", app.status);
        // x resets to the default, which removes the entry.
        press(&mut app, KeyCode::Char('x'));
        assert!(app.keys.is_default(Action::Watch));
        let c: Config = toml::from_str(&std::fs::read_to_string(dir.path().join("config.toml")).unwrap()).unwrap();
        assert!(!c.keys.contains_key("watch"));
        // The new keys work at once.
        app.settings.popup = None;
        app.view = View::Catalog;
        assert_eq!(app.keys.action(app.scope(), &KeyEvent::from(KeyCode::Char('w'))), Some(Action::Watch));
    }

    #[test]
    fn copies_text_and_links() {
        let mut app = test_app();
        let html = "<a href=\"#p1\" class=\"quotelink\">&gt;&gt;1</a><br><span class=\"quote\">&gt;green</span><br><s>secret</s> text";
        let parsed = crate::markup::parse_html(html, crate::markup::Flavor::Fourchan);
        let post = |no, body: Vec<Line<'static>>| Post { no, subject: Some("Subj".into()), body, ..Default::default() };
        app.board = Some(Board { uri: "g".into(), title: String::new(), nsfw: None });
        app.thread = Some(ThreadView::new("g".into(), 1, vec![post(1, vec![Line::raw("op")]), post(2, parsed.lines)]));
        app.thread.as_mut().unwrap().selected = 1;
        app.view = View::Thread;
        app.act(Action::Copy);
        assert_eq!(app.copied.as_deref(), Some(">>1\n>green\nsecret text"));
        assert_eq!(app.status.as_ref().unwrap().text, "Copied 22 characters");
        app.act(Action::CopyLink);
        assert_eq!(app.copied.as_deref(), Some("https://boards.4chan.org/g/thread/1#p2"));
        // The viewer copies the file's URL, or the post's link.
        app.thread.as_mut().unwrap().posts[1].files = vec![Attachment { url: "https://i.4cdn.org/g/1.png".into(), ..Default::default() }];
        app.images = crate::images::Images::offline();
        app.act(Action::View);
        app.on_key(KeyEvent::from(KeyCode::Char('y')));
        assert_eq!(app.copied.as_deref(), Some("https://i.4cdn.org/g/1.png"));
        app.on_key(KeyEvent::from(KeyCode::Char('Y')));
        assert_eq!(app.copied.as_deref(), Some("https://boards.4chan.org/g/thread/1#p2"));
        app.viewer = None;
        // Catalog: subject and text; a thread link.
        app.catalog = vec![post(7, vec![Line::raw("hello")])];
        app.catalog_list.state.select(Some(0));
        app.view = View::Catalog;
        app.act(Action::Copy);
        assert_eq!(app.copied.as_deref(), Some("Subj\nhello"));
        app.act(Action::CopyLink);
        assert_eq!(app.copied.as_deref(), Some("https://boards.4chan.org/g/thread/7"));
    }

    /// Two sites on hosts that refuse connections: nothing leaves the machine. (Their own
    /// port, so the requests don't take rate-limit slots other tests' hosts need.)
    fn local_app() -> App {
        let cfg: Config = toml::from_str(
            "[[site]]\nname = \"a\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"x\", \"xy\"]\n\
             [[site]]\nname = \"b\"\nkind = \"vichan\"\nurl = \"http://localhost:3\"\nboards = [\"y\"]",
        )
        .unwrap();
        let mut app = App::new(cfg, KeyMap::default(), None, Store::default());
        app.config_path = None;
        app
    }

    #[test]
    fn goto_opens_places_and_u_comes_back() {
        let mut app = local_app();
        app.goto_str("b/y/5#6");
        assert_eq!((app.site, app.view, app.pending_thread, app.pending_post), (1, View::Thread, 5, Some(6)));
        assert_eq!(app.board.as_ref().unwrap().uri, "y");
        // Esc from there returns to where : was typed.
        assert_eq!(app.return_to, Some(View::Sites));
        // From a thread, `u` comes back across sites.
        app.thread = Some(ThreadView::new("y".into(), 5, vec![Post { no: 5, ..Default::default() }, Post { no: 6, ..Default::default() }]));
        app.thread.as_mut().unwrap().selected = 1;
        app.goto_str("http://127.0.0.1:3/x/res/3.html#4");
        assert_eq!((app.site, app.pending_thread, app.board.as_ref().unwrap().uri.as_str()), (0, 3, "x"));
        app.thread = None;
        app.act(Action::JumpBack);
        assert!(app.thread.is_none());
        // (JumpBack needs a loaded thread; simulate the arrival of thread 3.)
        app.thread = Some(ThreadView::new("x".into(), 3, vec![Post { no: 3, ..Default::default() }]));
        app.act(Action::JumpBack);
        assert_eq!((app.site, app.pending_thread, app.pending_post, app.board.as_ref().unwrap().uri.as_str()), (1, 5, Some(6), "y"));
        // A board opens its catalog; a site its boards.
        app.goto_str("a/xy");
        assert_eq!((app.site, app.view, app.board.as_ref().unwrap().uri.as_str()), (0, View::Catalog, "xy"));
        app.goto_str("b");
        assert_eq!((app.site, app.view), (1, View::Boards));
        // Errors are said, not acted on.
        app.goto_str("https://example.com/g/");
        assert!(app.status.as_ref().unwrap().error);
        assert_eq!(app.view, View::Boards);
    }

    #[test]
    fn goto_input_completes_and_takes_pastes() {
        let mut app = local_app();
        app.act(Action::Goto);
        app.paste("a/");
        app.on_key(KeyEvent::from(KeyCode::Tab));
        // x and xy: completes the common part and lists both.
        assert_eq!(app.goto.as_deref(), Some("a/x"));
        assert!(app.status.as_ref().unwrap().text.contains("xy"));
        app.on_key(KeyEvent::from(KeyCode::Char('y')));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!((app.goto.as_deref(), app.view, app.board.as_ref().unwrap().uri.as_str()), (None, View::Catalog, "xy"));
        // Site names complete with a slash.
        app.act(Action::Goto);
        app.on_key(KeyEvent::from(KeyCode::Char('b')));
        app.on_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.goto.as_deref(), Some("b/"));
        app.on_key(KeyEvent::from(KeyCode::Esc));
        // A paste with nothing being typed starts the input.
        app.paste("http://localhost:3/y/res/1.html\n");
        assert_eq!(app.goto.as_deref(), Some("http://localhost:3/y/res/1.html"));
    }

    #[test]
    fn links_panel_lists_and_opens() {
        let mut app = local_app();
        app.switch_site(0);
        app.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
        let html = r#"<a href="/x/res/1.html#1" class="quotelink">&gt;&gt;1</a> <a href="/xy/res/9.html#10">&gt;&gt;&gt;/xy/10</a> see https://example.com/a"#;
        let parsed = crate::markup::parse_html(html, crate::markup::Flavor::Vichan);
        let reply = Post { no: 2, body: parsed.lines, quotes: parsed.quotes, links: parsed.links, urls: parsed.urls, files: vec![Attachment { filename: "a.png".into(), url: "http://127.0.0.1:3/x/src/a.png".into(), ..Default::default() }], ..Default::default() };
        app.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }, reply]));
        app.thread.as_mut().unwrap().selected = 1;
        app.view = View::Thread;
        app.act(Action::Links);
        // The quote of a post in this thread isn't listed; the other board's is.
        let kinds: Vec<String> = app.links.as_ref().unwrap().items.iter().map(|i| match i {
            LinkItem::Quote(_, label) => label.clone(),
            LinkItem::Url(u) => u.clone(),
            LinkItem::File(f) => f.filename.clone(),
        }).collect();
        assert_eq!(kinds, [">>>/xy/10  (thread 9)", "https://example.com/a", "a.png"]);
        // y copies the selected link; enter on a web link opens it.
        app.on_key(KeyEvent::from(KeyCode::Down));
        app.on_key(KeyEvent::from(KeyCode::Char('y')));
        assert_eq!(app.copied.as_deref(), Some("https://example.com/a"));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.links.is_none());
        assert_eq!(app.opened.as_deref(), Some("https://example.com/a"));
        // Enter on the quote opens its thread, and `u` will come back.
        app.act(Action::Links);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!((app.board.as_ref().unwrap().uri.as_str(), app.pending_thread, app.pending_post), ("xy", 9, Some(10)));
        assert_eq!(app.trail.len(), 1);
        // A post without links says so.
        app.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }]));
        app.act(Action::Links);
        assert!(app.links.is_none() && app.status.as_ref().unwrap().text == "Post has no links");
    }

    #[test]
    fn filters_and_hiding() {
        let mut app = local_app();
        let cfg = "[[filter]]\npattern = \"(?i)spam\"\nlabel = \"spam\"\n[[filter]]\npattern = \"rust\"\naction = \"highlight\"";
        #[derive(serde::Deserialize)]
        struct C {
            filter: Vec<crate::filter::FilterConfig>,
        }
        app.filters = Filters::new(&toml::from_str::<C>(cfg).unwrap().filter).unwrap();
        app.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
        app.catalog_board = "x".into();
        let op = |no, subject: &str| Post { no, subject: Some(subject.into()), ..Default::default() };
        app.catalog = vec![op(1, "SPAM here"), op(2, "rust thread"), op(3, "other")];
        app.remark_catalog();
        app.view = View::Catalog;
        assert_eq!(app.visible_catalog(), [1, 2]);
        assert_eq!(app.catalog_marks[1].highlight.as_deref(), Some("rust"));
        // H hides by hand; the filter's own can't be unhidden by H.
        app.catalog_list.state.select(Some(1));
        app.act(Action::Hide);
        assert_eq!(app.visible_catalog(), [1]);
        assert!(app.store.hidden_on("a", "x").contains(&3));
        // Z shows them all, keeping the selection on the same thread.
        app.act(Action::ShowHidden);
        assert_eq!(app.visible_catalog(), [0, 1, 2]);
        assert_eq!(app.selected_index(), Some(1));
        app.catalog_list.state.select(Some(0));
        app.act(Action::Hide);
        assert!(app.status.as_ref().unwrap().text.contains("filter \"spam\""));
        app.catalog_list.state.select(Some(2));
        app.act(Action::Hide);
        assert!(!app.store.hidden_on("a", "x").contains(&3));
        app.act(Action::ShowHidden);
        // In a thread, hidden posts collapse (never the OP).
        let mut reply = op(11, "");
        reply.body = vec![Line::raw("buy spam")];
        app.set_thread(vec![op(10, "spam OP"), reply, op(12, "")]);
        app.view = View::Thread;
        let t = app.thread.as_ref().unwrap();
        assert!(!t.is_collapsed(0) && t.is_collapsed(1) && !t.is_collapsed(2));
        app.thread.as_mut().unwrap().selected = 2;
        app.act(Action::Hide);
        assert!(app.thread.as_ref().unwrap().is_collapsed(2));
        app.act(Action::ShowHidden);
        assert!(!app.thread.as_ref().unwrap().is_collapsed(1));
    }

    #[test]
    fn notifies_about_new_posts_and_replies_to_yours() {
        let mut app = local_app();
        let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
        let post = |no, quotes: Vec<u64>| Post { no, quotes, ..Default::default() };
        app.store.toggle_watch(key(1), "One".into(), 2, 5);
        app.store.toggle_watch(key(2), "Two".into(), 1, 20);
        app.store.watched_mut(&key(1)).unwrap().mine.push(5);
        // The first refresh of the session tells nothing.
        app.refreshed(key(1), Ok(vec![post(1, vec![]), post(5, vec![]), post(6, vec![5])]));
        app.flush_notes(Instant::now());
        assert!(app.notified.is_empty());
        assert_eq!(app.store.watched(&key(1)).unwrap().replies, 1);
        // Then: a reply to your post, and another post.
        app.refreshed(key(1), Ok(vec![post(1, vec![]), post(5, vec![]), post(6, vec![5]), post(7, vec![5]), post(8, vec![1])]));
        app.flush_notes(Instant::now());
        assert_eq!(app.notified, ["New reply to your post in /x/ One", "1 new post in /x/ One"]);
        assert_eq!(app.store.watched(&key(1)).unwrap().replies, 2);
        // Several threads at once make one notification.
        app.notified.clear();
        app.refreshed(key(2), Ok(vec![post(20, vec![])]));
        app.refreshed(key(1), Ok(vec![post(1, vec![]), post(9, vec![])]));
        app.refreshed(key(2), Ok(vec![post(20, vec![]), post(21, vec![])]));
        // ...once nothing is still refreshing.
        app.refreshing.insert(key(3));
        app.flush_notes(Instant::now());
        assert!(app.notified.is_empty());
        app.refreshing.clear();
        app.flush_notes(Instant::now());
        assert_eq!(app.notified, ["2 watched threads have new posts"]);
    }

    #[test]
    fn marking_posts_as_yours_watches_the_thread() {
        let mut app = local_app();
        app.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
        app.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }, Post { no: 2, ..Default::default() }]));
        app.thread.as_mut().unwrap().selected = 1;
        app.view = View::Thread;
        app.act(Action::Mine);
        let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
        assert_eq!(app.store.watched(&key).unwrap().mine, [2]);
        assert!(app.thread.as_ref().unwrap().mine.contains(&2));
        app.act(Action::Mine);
        assert!(app.store.watched(&key).unwrap().mine.is_empty());
    }

    #[test]
    fn catalogs_mark_new_threads_and_replies() {
        let mut app = local_app();
        app.clock = Clock { fixed: Some(1000) };
        app.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
        let op = |no, replies| Post { no, replies: Some(replies), ..Default::default() };
        app.load_catalog();
        app.handle(Msg::Catalog(app.req, Ok(vec![op(1, 3), op(2, 0)])));
        assert!(app.catalog_new.is_empty());
        // Thread 1 is opened with 3 replies.
        app.set_thread(vec![Post { no: 1, ..Default::default() }, Post { no: 5, ..Default::default() }, Post { no: 6, ..Default::default() }, Post { no: 7, ..Default::default() }]);
        app.load_catalog();
        app.handle(Msg::Catalog(app.req, Ok(vec![op(9, 0), op(1, 8), op(2, 1)])));
        assert_eq!(app.catalog_new, [9].into());
        assert_eq!(app.new_replies(&app.catalog[1]), Some(5));
        // Threads never opened don't count replies.
        assert_eq!(app.new_replies(&app.catalog[2]), None);
    }

    #[test]
    fn replies_expand_inline() {
        // 1 <- 2 <- 3, and 3 also quotes 1; 4 quotes 2; 5 and 6 quote each other.
        let post = |no, quotes: Vec<u64>| Post { no, quotes, ..Default::default() };
        let mut t = ThreadView::new("x".into(), 1, vec![post(1, vec![]), post(2, vec![1]), post(3, vec![2, 1]), post(4, vec![2]), post(5, vec![6]), post(6, vec![5])]);
        let shown = |t: &ThreadView| t.entries.iter().map(|e| (t.posts[e.post].no, e.depth)).collect::<Vec<_>>();
        assert_eq!(t.toggle_expanded(), Ok(true));
        assert_eq!(shown(&t)[..4], [(1, 0), (2, 1), (3, 1), (2, 0)]);
        // Expand 2 inside 1: its replies come one level deeper; the cursor moves through them.
        t.select_entry(1);
        assert_eq!((t.selected, t.toggle_expanded()), (1, Ok(true)));
        assert_eq!(shown(&t)[..6], [(1, 0), (2, 1), (3, 2), (4, 2), (3, 1), (2, 0)]);
        t.select_entry(2);
        assert_eq!((t.selected, t.entry()), (2, 2));
        // A post with no replies says so.
        assert_eq!(t.toggle_expanded(), Err("No replies to this post"));
        // Collapsing the top one removes everything under it; the selection stays on it.
        t.select_entry(0);
        assert_eq!(t.toggle_expanded(), Ok(false));
        assert_eq!(shown(&t), [(1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0)]);
        assert_eq!(t.entry(), 0);
        // Quote loops stop: 5 under 6 under 5 isn't shown again.
        t.select(4);
        t.toggle_expanded().unwrap();
        t.select_entry(5);
        t.toggle_expanded().unwrap();
        assert_eq!(shown(&t)[4..], [(5, 0), (6, 1), (6, 0)]);
        // Setting `selected` directly lands on the post's top-level entry.
        t.selected = 3;
        assert_eq!(t.entry(), 3);
    }

    #[test]
    fn expanded_replies_survive_a_refresh() {
        let mut app = local_app();
        app.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
        let post = |no, quotes: Vec<u64>| Post { no, quotes, ..Default::default() };
        app.set_thread(vec![post(1, vec![]), post(2, vec![1])]);
        app.view = View::Thread;
        app.act(Action::Expand);
        app.on_key(KeyEvent::from(KeyCode::Down));
        assert_eq!((app.thread.as_ref().unwrap().entry(), app.thread.as_ref().unwrap().selected), (1, 1));
        app.set_thread(vec![post(1, vec![]), post(2, vec![1]), post(3, vec![1])]);
        let t = app.thread.as_ref().unwrap();
        assert_eq!(t.entries.len(), 5);
        assert_eq!((t.entry(), t.entries[t.entry()].depth), (1, 1));
    }

    #[test]
    fn grid_moves_in_two_dimensions() {
        let mut app = test_app();
        app.catalog = (1..=7).map(|no| Post { no, ..Default::default() }).collect();
        app.view = View::Catalog;
        app.default_layout = CatalogLayout::Grid;
        app.grid_cols = 3;
        app.catalog_list.state.select(Some(0));
        let press = |app: &mut App, c| app.on_key(KeyEvent::from(KeyCode::Char(c)));
        let at = |app: &App| app.catalog_list.state.selected().unwrap();
        press(&mut app, 'j');
        assert_eq!(at(&app), 3);
        press(&mut app, 'l');
        press(&mut app, 'l');
        assert_eq!(at(&app), 5);
        // The end of a row stops; down from the last full row goes to the last thread.
        press(&mut app, 'l');
        assert_eq!(at(&app), 5);
        press(&mut app, 'j');
        assert_eq!(at(&app), 6);
        press(&mut app, 'k');
        assert_eq!(at(&app), 3);
        // h in the first column goes back, as in lists.
        press(&mut app, 'h');
        assert_eq!(app.view, View::Boards);
        // Clicks hit the right card.
        app.view = View::Catalog;
        app.hit = Some(Hit::Grid { area: Rect::new(2, 2, 66, 24), offset: 0, cols: 3, cell: (22, 12) });
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2 + 22 + 5, 2 + 12 + 3), Instant::now());
        assert_eq!(at(&app), 4);
        // c cycles this board's layout.
        app.act(Action::Compact);
        assert_eq!(app.layout(), CatalogLayout::Cards);
    }

    #[test]
    fn gallery_of_the_threads_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = local_app();
        app.download_dir = Some(dir.path().display().to_string());
        app.images = crate::images::Images::offline();
        app.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
        let file = |name: &str| Attachment { filename: name.into(), url: format!("http://127.0.0.1:3/x/src/{name}"), ..Default::default() };
        let post = |no, files: Vec<Attachment>| Post { no, files, ..Default::default() };
        app.thread = Some(ThreadView::new("x".into(), 1, vec![post(1, vec![file("a.png")]), post(2, vec![]), post(3, vec![file("b.jpg"), file("c.gif")])]));
        app.view = View::Thread;
        app.thread.as_mut().unwrap().selected = 1;
        app.act(Action::Gallery);
        // It starts at the selected post's file, or the next.
        assert_eq!(app.gallery.as_ref().unwrap().state.selected(), Some(1));
        app.gallery.as_mut().unwrap().cols = 2;
        let press = |app: &mut App, code| app.on_key(KeyEvent::from(code));
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.gallery.as_ref().unwrap().state.selected(), Some(2));
        // Enter views every file of the thread, from this one; esc comes back to the grid.
        press(&mut app, KeyCode::Enter);
        assert_eq!((app.viewer.as_ref().unwrap().files.len(), app.viewer.as_ref().unwrap().index), (3, 2));
        press(&mut app, KeyCode::Char('h'));
        press(&mut app, KeyCode::Char('Y'));
        assert_eq!(app.copied.as_deref(), Some("http://127.0.0.1:3/x/res/1.html#3"));
        press(&mut app, KeyCode::Esc);
        assert!(app.viewer.is_none());
        assert_eq!(app.gallery.as_ref().unwrap().state.selected(), Some(1));
        // d saves the one file.
        press(&mut app, KeyCode::Char('d'));
        assert_eq!((app.downloads.total, app.downloads.running), (1, 1));
        // Esc: back to the thread, on the file's post.
        press(&mut app, KeyCode::Esc);
        assert!(app.gallery.is_none());
        assert_eq!(app.thread.as_ref().unwrap().selected, 2);
    }

    #[test]
    fn archive_search_and_back() {
        let cfg: Config = toml::from_str(
            "[[site]]\nname = \"chan\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"g\"]\narchive = \"arch\"\n\
             [[site]]\nname = \"arch\"\nkind = \"foolfuuka\"\nurl = \"http://localhost:3\"\nboards = [\"g\"]",
        )
        .unwrap();
        let mut app = App::new(cfg, KeyMap::default(), None, Store::default());
        app.config_path = None;
        app.board = Some(Board { uri: "g".into(), title: String::new(), nsfw: None });
        app.view = View::Catalog;
        app.act(Action::ArchiveSearch);
        for c in "borrow".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!((app.view, app.site), (View::Search, 1));
        let path = format!("{}/tests/fixtures/foolfuuka_search.json", env!("CARGO_MANIFEST_DIR"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        app.handle(Msg::Search(app.req, 1, crate::backend::foolfuuka::parse_search(&v)));
        assert_eq!(app.search.as_ref().unwrap().hits.len(), 4);
        // Going down to the end asks for the next page.
        let req = app.req;
        for _ in 0..4 {
            app.on_key(KeyEvent::from(KeyCode::Down));
        }
        assert_eq!(app.req, req + 1);
        app.handle(Msg::Search(app.req, 2, Err(anyhow::anyhow!("You're searching too fast."))));
        assert!(app.status.as_ref().is_some_and(|s| s.error && s.text.contains("too fast")));
        // Enter: the thread, on the archive, with the post selected.
        app.search_list.state.select(Some(1));
        app.enter();
        assert_eq!((app.view, app.pending_thread, app.pending_post), (View::Thread, 109912686, Some(109914413)));
        app.back();
        assert_eq!(app.view, View::Search);
        app.back();
        assert_eq!((app.view, app.site, app.search.is_none()), (View::Catalog, 0, true));
        // Sites without an archive say so.
        app.sites[0].cfg.archive = None;
        app.act(Action::ArchiveSearch);
        assert!(app.search_input.is_none() && app.status.as_ref().unwrap().text.contains("no archive"));
    }

    #[test]
    fn reverse_image_search() {
        let mut app = local_app();
        app.images = crate::images::Images::offline();
        let file = |name: &str, thumb| Attachment { filename: name.into(), url: format!("https://i.example/{name}"), thumb, ..Default::default() };
        let files = vec![file("a.png", None), file("b.webm", Some("https://i.example/bs.jpg".into())), file("c.pdf", None)];
        app.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, files, ..Default::default() }]));
        app.view = View::Thread;
        app.act(Action::ImageSearch);
        // The image itself, and the video's thumbnail; a file with neither is left out.
        let rows = &app.image_search_panel.as_ref().unwrap().rows;
        assert_eq!(rows.len(), 2 + 2 * 4);
        assert_eq!(rows[0], Err("a.png".into()));
        assert_eq!(rows[6], Ok(("https://i.example/bs.jpg".into(), 0)));
        app.on_key(KeyEvent::from(KeyCode::Down));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.opened.as_deref(), Some("https://lens.google.com/uploadbyurl?url=https%3A%2F%2Fi.example%2Fa.png"));
        assert!(app.image_search_panel.is_none());
        // In the viewer: the file shown.
        app.act(Action::View);
        app.on_key(KeyEvent::from(KeyCode::Char('R')));
        assert_eq!(app.image_search_panel.as_ref().unwrap().rows.len(), 4);
        app.on_key(KeyEvent::from(KeyCode::Char('y')));
        assert_eq!(app.copied.as_deref(), Some("https://saucenao.com/search.php?url=https%3A%2F%2Fi.example%2Fa.png"));
        // Engines can be configured.
        let cfg: Config = toml::from_str("[[image_search]]\nname = \"Mine\"\nurl = \"https://s.example/?u={url}\"\n[[site]]\nname = \"a\"\nkind = \"4chan\"").unwrap();
        let app = App::new(cfg, KeyMap::default(), None, Store::default());
        assert_eq!(app.image_search.len(), 1);
        assert_eq!(app.image_search[0].link("http://x/y z"), "https://s.example/?u=http%3A%2F%2Fx%2Fy%20z");
    }

    #[test]
    fn sessions_save_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = local_app();
        app.store = Store::load(Some(dir.path().to_path_buf())).0;
        // A thread on the second site, with a post selected.
        app.goto_str("b/y/5");
        app.set_thread(vec![Post { no: 5, ..Default::default() }, Post { no: 6, ..Default::default() }]);
        app.thread.as_mut().unwrap().selected = 1;
        app.catalog_sort = Sort::Newest;
        app.save_session(None);
        let saved = app.store.load_session().unwrap();
        assert_eq!(saved.tabs[0], crate::store::Place {
            view: "thread".into(),
            site: "b".into(),
            board: Some("y".into()),
            thread: Some(5),
            selected: Some(6),
            sort: Some(Sort::Newest),
            filter: String::new(),
        });
        // The next run starts there.
        let mut next = local_app();
        next.store = Store::load(Some(dir.path().to_path_buf())).0;
        next.restore_session();
        assert_eq!((next.site, next.view, next.pending_thread, next.pending_post, next.catalog_sort), (1, View::Thread, 5, Some(6), Sort::Newest));
        // If the thread is gone, its catalog instead.
        next.handle(Msg::Thread(next.req, Err(anyhow::Error::new(http::HttpError::NotFound("x".into())))));
        assert_eq!(next.view, View::Catalog);
        // A catalog with its selected thread.
        next.handle(Msg::Catalog(next.req, Ok(vec![])));
        next.catalog = (1..4).map(|no| Post { no, ..Default::default() }).collect();
        next.catalog_list.state.select(Some(2));
        // (The board has no sort of its own, so its catalog is in bump order: index 2 is thread 3.)
        let place = next.place();
        assert_eq!((place.view.as_str(), place.selected), ("catalog", Some(3)));
        let mut third = local_app();
        third.go_to_place(&place);
        third.handle(Msg::Catalog(third.req, Ok((1..4).map(|no| Post { no, time: no as i64, ..Default::default() }).collect())));
        assert_eq!(third.selected_index().map(|i| third.catalog[i].no), Some(3));
    }

    #[test]
    fn tabs_keep_their_own_place_and_responses() {
        let mut app = local_app();
        // Tab 0 loads a catalog on site a.
        app.goto_str("a/x");
        let first_req = app.req;
        // Tab 1 opens a thread on site b while that's still loading.
        app.tabs.push(Tab::new(0));
        app.switch_tab(1);
        assert_eq!((app.view, app.thread.is_none()), (View::Sites, true));
        app.goto_str("b/y/5");
        assert_eq!((app.site, app.view, app.pending_thread), (1, View::Thread, 5));
        // Tab 0's catalog arrives: it goes to tab 0, not here.
        app.handle(Msg::Catalog(first_req, Ok(vec![Post { no: 1, ..Default::default() }])));
        assert!(app.catalog.is_empty());
        app.handle(Msg::Thread(app.req, Ok(vec![Post { no: 5, ..Default::default() }])));
        assert_eq!(app.thread.as_ref().unwrap().no, 5);
        app.switch_tab(0);
        assert_eq!((app.site, app.view, app.catalog.len(), app.loading.is_none()), (0, View::Catalog, 1, true));
        assert!(app.thread.is_none());
        // A response for a tab that's gone is dropped.
        let stale = app.tabs[1].req_for_tests();
        app.tabs.truncate(1);
        app.handle(Msg::Thread(stale, Ok(vec![Post { no: 9, ..Default::default() }])));
        assert!(app.thread.is_none());
    }

    #[test]
    fn new_tabs_switching_closing_and_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = local_app();
        app.store = Store::load(Some(dir.path().to_path_buf())).0;
        app.goto_str("a/x");
        app.handle(Msg::Catalog(app.req, Ok((1..=3).map(|no| Post { no, ..Default::default() }).collect())));
        app.catalog_list.state.select(Some(1));
        let key = |c| KeyEvent::from(KeyCode::Char(c));
        // T: thread 2 in a new tab after this one.
        app.on_key(key('T'));
        assert_eq!((app.tabs.len(), app.active, app.view, app.pending_thread), (2, 1, View::Thread, 2));
        assert_eq!(app.tab_label(0), "/x/");
        // tab / shift-tab switch; each tab keeps its place.
        app.on_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!((app.active, app.view, app.catalog.len()), (0, View::Catalog, 3));
        app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!((app.active, app.view), (1, View::Thread));
        // The session has both.
        app.save_session(None);
        let s = app.store.load_session().unwrap();
        assert_eq!((s.tabs.len(), s.active, s.tabs[1].thread), (2, 1, Some(2)));
        // ctrl-w closes; the last tab stays.
        app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!((app.tabs.len(), app.active, app.view), (1, 0, View::Catalog));
        app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(app.tabs.len(), 1);
        // At most MAX_TABS.
        for _ in 0..MAX_TABS + 2 {
            app.switch_tab(0);
            app.new_tab();
        }
        assert_eq!(app.tabs.len(), MAX_TABS);
        // A new run restores the tabs.
        let mut next = local_app();
        next.store = Store::load(Some(dir.path().to_path_buf())).0;
        next.restore_session();
        assert_eq!((next.tabs.len(), next.active, next.view, next.pending_thread), (2, 1, View::Thread, 2));
        next.switch_tab(0);
        assert_eq!((next.view, next.board.as_ref().unwrap().uri.as_str()), (View::Catalog, "x"));
    }

    #[test]
    fn favorite_boards_on_the_home_screen() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = local_app();
        app.config_path = Some(dir.path().join("config.toml"));
        // * in Boards on the selected board, and in a catalog on its board.
        app.switch_site(1);
        app.view = View::Boards;
        app.board_list.state.select(Some(0));
        app.act(Action::Favorite);
        app.goto_str("a/xy");
        app.act(Action::Favorite);
        assert_eq!(app.favorites.iter().map(BoardRef::key).collect::<Vec<_>>(), ["b/y", "a/xy"]);
        let c: Config = toml::from_str(&std::fs::read_to_string(dir.path().join("config.toml")).unwrap()).unwrap();
        assert_eq!(c.favorites, ["b/y", "a/xy"]);
        // They're on the home screen after Watched and History; 2 opens the second.
        app.view = View::Sites;
        assert_eq!(app.visible_sites()[2..4], [SiteRow::Favorite(0), SiteRow::Favorite(1)]);
        app.on_key(KeyEvent::from(KeyCode::Char('1')));
        assert_eq!((app.site, app.view, app.board.as_ref().unwrap().uri.as_str()), (1, View::Catalog, "y"));
        app.view = View::Sites;
        app.on_key(KeyEvent::from(KeyCode::Char('2')));
        assert_eq!((app.site, app.board.as_ref().unwrap().uri.as_str()), (0, "xy"));
        // x on a favorite row takes it off; * again on the board does too.
        app.view = View::Sites;
        app.site_list.state.select(Some(2));
        app.act(Action::Remove);
        assert_eq!(app.favorites.len(), 1);
        app.goto_str("a/xy");
        app.act(Action::Favorite);
        assert!(app.favorites.is_empty());
        app.view = View::Sites;
        app.on_key(KeyEvent::from(KeyCode::Char('3')));
        assert!(app.status.as_ref().unwrap().text.contains("No favorites yet"));
    }

    #[test]
    fn recent_boards_on_the_home_screen() {
        let mut app = local_app();
        for board in ["a/x", "b/y", "a/xy"] {
            app.goto_str(board);
            app.handle(Msg::Catalog(app.req, Ok(vec![])));
        }
        assert_eq!(app.store.recent_boards, ["a/xy", "b/y", "a/x"]);
        // Favorites aren't repeated as recent.
        app.favorites.push(BoardRef::parse("b/y").unwrap());
        app.view = View::Sites;
        let rows = app.visible_sites();
        assert_eq!(rows[2..5], [SiteRow::Favorite(0), SiteRow::Recent(0), SiteRow::Recent(2)]);
        // Enter opens; x forgets it.
        app.site_list.state.select(Some(4));
        app.enter();
        assert_eq!((app.view, app.board.as_ref().unwrap().uri.as_str()), (View::Catalog, "x"));
        app.view = View::Sites;
        app.site_list.state.select(Some(3));
        app.act(Action::Remove);
        assert_eq!(app.store.recent_boards, ["b/y", "a/x"]);
    }

    #[test]
    fn hiding_sites_from_the_home_screen() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = local_app();
        app.config_path = Some(dir.path().join("config.toml"));
        let sites = |app: &App| app.visible_sites().into_iter().filter(|r| matches!(r, SiteRow::Site(_) | SiteRow::HiddenSites)).collect::<Vec<_>>();
        assert_eq!(sites(&app), [SiteRow::Site(0), SiteRow::Site(1)]);
        app.site_list.state.select(Some(2));
        app.act(Action::Remove);
        assert_eq!(sites(&app), [SiteRow::Site(1), SiteRow::HiddenSites]);
        let c: Config = toml::from_str(&std::fs::read_to_string(dir.path().join("config.toml")).unwrap()).unwrap();
        assert_eq!(c.hidden_sites, ["a"]);
        // The last row shows them; x on one brings it back.
        app.site_list.state.select(Some(3));
        app.enter();
        assert_eq!(sites(&app), [SiteRow::Site(0), SiteRow::Site(1), SiteRow::HiddenSites]);
        app.site_list.state.select(Some(2));
        app.act(Action::Remove);
        assert_eq!(sites(&app), [SiteRow::Site(0), SiteRow::Site(1)]);
        assert!(app.hidden_sites.is_empty());
    }

    #[test]
    fn boards_remember_their_sort_and_layout() {
        let mut app = local_app();
        app.goto_str("a/x");
        app.act(Action::Sort);
        app.act(Action::Compact);
        assert_eq!((app.catalog_sort, app.layout()), (Sort::Replies, CatalogLayout::Compact));
        // Another board: the defaults.
        app.goto_str("a/xy");
        assert_eq!((app.catalog_sort, app.layout()), (Sort::Bump, CatalogLayout::Cards));
        // Back on the first: its own again (also after a restart, from the data directory).
        app.goto_str("a/x");
        assert_eq!((app.catalog_sort, app.layout()), (Sort::Replies, CatalogLayout::Compact));
        assert_eq!(app.store.board_prefs["a/x"], crate::store::BoardPrefs { sort: Some(Sort::Replies), layout: Some(CatalogLayout::Compact) });
        // The default (Settings) applies to boards without their own.
        app.default_layout = CatalogLayout::Grid;
        app.goto_str("a/xy");
        assert_eq!(app.layout(), CatalogLayout::Grid);
    }

    #[test]
    fn following_a_general() {
        let mut app = local_app();
        app.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
        let op = |no, subject: &str| Post { no, subject: Some(subject.into()), replies: Some(10), ..Default::default() };
        app.set_thread(vec![op(10, "/lmg/ - Local Models General #5"), Post { no: 11, ..Default::default() }]);
        app.view = View::Thread;
        // F follows it (watching it too).
        app.act(Action::Follow);
        let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
        assert_eq!(app.store.watched(&key(10)).unwrap().general.as_deref(), Some("/lmg/"));
        // Alive and not full: nothing to look for.
        let now = Instant::now();
        app.check_generals(now);
        assert!(app.generals_searching.is_empty());
        // At the bump limit (a refresh says so): its board is searched, once.
        let mut full = op(10, "/lmg/ - Local Models General #5");
        full.bumplimit = true;
        app.view = View::Sites;
        app.refreshed(key(10), Ok(vec![full, Post { no: 11, ..Default::default() }]));
        assert!(app.store.watched(&key(10)).unwrap().at_limit);
        app.check_generals(now);
        app.check_generals(now);
        assert_eq!(app.generals_searching.len(), 1);
        // No new thread yet: tried again only after a while.
        app.general_catalog(key(10), Ok(vec![op(10, "/lmg/ - Local Models General #5"), op(12, "/ldg/ - Local Diffusion")]));
        app.check_generals(now + Duration::from_secs(60));
        assert!(app.generals_searching.is_empty());
        // The next one appears: it's watched and followed; the old one (still going) is kept.
        app.check_generals(now + Duration::from_secs(601));
        app.general_catalog(key(10), Ok(vec![op(9, "/lmg/ old"), op(13, "/lmg/ - Local Models General #6"), op(14, "/ldg/")]));
        assert_eq!(app.store.watched(&key(13)).unwrap().general.as_deref(), Some("/lmg/"));
        assert_eq!(app.store.watched(&key(10)).unwrap().general, None);
        assert!(app.notified.last().unwrap().starts_with("New /lmg/ thread on /x/"));
        // When the followed thread dies, it's replaced in Watched.
        app.store.watched_mut(&key(13)).unwrap().dead = true;
        app.check_generals(now + Duration::from_secs(1200));
        app.general_catalog(key(13), Ok(vec![op(20, "/lmg/ - Local Models General #7")]));
        assert!(app.store.watched(&key(13)).is_none());
        assert!(app.store.watched(&key(20)).is_some());
        // F again stops following.
        app.view = View::Watched;
        let i = app.store.watched.iter().position(|w| w.key == key(20)).unwrap();
        app.watched_list.state.select(Some(i));
        app.act(Action::Follow);
        assert_eq!(app.store.watched(&key(20)).unwrap().general, None);
    }

    #[test]
    fn background_changes_are_saved_together() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = local_app();
        app.store = Store::load(Some(dir.path().to_path_buf())).0;
        app.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
        app.clock = Clock { fixed: Some(1000) };
        let posts = |n: u64| (1..=n).map(|no| Post { no, ..Default::default() }).collect::<Vec<_>>();
        app.set_thread(posts(2));
        // Opening a thread is a visit, written a little later rather than at once.
        assert!(!dir.path().join("history.json").exists());
        app.saved_at -= SAVE_EVERY;
        app.poll();
        assert!(dir.path().join("history.json").exists());
        // A refresh without new posts isn't a new visit; one with new posts is.
        app.clock = Clock { fixed: Some(2000) };
        app.set_thread(posts(2));
        assert_eq!(app.store.history[0].opened, 1000);
        app.set_thread(posts(3));
        assert_eq!((app.store.history[0].opened, app.store.history[0].last_seen), (2000, 3));
        // What the user does is saved at once.
        app.view = View::Thread;
        app.act(Action::Watch);
        let watched = std::fs::read_to_string(dir.path().join("watched.json")).unwrap();
        assert!(watched.contains("\"no\": 1"), "{watched}");
    }

    #[test]
    fn tab_switches_keep_layouts_and_theme_changes_redo_them_all() {
        let mut app = local_app();
        let layout = || Some(ThreadLayout { width: 40, blocks: Vec::new(), starts: vec![0, 0], thumbs: Vec::new() });
        app.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }]));
        app.thread.as_mut().unwrap().layout = layout();
        app.tabs.push(Tab::new(0));
        app.switch_tab(1);
        app.thread = Some(ThreadView::new("x".into(), 2, vec![Post { no: 2, ..Default::default() }]));
        app.thread.as_mut().unwrap().layout = layout();
        // Switching (and handling another tab's response) doesn't throw layouts away.
        app.switch_tab(0);
        app.switch_tab(1);
        assert!(app.thread.as_ref().unwrap().layout.is_some());
        // A theme change does, in every tab.
        app.set_theme(crate::theme::theme());
        assert!(app.thread.as_ref().unwrap().layout.is_none());
        app.switch_tab(0);
        assert!(app.thread.as_ref().unwrap().layout.is_none());
    }

    #[test]
    fn status_messages_expire() {
        let mut app = test_app();
        let t0 = Instant::now();
        app.info("No unread posts");
        app.expire_status(t0);
        app.expire_status(t0 + Duration::from_millis(1500));
        assert!(app.status.is_some());
        app.expire_status(t0 + Duration::from_secs(2));
        assert!(app.status.is_none());

        // Errors stay longer, and a new message restarts the timer.
        app.error("Rate limited");
        app.expire_status(t0);
        app.expire_status(t0 + Duration::from_secs(4));
        assert!(app.status.is_some());
        app.error("Thread was deleted or archived");
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
        // A frame is ~16ms; even unoptimized (and on a busy machine), filtering should take a
        // fraction of it.
        assert!(per_call < Duration::from_millis(25), "filtering took {per_call:?}");
    }
}
