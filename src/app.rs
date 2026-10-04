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

/// The popup open, taken if it's of this kind (any other stays).
macro_rules! take_popup {
    ($app:expr, $kind:ident) => {
        match $app.popup.take() {
            Some(Popup::$kind(x)) => Some(x),
            other => {
                $app.popup = other;
                None
            }
        }
    };
}

mod filters;
mod focus;
mod gallery;
mod input;
mod generals;
mod loading;
mod thread_view;
mod goto;
mod home;
mod links;
mod saved;
mod saving;
mod board_images;
mod search;
mod session;
mod tabs;
mod settings;
mod sites;
pub use filters::{AddFilter, Candidate, EDIT_ROWS, EditRow, Reach, problem as filters_problem, with_text as filters_with_text};
pub use focus::{HintTarget, HintTo, Hints, Menu, MenuItem};
pub use gallery::Gallery;
pub use home::BoardRef;
pub use links::{ImageSearchPanel, LinkItem, LinksPanel};
pub use saving::{Confirm, Saving};
pub use search::{SavedSearch, Search};
pub use saving::Downloads;
pub use thread_view::*;
pub use sites::{Adding, BoardsUpdate, MySites, origin as site_origin};
pub use tabs::{MAX_TABS, Offline, Tab, TabPopup};
pub use settings::{SettingsPopup, SECTIONS as SETTING_SECTIONS, key_rows, rows as setting_rows, tilde};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Sites,
    Boards,
    Catalog,
    Thread,
    Watched,
    History,
    /// Saved copies of threads (watched and exported ones), readable offline.
    Saved,
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
    Saved,
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
    /// An empty filter, with the first row selected.
    fn top() -> Self {
        Self { state: ListState::default().with_selected(Some(0)), filter: String::new() }
    }

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


/// Wall clock for timestamps and "3h ago", and the monotonic clock for deadlines. Tests fix
/// the first (and format in UTC) so snapshots don't depend on when or where they run; the
/// fuzzer runs both on its own time.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clock {
    pub fixed: Option<i64>,
    pub instant: Option<Instant>,
}

impl Clock {
    pub fn now(&self) -> i64 {
        self.fixed.unwrap_or_else(|| chrono::Utc::now().timestamp())
    }

    pub fn instant(&self) -> Instant {
        self.instant.unwrap_or_else(Instant::now)
    }
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

/// A popup over the whole screen. One at a time: each opens from a key or the menu, with
/// nothing else open.
pub enum Popup {
    /// Settings' own: a theme, colors, keys, filters, …
    Settings(SettingsPopup),
    /// What can be done with what's selected (`.`).
    Menu(Menu),
    /// Labels on what's on screen (`f`).
    Hints(Hints),
    /// A big save asking first.
    Confirm(saving::Confirm),
    /// Adding a site.
    Adding(sites::Adding),
    /// `X`: a filter like the selected post.
    AddFilter(AddFilter),
    /// Choosing a reverse image search.
    ImageSearch(ImageSearchPanel),
    /// The key help, scrolled this far.
    Help(u16),
}

/// Popup with the posts the selected post quotes.
pub struct Preview {
    /// Indices of quoted posts in this thread.
    pub posts: Vec<usize>,
    /// Quoted post numbers that aren't in this thread.
    pub elsewhere: Vec<u64>,
    pub scroll: u16,
}

/// Full-screen viewer over a post's files, or a thread's.
pub struct Viewer {
    pub files: Vec<Attachment>,
    pub index: usize,
    /// Link to the post the files are from (one post's).
    pub link: Option<String>,
    /// The post each file is from, when they're a thread's (else empty).
    pub posts: Vec<u64>,
    /// How far it's zoomed in, and where.
    pub crop: crate::images::Crop,
    /// How much of the image is on screen (thousandths of its width and height), as last
    /// drawn: what moving around steps by.
    pub shown: Option<(u16, u16)>,
}

impl Viewer {
    pub fn new(files: Vec<Attachment>, index: usize, link: Option<String>) -> Self {
        Viewer { files, index, link, posts: Vec::new(), crop: crate::images::Crop::FIT, shown: None }
    }
}

enum Msg {
    /// The request was answered from the cache without hitting the network.
    Cached(u64, Duration),
    Boards(u64, usize, Result<Vec<Board>>),
    /// The pages of a board list loaded so far; more are coming.
    BoardsPartial(u64, usize, Vec<Board>),
    Catalog(u64, Result<Vec<Post>>),
    CatalogPartial(u64, Vec<Post>),
    Thread(u64, Result<Vec<Post>>),
    /// The last copy kept of the catalog or thread being fetched, and when it was fetched.
    CachedCatalog(u64, Vec<Post>, i64),
    CachedThread(u64, Vec<Post>, i64),
    /// A page of archive search results.
    Search(u64, u32, Result<crate::backend::SearchPage>),
    /// The thread a quoted post is in: (board, post, thread).
    Found(u64, Board, u64, Result<Option<u64>>),
    /// What background work found, to apply on the UI thread (see `Later`).
    Done(Box<dyn FnOnce(&mut App) + Send>),
    Input(Event),
    /// Something else (a loaded image) needs a redraw.
    Wake,
}

/// Sends what background work found back to the UI thread, to apply there in the order sent.
#[derive(Clone)]
struct Later(Sender<Msg>);

impl Later {
    /// Run `apply` on the UI thread. False once the app is gone (the work can stop).
    fn run(&self, apply: impl FnOnce(&mut App) + Send + 'static) -> bool {
        self.0.send(Msg::Done(Box::new(apply))).is_ok()
    }
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
            | Msg::CachedCatalog(id, ..)
            | Msg::CachedThread(id, ..)
            | Msg::Thread(id, _)
            | Msg::Search(id, ..)
            | Msg::Found(id, ..) => Some(*id),
            _ => None,
        }
    }
}

pub struct App {
    pub sites: Vec<Site>,
    pub site_list: Picker,
    /// Favorite boards (from the config), and board titles for the home screen.
    pub favorites: Vec<BoardRef>,
    pub home_titles: HashMap<String, String>,
    /// Sites left off the home screen (from the config), and whether they're shown anyway.
    pub hidden_sites: std::collections::BTreeSet<String>,
    pub show_hidden_sites: bool,
    /// The catalog layout for boards without their own (`catalog_layout` in the config).
    pub default_layout: CatalogLayout,
    /// Columns of the catalog grid as last drawn (0: not a grid).
    pub grid_cols: usize,
    /// The popup over the screen, if any: one at a time.
    pub popup: Option<Popup>,
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
    pub saved_list: Picker,
    /// The saved copy `x` was pressed on once: a second `x` removes it.
    pub saved_confirm: Option<ThreadKey>,
    /// The search of saved threads that's wanted; a running one stops when it changes.
    saved_search: Arc<std::sync::atomic::AtomicU64>,
    /// Sites taken out of the config in Settings: off the home screen until ck restarts.
    pub removed_sites: std::collections::BTreeSet<String>,
    pub store: Store,
    refresh_thread: Duration,
    refresh_watched: Duration,
    watched_checked: HashMap<ThreadKey, Instant>,
    /// Background refreshes in flight.
    pub refreshing: HashSet<ThreadKey>,
    /// The newest post seen in each watched thread by a refresh this session; notifications
    /// are for posts past it.
    notified_max: HashMap<ThreadKey, u64>,
    /// Followed generals: when each one's board was last searched, and searches running.
    generals_checked: HashMap<ThreadKey, Instant>,
    generals_searching: HashSet<ThreadKey>,
    /// When each (site, board) catalog was last fetched for them.
    general_boards: HashMap<(String, String), Instant>,
    /// Notifications waiting to be sent together, and since when.
    notes: Vec<Note>,
    notes_since: Option<Instant>,
    pub notify_mode: crate::notify::NotifyMode,
    pub notify_command: Option<Vec<String>>,
    /// Notifications sent (the last few), for the record.
    pub notified: Vec<String>,
    /// Sites whose saved board list is being refreshed in the background, and when each was
    /// last tried (a failed one isn't retried within `MIN_REFETCH`).
    boards_refreshing: HashSet<usize>,
    boards_tried: HashMap<usize, Instant>,
    pub filters: Filters,
    /// The last copies of catalogs and threads, to open them at once (none in tests).
    pub pages: Option<crate::pages::Pages>,
    /// `scroll_margin`: where the selected post sits while reading (a fraction of the screen).
    pub scroll_margin: f32,
    /// The config's `[[filter]]` tables, as last read or written (`filters` is made from them).
    pub filter_cfgs: Vec<crate::filter::FilterConfig>,
    /// `hidden_words`: posts with one are hidden everywhere.
    pub hidden_words: Vec<String>,
    /// The filter just added (and where): `u` as the next key takes it back.
    pub filter_undo: Option<filters::Undo>,
    /// Show hidden threads and posts (dimmed) instead of leaving them out.
    pub show_hidden: bool,
    /// True while typing into the filter.
    pub filtering: bool,
    pub status: Option<Status>,
    /// The status message as last seen by `expire_status`, and when it appeared.
    status_since: Option<(String, Instant)>,
    pub images: Images,
    /// Reverse image search engines (`R`), and the panel choosing one.
    pub image_search: Vec<crate::config::ImageSearch>,
    /// Save where you are and start there next time.
    pub restore_session: bool,
    /// Reading the end of a thread, new posts come into view (`follow_new_posts`).
    pub follow_new_posts: bool,
    /// Images on boards the site marks NSFW (`nsfw_images`).
    pub nsfw_images: crate::config::NsfwImages,
    /// NSFW boards of sites whose board list isn't loaded, from their saved lists; and the
    /// sites asked for theirs, once, to know.
    nsfw_saved: HashMap<usize, HashSet<String>>,
    nsfw_asked: HashSet<usize>,
    /// The data directory has changes to write, and when it was last written.
    save_pending: bool,
    saved_at: Instant,
    /// The session as last saved, and when that was checked.
    session_saved: (Option<crate::store::Session>, Instant),
    /// The archive search query being typed.
    pub search_input: Option<String>,
    /// True while typing a thread search.
    pub searching: bool,
    /// What's typed after `:`, while it's being typed.
    pub goto: Option<String>,
    pub keys: KeyMap,
    /// The last text copied to the clipboard, and the last URL opened.
    pub copied: Option<String>,
    pub opened: Option<String>,
    /// The config file that settings are saved to (tests point it elsewhere).
    pub config_path: Option<std::path::PathBuf>,
    pub clock: Clock,
    pub downloads: Downloads,
    pub(crate) download_dir: Option<String>,
    /// Set by the UI every frame.
    pub hit: Option<Hit>,
    /// Where each tab's chip was drawn, for clicks.
    pub tab_chips: Vec<(Rect, usize)>,
    /// Last left click: when, and the list index or thread post it hit.
    last_click: Option<(Instant, usize)>,
    pub tick: usize,
    pub quit: bool,
    /// The active tab's place; the other tabs (the active one's slot holds nothing useful),
    /// and which is active.
    pub tab: Tab,
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// The last request id given out (ids are unique across tabs).
    next_id: u64,
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
        let mut store = store;
        store.saved_max = cfg.saved_max_mb.saturating_mul(1024 * 1024);
        let sites = cfg
            .sites
            .into_iter()
            .map(|cfg| Site { backend: backend::build(&cfg), cfg, boards: None })
            .collect();
        let (tx, rx) = channel();
        let mut app = Self {
            sites,
            site_list: Picker::top(),
            favorites: cfg.favorites.iter().filter_map(|f| BoardRef::parse(f)).collect(),
            home_titles: HashMap::new(),
            hidden_sites: cfg.hidden_sites.iter().cloned().collect(),
            show_hidden_sites: false,
            grid_cols: 0,
            default_layout: store.settings.catalog_layout.unwrap_or(match store.settings.compact_catalog {
                Some(true) => CatalogLayout::Compact,
                Some(false) => CatalogLayout::Cards,
                None => layout,
            }),
            settings_list: Picker::top(),
            theme_name,
            themes,
            color_mode: cfg.color,
            truecolor: cfg.color.truecolor(),
            images_mode: cfg.images,
            watched_list: Picker::top(),
            history_list: Picker::top(),
            saved_list: Picker::top(),
            saved_confirm: None,
            saved_search: Arc::default(),
            removed_sites: Default::default(),
            store,
            refresh_thread,
            refresh_watched,
            watched_checked: HashMap::new(),
            refreshing: HashSet::new(),
            notified_max: HashMap::new(),
            generals_checked: HashMap::new(),
            generals_searching: HashSet::new(),
            general_boards: HashMap::new(),
            notes: Vec::new(),
            notes_since: None,
            notify_mode: cfg.notify,
            notify_command: cfg.notify_command.clone(),
            notified: Vec::new(),
            boards_refreshing: HashSet::new(),
            boards_tried: HashMap::new(),
            filters: Filters::new(&cfg.filters).and_then(|f| f.with_words(&cfg.hidden_words)).unwrap_or_default(),
            hidden_words: cfg.hidden_words.clone(),
            filter_cfgs: cfg.filters.clone(),
            scroll_margin: if cfg.scroll_margin.is_finite() { cfg.scroll_margin.clamp(0.0, 0.5) } else { 0.3 },
            pages: crate::pages::Pages::default_dir()
                .filter(|_| !cfg!(test) && cfg.page_cache_mb > 0)
                .map(|d| crate::pages::Pages::new(d, cfg.page_cache_mb.saturating_mul(1024 * 1024))),
            filter_undo: None,
            show_hidden: false,
            filtering: false,
            status: None,
            status_since: None,
            popup: None,
            images: Images::new(picker, {
                let tx = tx.clone();
                Arc::new(move || {
                    let _ = tx.send(Msg::Wake);
                })
            }, DiskCache::default_dir().map(|d| DiskCache::new(d, crate::disk_cache::BUDGET))),
            image_search: if cfg.image_search.is_empty() { crate::config::ImageSearch::defaults() } else { cfg.image_search.clone() },
            restore_session: cfg.restore_session,
            follow_new_posts: cfg.follow_new_posts,
            nsfw_images: cfg.nsfw_images,
            nsfw_saved: HashMap::new(),
            nsfw_asked: HashSet::new(),
            save_pending: false,
            saved_at: Instant::now(),
            session_saved: (None, Instant::now()),
            search_input: None,
            searching: false,
            goto: None,
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
            tab: Tab::new(0, Instant::now()),
            tabs: vec![Tab::new(0, Instant::now())],
            active: 0,
            next_id: 0,
            tx,
            rx,
        };
        app.load_home_titles();
        app
    }

    // ----- visible (filtered) items -----

    pub fn visible_sites(&self) -> Vec<SiteRow> {
        let rows: Vec<SiteRow> = [SiteRow::Watched, SiteRow::History, SiteRow::Saved]
            .into_iter()
            .chain((0..self.favorites.len()).map(SiteRow::Favorite))
            .chain(self.recent_rows().into_iter().map(SiteRow::Recent))
            .chain(
                (0..self.sites.len())
                    .filter(|&i| !self.removed_sites.contains(&self.sites[i].cfg.name))
                    .filter(|&i| self.show_hidden_sites || !self.is_site_hidden(i))
                    .map(SiteRow::Site),
            )
            .chain((!self.hidden_sites.is_empty()).then_some(SiteRow::HiddenSites))
            .collect();
        let name = |k: usize| match &rows[k] {
            SiteRow::Watched => "Watched".to_string(),
            SiteRow::History => "History".to_string(),
            SiteRow::Saved => "Saved".to_string(),
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

    pub fn visible_saved(&self) -> Vec<usize> {
        let s = &self.store.saved;
        filtered(&self.saved_list.filter, s.len(), |i| format!("{} {} {} {}", s[i].key.site, s[i].key.board, s[i].key.no, s[i].subject))
    }

    pub fn visible_history(&self) -> Vec<usize> {
        let h = &self.store.history;
        filtered(&self.history_list.filter, h.len(), |i| format!("{} {} {} {}", h[i].key.site, h[i].key.board, h[i].key.no, h[i].subject))
    }

    pub fn boards(&self) -> &[Board] {
        self.sites[self.tab.site].boards.as_deref().unwrap_or(&[])
    }

    pub fn visible_boards(&self) -> Vec<usize> {
        let b = self.boards();
        filtered(&self.tab.board_list.filter, b.len(), |i| format!("{} {}", b[i].uri, b[i].title))
    }

    pub fn visible_catalog(&self) -> Vec<usize> {
        let needle = self.tab.catalog_list.filter.to_lowercase();
        let shown = |i: usize| self.show_hidden || self.tab.catalog_marks.get(i).is_none_or(|m| m.hidden.is_none());
        let mut v: Vec<usize> =
            (0..self.tab.catalog.len()).filter(|&i| shown(i) && self.tab.catalog[i].search_text().contains(&needle)).collect();
        let c = &self.tab.catalog;
        match self.tab.catalog_sort {
            Sort::Bump => {}
            Sort::Replies => v.sort_by_key(|&i| std::cmp::Reverse(c[i].replies.unwrap_or(0))),
            Sort::Newest => v.sort_by_key(|&i| std::cmp::Reverse((c[i].time, c[i].no))),
            Sort::Oldest => v.sort_by_key(|&i| (c[i].time, c[i].no)),
        }
        v
    }

    /// What's on screen, broadly: when it changes, the screen is painted whole.
    pub fn screen(&self) -> (View, usize, bool, bool) {
        (self.tab.view, self.active, self.tab.viewer().is_some(), self.tab.gallery.is_some())
    }

    /// Keep the current list's selection on a row that exists.
    fn clamp_list(&mut self) {
        if let Some((p, len)) = self.picker() {
            p.clamp(len);
        }
    }

    fn picker(&mut self) -> Option<(&mut Picker, usize)> {
        Some(match self.tab.view {
            View::Sites => (self.visible_sites().len(), &mut self.site_list),
            View::Boards => (self.visible_boards().len(), &mut self.tab.board_list),
            View::Catalog => (self.visible_catalog().len(), &mut self.tab.catalog_list),
            View::Watched => (self.visible_watched().len(), &mut self.watched_list),
            View::History => (self.visible_history().len(), &mut self.history_list),
            View::Saved => (self.visible_saved().len(), &mut self.saved_list),
            View::Settings => (settings::items().len(), &mut self.settings_list),
            View::Search => (self.tab.search.as_ref().map_or(0, |s| s.hits.len()), &mut self.tab.search_list),
            View::Thread => return None,
        })
        .map(|(len, p)| (p, len))
    }

    /// To send what background work finds back to this thread.
    fn later(&self) -> Later {
        Later(self.tx.clone())
    }

    pub fn current_site(&self) -> &Site {
        &self.sites[self.tab.site]
    }

    // ----- background loading -----

    /// Block until something happens (input, a finished request, a loaded image) or
    /// `timeout` passes, then handle everything pending. Returns whether anything arrived.
    pub fn wait(&mut self, timeout: Duration) -> bool {
        let got = self.rx.recv_timeout(timeout).map(|msg| self.handle(msg)).is_ok();
        self.poll();
        got
    }

    /// Handle everything pending without blocking. Queued input is all handled before the
    /// next draw, so held-down keys don't build up a lag.
    pub fn poll(&mut self) {
        self.images.poll();
        for e in self.store.settle() {
            self.error(e);
        }
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg);
        }
        self.background();
        self.flush_notes(self.clock.instant());
        self.save_session(Some(self.clock.instant()));
        if self.save_pending && self.clock.instant().saturating_duration_since(self.saved_at) >= SAVE_EVERY {
            self.save_now();
        }
        self.expire_status(self.clock.instant());
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
        let animating = self.tab.loading.is_some() || !self.refreshing.is_empty() || self.downloads.running > 0 || self.tab.viewer().is_some();
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
        if self.tab.view == View::Thread && self.tab.thread.is_some() && self.tab.loading.is_none() && self.tab.offline.is_none() {
            at(self.tab.thread_checked + self.refresh_thread);
        }
        // At capacity, a finished refresh wakes the loop anyway (and due ones mustn't spin it).
        if self.refreshing.len() < MAX_REFRESHING {
            for w in self.store.watched.iter().filter(|w| !w.dead && !self.refreshing.contains(&w.key)) {
                at(self.watched_checked.get(&w.key).map_or(now, |t| *t + self.refresh_watched));
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
                crate::input_log::note(|| format!("read    {ev:?}"));
                if tx.send(Msg::Input(ev)).is_err() {
                    return;
                }
            }
        });
    }

    fn handle(&mut self, msg: Msg) {
        // A response to another request: another tab's is handled there, the rest are stale
        // (but a finished board list is worth keeping anyway).
        let other = msg.id().filter(|&id| id != self.tab.req);
        if let Some(i) = other.and_then(|id| self.tab_of(id)) {
            self.handle_in_tab(i, msg);
            return;
        }
        if other.is_some() && !matches!(msg, Msg::Boards(..)) {
            return;
        }
        if let Msg::Input(ev) = &msg {
            let acted = !matches!(ev, Event::Key(k) if k.kind != KeyEventKind::Press);
            crate::input_log::note(|| format!("handle  {ev:?}{}", if acted { "" } else { "  (not a press: ignored)" }));
        }
        match msg {
            Msg::Wake => {}
            Msg::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => self.on_key(key),
            Msg::Input(Event::Mouse(m)) => self.on_mouse(m, self.clock.instant()),
            Msg::Input(Event::Paste(text)) => self.paste(&text),
            Msg::Input(_) => {}
            Msg::Done(apply) => apply(self),
            Msg::Found(_, board, post, res) => {
                self.tab.loading = None;
                match res {
                    Ok(Some(no)) => {
                        if let Some(t) = &self.tab.thread {
                            self.tab.trail.push((self.tab.site, self.tab.board.clone().unwrap_or(board.clone()), t.no, t.current().map_or(t.no, |p| p.no)));
                        }
                        let in_settings = self.tab.view == View::Settings;
                        self.open_thread_at(board, no, Some(post));
                        // Found while the settings were open: the thread is behind them.
                        if in_settings {
                            self.tab.settings_back = Some(View::Thread);
                            self.tab.view = View::Settings;
                        }
                    }
                    Ok(None) => {
                        self.error(format!("Post {post} isn't in this thread, and this site can't say which thread it's in"));
                    }
                    Err(e) => self.error(e),
                }
            }
            Msg::Search(_, page, res) => {
                self.tab.loading = None;
                self.search_results(page, res);
            }
            Msg::Cached(_, age) => {
                if self.status.is_none() {
                    self.info(format!("Up to date (checked {}s ago)", age.as_secs()));
                }
            }
            Msg::Boards(_, site, res) => {
                if other.is_none() {
                    self.tab.loading = None;
                }
                match res {
                    Ok(b) => self.set_boards(site, b, true),
                    Err(e) => self.error(e),
                }
            }
            Msg::BoardsPartial(_, site, b) => self.set_boards(site, b, false),
            Msg::CachedCatalog(_, posts, fetched) => {
                // Only before anything fetched has arrived.
                if self.tab.catalog.is_empty() && self.tab.view == View::Catalog {
                    self.tab.catalog_cached = Some(tabs::Offline { saved: fetched, dead: false });
                    self.show_catalog(posts);
                    if let Some(no) = self.tab.pending_catalog
                        && let Some(i) = self.visible_catalog().iter().position(|&k| self.tab.catalog[k].no == no)
                    {
                        self.tab.catalog_list.state.select(Some(i));
                    }
                }
            }
            Msg::CachedThread(_, posts, fetched) => {
                if self.tab.thread.is_none() && self.tab.view == View::Thread && self.tab.offline.is_none() {
                    self.set_cached_thread(posts, fetched);
                }
            }
            Msg::CatalogPartial(_, posts) => {
                self.tab.catalog_cached = None;
                self.show_catalog(posts);
            }
            Msg::Catalog(_, res) => {
                self.tab.loading = None;
                match res {
                    Ok(posts) => {
                        self.tab.catalog_cached = None;
                        self.show_catalog(posts);
                        self.catalog_seen();
                        if let Some(no) = self.tab.pending_catalog.take()
                            && let Some(i) = self.visible_catalog().iter().position(|&k| self.tab.catalog[k].no == no)
                        {
                            self.tab.catalog_list.state.select(Some(i));
                        }
                        let len = self.visible_catalog().len();
                        self.tab.catalog_list.clamp(len);
                    }
                    Err(e) => self.error(e),
                }
            }
            Msg::Thread(_, res) => {
                self.tab.loading = None;
                self.tab.thread_checked = self.clock.instant();
                let restoring = std::mem::take(&mut self.tab.restoring);
                match res {
                    Ok(posts) => self.set_thread(posts),
                    // Last session's thread is gone: its catalog instead.
                    Err(e) if restoring && http::is_not_found(&e) => {
                        self.tab.view = View::Catalog;
                        self.load_catalog();
                        self.info("The thread you had open last time is gone (archived or deleted)");
                    }
                    Err(e) if http::is_not_found(&e) => {
                        let Some(key) = self.tab.board.as_ref().map(|b| self.key(&b.uri, self.tab.pending_thread)) else {
                            return;
                        };
                        if let Some(w) = self.store.watched_mut(&key) {
                            w.dead = true;
                            self.save();
                        }
                        self.store.saved_dead(&key);
                        self.thread_gone(&key);
                    }
                    Err(e) => self.error(e),
                }
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

    fn key(&self, board: &str, no: u64) -> ThreadKey {
        ThreadKey { site: self.current_site().cfg.name.clone(), board: board.to_string(), no }
    }

    /// Save soon: background changes (refreshes, visits) are written together, every few
    /// seconds and on quit.
    fn save(&mut self) {
        self.save_pending = true;
    }

    /// Finish the background writes (on quit), within a few seconds.
    pub fn flush_writes(&mut self) {
        for e in self.store.flush(Duration::from_secs(5)) {
            self.error(e);
        }
    }

    /// Save now (what the user just did).
    pub fn save_now(&mut self) {
        self.save_pending = false;
        self.saved_at = self.clock.instant();
        if let Err(e) = self.store.save() {
            self.error(format!("Couldn't save watched threads: {e:#}"));
        }
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
        let nos: Vec<u64> = self.tab.catalog.iter().map(|p| p.no).collect();
        let site = self.current_site().cfg.name.clone();
        let board = self.tab.catalog_board.clone();
        self.tab.catalog_new = self.store.catalog_seen(&site, &board, &nos, self.clock.now());
        self.store.board_opened(&site, &board);
        self.note_titles(self.tab.site);
        self.save();
    }

    /// Replies a catalog thread has gained since it was last opened.
    pub fn new_replies(&self, p: &Post) -> Option<u32> {
        let seen = self.store.replies_seen(&self.current_site().cfg.name, &self.board_of(p), p.no)?;
        p.replies.filter(|&r| r > seen).map(|r| r - seen)
    }

    pub fn remark_catalog(&mut self) {
        let marks = self.marks(&self.tab.catalog, |p| self.board_of(p));
        self.tab.catalog_marks = marks;
    }

    /// The board a catalog thread is on (overboards mix boards).
    pub fn board_of(&self, p: &Post) -> String {
        let loaded = Some(self.tab.catalog_board.clone()).filter(|b| !b.is_empty());
        p.board.clone().or(loaded).or_else(|| self.tab.board.as_ref().map(|b| b.uri.clone())).unwrap_or_default()
    }

    pub fn remark_thread(&mut self) {
        let Some(t) = &self.tab.thread else { return };
        let marks = self.marks(&t.posts, |_| t.board.clone());
        let mine = self.store.watched(&self.key(&t.board, t.no)).map(|w| w.mine.iter().copied().collect()).unwrap_or_default();
        let show = self.show_hidden;
        if let Some(t) = &mut self.tab.thread {
            t.marks = marks;
            t.mine = mine;
            t.show_hidden = show;
            t.layout = None;
            // A post just collapsed (hidden) has no parts to focus.
            if t.focus.as_ref().is_some_and(|f| !t.parts_of(t.entry()).contains(f)) {
                t.focus = None;
            }
        }
    }

    /// `H`: hide or unhide the selected thread (catalog) or post (thread) by hand.
    fn toggle_hidden(&mut self) {
        let (board, no, what, mark) = match self.tab.view {
            View::Catalog => {
                let Some(i) = self.selected_index() else { return };
                let p = &self.tab.catalog[i];
                (self.board_of(p), p.no, "thread", self.tab.catalog_marks.get(i).cloned())
            }
            View::Thread => {
                let Some(t) = &self.tab.thread else { return };
                let what = if t.selected == 0 { "thread" } else { "post" };
                (t.board.clone(), t.current().map_or(t.no, |p| p.no), what, t.marks.get(t.selected).cloned())
            }
            _ => return,
        };
        if let Some(label) = mark.and_then(|m| m.hidden).filter(|l| !l.is_empty()) {
            self.info(format!("Hidden by the filter \"{label}\"; Settings › Filters changes it"));
            return;
        }
        let site = self.current_site().cfg.name.clone();
        let hidden = self.store.toggle_hidden(&site, &board, no);
        self.save_now();
        let show = self.keys.key(Action::ShowHidden);
        self.info(if hidden { format!("Hid {what} {no} ({show} shows hidden ones)") } else { format!("Unhid {what} {no}") });
        self.remark_catalog();
        self.remark_thread();
        self.clamp_list();
    }

    /// `Z`: show hidden threads and posts (dimmed), or leave them out again.
    fn toggle_show_hidden(&mut self) {
        // Keep the same thread selected in the catalog.
        let keep = self.selected_index().filter(|_| self.tab.view == View::Catalog);
        self.show_hidden = !self.show_hidden;
        self.remark_thread();
        if let Some(i) = keep
            && let Some(pos) = self.visible_catalog().iter().position(|&v| v == i)
        {
            self.tab.catalog_list.state.select(Some(pos));
        }
        self.clamp_list();
        self.info(if self.show_hidden { "Showing hidden threads and posts" } else { "Leaving out hidden threads and posts" });
    }

    // ----- watched threads and auto-refresh -----

    // ----- downloads and settings -----

    /// The open catalog's board, as `site/board` (for its own sort and layout).
    fn board_key(&self) -> String {
        let board = Some(self.tab.catalog_board.clone()).filter(|b| !b.is_empty()).or_else(|| self.tab.board.as_ref().map(|b| b.uri.clone()));
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
        let board = self.tab.board.as_ref().map_or(String::new(), |b| format!(" for /{}/", b.uri));
        self.info(format!("Layout{board}: {} (the default is in Settings)", layout.as_str()));
    }

    /// The board's own sort, when its catalog opens.
    fn apply_board_sort(&mut self) {
        self.tab.catalog_sort = self.store.board_prefs.get(&self.board_key()).and_then(|p| p.sort).unwrap_or_default();
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

    /// A thread 404'd: say so, and offer the site's archive if it has one.
    ///
    /// With a saved copy, that comes first: a thread still on screen becomes its saved copy,
    /// and otherwise `enter` opens it.
    pub(crate) fn thread_gone(&mut self, key: &ThreadKey) {
        let archive = self.sites.iter().find(|s| s.cfg.name == key.site).and_then(|s| s.cfg.archive.clone());
        self.tab.archive_offer = archive.filter(|a| self.sites.iter().any(|s| s.cfg.name == *a)).map(|a| ThreadKey { site: a, board: key.board.clone(), no: key.no });
        let in_archive = self.tab.archive_offer.as_ref().map(|a| format!("{} opens it in {}", self.keys.key(Action::Archive), a.site));
        let saved = self.store.saved(key).map(|m| m.saved);
        let shown = self.tab.thread.as_ref().is_some_and(|t| t.no == key.no && t.board == key.board);
        let text = match saved {
            Some(at) if shown => {
                self.tab.cached = None;
                self.tab.offline = Some(tabs::Offline { saved: at, dead: true });
                format!("Thread was deleted or archived: this is its saved copy{}", in_archive.map(|a| format!(" ({a})")).unwrap_or_default())
            }
            Some(at) => {
                self.tab.saved_offer = Some(key.clone());
                let ago = crate::ui::ago(at, self.clock);
                format!("Thread was deleted or archived. A saved copy from {ago}: enter opens it{}", in_archive.map(|a| format!(", {a}")).unwrap_or_default())
            }
            None => {
                // A copy shown while it loaded stays, marked as gone.
                if let Some(c) = self.tab.cached.as_mut().filter(|_| shown) {
                    c.dead = true;
                }
                format!("Thread was deleted or archived{}", in_archive.map(|a| format!(". Press {a}")).unwrap_or_default())
            }
        };
        self.error(text);
    }

    fn toggle_watch(&mut self) {
        let (board, no, subject, posts, last_seen) = match (self.tab.view, &self.tab.board) {
            (View::Thread, _) => {
                let Some(t) = &self.tab.thread else { return };
                let max_no = t.posts.iter().map(|p| p.no).max().unwrap_or(0);
                (t.board.clone(), t.no, thread_subject(&t.posts), t.posts.len(), max_no)
            }
            (View::Catalog, Some(b)) => {
                let Some(i) = self.selected_index() else { return };
                let op = &self.tab.catalog[i];
                let posts = op.replies.map_or(1, |r| r as usize + 1);
                let board = op.board.clone().unwrap_or_else(|| b.uri.clone());
                // Unknown until the first refresh, which then counts nothing as unread.
                (board, op.no, thread_subject(std::slice::from_ref(op)), posts, 0)
            }
            _ => return,
        };
        let key = self.key(&board, no);
        let watching = self.store.toggle_watch(key.clone(), subject, posts, last_seen);
        if watching {
            self.keep_open_thread(&key);
        }
        // Refreshed soon, to learn where it's at, unless it was just fetched.
        let now = self.clock.instant();
        if self.watched_checked.get(&key).is_none_or(|t| now.saturating_duration_since(*t) >= http::MIN_REFETCH) {
            self.watched_checked.remove(&key);
        }
        self.info(if watching { format!("Watching thread {no}") } else { format!("Stopped watching thread {no}") });
        self.save_now();
    }

    /// `c`: the selected post's conversation alone, or the whole thread again.
    fn toggle_conversation(&mut self) {
        let Some(t) = self.tab.thread.as_mut().filter(|_| self.tab.view == View::Thread && self.tab.gallery.is_none()) else { return };
        if t.conversation.is_some() {
            t.leave_conversation();
            return;
        }
        match t.enter_conversation() {
            Ok(n) => {
                let capped = t.conversation.as_ref().is_some_and(|c| c.capped);
                let esc = if capped { format!(" (the nearest {n}; there are more)") } else { String::new() };
                self.info(format!("{}{esc}; esc or {} shows the whole thread", plural_posts(n), self.keys.key(Action::Conversation)));
            }
            Err(e) => self.info(e),
        }
    }

    /// Keep a copy of a thread's posts in the data directory (a watched thread's, or with
    /// `always`, any). Unchanged posts aren't written again.
    fn keep_copy(&mut self, key: &ThreadKey, posts: &[Post], always: bool) {
        if !always && self.store.watched(key).is_none() {
            return;
        }
        let url = self.thread_link(key, None).unwrap_or_default();
        if self.store.keep_thread(key, &thread_subject(posts), &url, posts, self.clock.now()) {
            self.save();
        }
    }

    /// A thread just watched: if it's the one open (and loaded), it's saved at once. A saved
    /// copy open is kept as it is (a copy saved under another number, when the site answered
    /// with another thread, is kept under this one too).
    fn keep_open_thread(&mut self, key: &ThreadKey) {
        if self.tab.view != View::Thread || (self.tab.offline.is_some() && self.store.saved(key).is_some()) {
            return;
        }
        let Some(t) = self.tab.thread.as_ref().filter(|t| self.key(&t.board, t.no) == *key) else { return };
        let posts = t.posts.clone();
        self.keep_copy(key, &posts, false);
        if self.tab.offline.is_some_and(|o| o.dead) {
            self.store.saved_dead(key);
        }
    }

    /// `m`: mark the selected post as yours (or not), to hear about replies to it. The
    /// thread is watched if it isn't.
    fn toggle_mine(&mut self) {
        let Some(t) = &self.tab.thread else { return };
        let key = self.key(&t.board, t.no);
        let Some(no) = t.current().map(|p| p.no) else { return };
        if self.store.watched(&key).is_none() {
            let max_no = t.posts.iter().map(|p| p.no).max().unwrap_or(0);
            self.store.toggle_watch(key.clone(), thread_subject(&t.posts), t.posts.len(), max_no);
            self.keep_open_thread(&key);
        }
        let Some(w) = self.store.watched_mut(&key) else { return };
        let mine = !w.mine.contains(&no);
        if mine {
            w.mine.push(no);
        } else {
            w.mine.retain(|&n| n != no);
        }
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
        self.tab.board = Some(board.unwrap_or(Board { uri: key.board, title: String::new(), nsfw: None }));
        self.tab.return_to = Some(self.tab.view);
        self.tab.from_catalog = false;
        self.tab.gallery = None;
        self.tab.thread = None;
        self.tab.view = View::Thread;
        self.load_thread(key.no);
    }

    /// Make `site` the current one, with its board list if it's saved (else fetched in the
    /// background), so going back to Boards shows it.
    fn switch_site(&mut self, site: usize) {
        if site != self.tab.site {
            self.tab.board_list = Picker::top();
            self.tab.site = site;
        }
        // Also for the site it's on already (the first, at startup: `ck 4chan` showed none).
        if self.sites[site].boards.is_none() {
            match self.known_boards(site) {
                Some(boards) => self.sites[site].boards = Some(boards),
                None => self.refresh_boards_in_background(site),
            }
        }
    }

    /// A site's boards as far as they're known without a request: loaded, from the config,
    /// or saved last time.
    fn known_boards(&self, site: usize) -> Option<Vec<Board>> {
        let s = self.sites.get(site)?;
        s.boards
            .clone()
            .or_else(|| s.cfg.boards.as_ref().map(|b| b.iter().map(backend::to_board).collect()))
            .or_else(|| self.store.load_boards(&s.cfg.name).map(|(b, _)| b))
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
        let (Some(t), Some(board)) = (&self.tab.thread, &self.tab.board) else { return None };
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
        let Some(board) = self.tab.board.clone() else { return };
        let target = match &link.board {
            Some(uri) if *uri != board.uri => self.find_board(uri),
            _ => board.clone(),
        };
        match (link.thread, link.post) {
            (Some(no), post) => {
                if let Some(t) = self.tab.thread.as_ref().filter(|_| self.tab.view == View::Thread) {
                    self.tab.trail.push((self.tab.site, board, t.no, t.current().map_or(t.no, |p| p.no)));
                }
                self.open_thread_at(target, no, post);
            }
            (None, None) => {
                // A board link: open its catalog.
                self.tab.return_to = None;
                self.open_catalog(target);
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
    fn open_thread_at(&mut self, board: Board, no: u64, post: Option<u64>) {
        self.tab.from_catalog = false;
        self.tab.gallery = None;
        self.tab.board = Some(board);
        self.tab.pending_post = post;
        self.tab.thread = None;
        self.tab.view = View::Thread;
        self.load_thread(no);
    }

    /// The post whose files `v`, `i`, `d` act on: the selected catalog entry or thread post.
    fn selected_post(&self) -> Option<&Post> {
        match self.tab.view {
            View::Catalog => self.selected_index().map(|i| &self.tab.catalog[i]),
            View::Thread => self.tab.thread.as_ref().and_then(ThreadView::current),
            _ => None,
        }
    }

    fn open_viewer(&mut self) {
        if !self.images.enabled() {
            self.info("Images are off (images = \"off\" in the config); i opens the file");
            return;
        }
        if self.images_off_here() {
            return;
        }
        let link = self.selected_link();
        match self.selected_post().map(|p| p.files.clone()) {
            Some(files) if !files.is_empty() => {
                if !self.thread_viewer(0) {
                    self.tab.popup = Some(TabPopup::Viewer(Viewer::new(files, 0, link)));
                }
            }
            _ => self.info("Post has no file"),
        }
    }

    /// Open a file externally: videos in mpv when it's installed, everything else in the default opener.
    pub fn open_file(&mut self, f: &Attachment) {
        if f.is_video() && on_path("mpv") && !crate::sandboxed() {
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
        if self.tab.view == View::Sites {
            match self.selected_site_row() {
                Some(SiteRow::Favorite(i)) => {
                    let f = self.favorites.remove(i);
                    self.save_favorites(&format!("/{}/ off the favorites", f.board));
                }
                Some(SiteRow::Recent(i)) => {
                    self.store.recent_boards.remove(i);
                    self.save_now();
                    self.clamp_home();
                }
                Some(SiteRow::Site(i)) => self.toggle_site_hidden(i),
                _ => {}
            }
            return;
        }
        let Some(i) = self.selected_index() else { return };
        match self.tab.view {
            View::Watched => {
                let w = self.store.watched.remove(i);
                self.info(format!("Stopped watching thread {}", w.key.no));
            }
            View::History => {
                self.store.history.remove(i);
            }
            View::Saved => {
                let key = self.store.saved[i].key.clone();
                // Asked first: a copy can't be fetched again once the thread is gone.
                if self.saved_confirm.take().as_ref() != Some(&key) {
                    let x = self.keys.key(Action::Remove);
                    self.info(format!("Press {x} again to remove the saved copy of thread {}", key.no));
                    self.saved_confirm = Some(key);
                    return;
                }
                self.store.forget_saved(&key);
                self.info(format!("Removed the saved copy of thread {}", key.no));
            }
            _ => return,
        }
        self.save_now();
        self.clamp_list();
    }

    /// Index of the selected item in the current list's underlying data (not for Sites).
    fn selected_index(&self) -> Option<usize> {
        match self.tab.view {
            View::Sites | View::Thread | View::Settings | View::Search => None,
            View::Boards => self.tab.board_list.state.selected().and_then(|i| self.visible_boards().get(i).copied()),
            View::Catalog => self.tab.catalog_list.state.selected().and_then(|i| self.visible_catalog().get(i).copied()),
            View::Watched => self.watched_list.state.selected().and_then(|i| self.visible_watched().get(i).copied()),
            View::History => self.history_list.state.selected().and_then(|i| self.visible_history().get(i).copied()),
            View::Saved => self.saved_list.state.selected().and_then(|i| self.visible_saved().get(i).copied()),
        }
    }

    fn enter(&mut self) {
        match (self.tab.view, self.selected_index()) {
            (View::Settings, _) => self.activate_setting(),
            (View::Search, _) => self.open_search_hit(),
            (View::Sites, _) => match self.selected_site_row() {
                Some(SiteRow::Watched) => self.tab.view = View::Watched,
                Some(SiteRow::History) => self.tab.view = View::History,
                Some(SiteRow::Saved) => self.tab.view = View::Saved,
                Some(SiteRow::Favorite(i)) => self.open_favorite(i),
                Some(SiteRow::Recent(i)) => {
                    if let Some(b) = self.recent_board(i) {
                        self.open_board(&b);
                    }
                }
                Some(SiteRow::Site(i)) => self.enter_site(i),
                Some(SiteRow::HiddenSites) => {
                    self.show_hidden_sites = !self.show_hidden_sites;
                    self.clamp_home();
                }
                None => {}
            },
            (View::Watched, Some(i)) => self.open_key(self.store.watched[i].key.clone()),
            (View::History, Some(i)) => self.open_key(self.store.history[i].key.clone()),
            (View::Saved, Some(i)) => self.open_saved(self.store.saved[i].key.clone()),
            (View::Boards, Some(i)) => self.open_catalog(self.boards()[i].clone()),
            (View::Catalog, Some(i)) => {
                let no = self.tab.catalog[i].no;
                // On an overboard the thread lives on its own board.
                if let Some(uri) = self.tab.catalog[i].board.clone().filter(|b| *b != self.tab.catalog_board) {
                    self.tab.board = Some(self.find_board(&uri));
                }
                self.tab.thread = None;
                self.tab.return_to = None;
                self.tab.from_catalog = true;
                self.tab.view = View::Thread;
                self.load_thread(no);
            }
            _ => {}
        }
    }

    /// Show `board`'s catalog from the top, and load it.
    fn open_catalog(&mut self, board: Board) {
        self.tab.board = Some(board);
        self.tab.catalog.clear();
        self.tab.catalog_cached = None;
        self.tab.catalog_list = Picker::top();
        self.tab.view = View::Catalog;
        self.load_catalog();
    }

    fn enter_site(&mut self, i: usize) {
        if i != self.tab.site {
            self.tab.board_list = Picker::default();
        }
        self.tab.site = i;
        self.tab.view = View::Boards;
        if self.tab.board_list.state.selected().is_none() {
            self.tab.board_list.state.select(Some(0));
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
                if self.clock.now().saturating_sub(fetched) > BOARDS_MAX_AGE {
                    self.refresh_boards_in_background(i);
                }
            }
            None => self.load_boards(),
        }
    }

    fn back(&mut self) {
        self.tab.gallery = None;
        self.tab.view = match self.tab.view {
            View::Sites | View::Boards | View::Watched | View::History | View::Saved => View::Sites,
            View::Settings => self.tab.settings_back.take().unwrap_or(View::Sites),
            View::Search => self.close_search(),
            View::Catalog => View::Boards,
            View::Thread => self.tab.return_to.take().unwrap_or(View::Catalog),
        };
        self.tab.trail.clear();
        // Back to the catalog the thread was opened from (an overboard's, maybe).
        if self.tab.view == View::Catalog && std::mem::take(&mut self.tab.from_catalog) {
            self.tab.board = self.tab.catalog_of.clone();
        }
        // After following links to another board (or a board of the same name on another
        // site), the loaded catalog is for the old one.
        if self.tab.view == View::Catalog
            && let Some(board) = self.tab.board.clone().filter(|b| b.uri != self.tab.catalog_board || self.tab.site != self.tab.catalog_site)
        {
            self.open_catalog(board);
            return;
        }
        // Navigating away cancels any in-flight request (its response will be ignored).
        if self.tab.loading.is_some() {
            self.tab.req = 0;
            self.tab.loading = None;
        }
    }

    fn refresh(&mut self) {
        match self.tab.view {
            View::Sites | View::Watched | View::History | View::Saved | View::Settings => {}
            View::Search => {
                if let Some(s) = &mut self.tab.search {
                    s.hits.clear();
                    s.pages = 0;
                }
                self.load_search_page();
            }
            View::Boards => self.load_boards(),
            View::Catalog => self.load_catalog(),
            View::Thread if self.tab.offline.is_some() => self.refresh_saved(),
            View::Thread => {
                if let Some(no) = self.tab.thread.as_ref().map(|t| t.no) {
                    self.load_thread(no);
                }
            }
        }
    }

    /// Copy the selected thing's text (or file URL in the viewer), or with `link` its URL.
    fn copy(&mut self, link: bool) {
        let what = if let Some(v) = self.tab.viewer() {
            let post_link = v.link.clone().or_else(|| self.viewer_post_link()).or_else(|| self.gallery_link(v.index));
            if link { post_link.map(|l| ("link", l)) } else { Some(("file URL", v.files[v.index].url.clone())) }
        } else if link {
            self.selected_link().map(|l| ("link", l))
        } else {
            let saved = |key: &ThreadKey, subject: &str| {
                let url = self.thread_link(key, None).unwrap_or_default();
                format!("{subject}\n{url}").trim().to_string()
            };
            match self.tab.view {
                View::Thread => self.tab.thread.as_ref().and_then(ThreadView::current).map(|p| ("text", copy_text(p, false))),
                View::Catalog => self.selected_post().map(|p| ("text", copy_text(p, true))),
                View::Watched => self.selected_index().map(|i| ("text", saved(&self.store.watched[i].key, &self.store.watched[i].subject))),
                View::History => self.selected_index().map(|i| ("text", saved(&self.store.history[i].key, &self.store.history[i].subject))),
                View::Saved => self.selected_index().map(|i| ("text", saved(&self.store.saved[i].key, &self.store.saved[i].subject))),
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
        match (self.tab.view, &self.tab.board) {
            (View::Watched, _) => self.selected_index().and_then(|i| self.thread_link(&self.store.watched[i].key, None)),
            (View::History, _) => self.selected_index().and_then(|i| self.thread_link(&self.store.history[i].key, None)),
            (View::Saved, _) => self.selected_index().and_then(|i| self.thread_link(&self.store.saved[i].key, None)),
            (View::Boards, _) => self.selected_index().map(|i| backend.board_url(&self.boards()[i].uri)),
            (View::Catalog, Some(b)) => self.selected_index().map(|i| {
                let p = &self.tab.catalog[i];
                backend.thread_url(p.board.as_deref().unwrap_or(&b.uri), p.no)
            }),
            (View::Thread, Some(b)) => self.tab.thread.as_ref().and_then(|t| self.thread_link(&self.key(&b.uri, t.no), Some(t.current()?.no))),
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
        if crate::sandboxed() {
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

fn plural_posts(n: usize) -> String {
    if n == 1 { "1 post".into() } else { format!("{n} posts") }
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
/// Where j/k (or the arrows) move one row and g/G (or home/end) jump to the ends of a list
/// of `len` rows, from row `cur`; `None` for other keys.
fn list_move(code: KeyCode, cur: usize, len: usize) -> Option<usize> {
    let last = len.saturating_sub(1);
    match code {
        KeyCode::Char('j') | KeyCode::Down => Some((cur + 1).min(last)),
        KeyCode::Char('k') | KeyCode::Up => Some(cur.saturating_sub(1)),
        KeyCode::Char('g') | KeyCode::Home => Some(0),
        KeyCode::Char('G') | KeyCode::End => Some(last),
        _ => None,
    }
}

/// Backspace and typed characters in a text field; other keys do nothing.
fn edit_text(text: &mut String, code: KeyCode) {
    match code {
        KeyCode::Backspace => {
            text.pop();
        }
        KeyCode::Char(c) => text.push(c),
        _ => {}
    }
}

fn filtered(filter: &str, n: usize, text: impl Fn(usize) -> String) -> Vec<usize> {
    let needle = filter.to_lowercase();
    (0..n).filter(|&i| needle.is_empty() || text(i).to_lowercase().contains(&needle)).collect()
}

#[cfg(test)]
mod fuzz;
#[cfg(test)]
pub mod tests;
