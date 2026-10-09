use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::ListState;

use crate::backend::{self, Backend};
pub use crate::config::Sort;
use crate::config::{CatalogLayout, ColorMode, Config, ImagesMode, SiteConfig, SiteKind};
use crate::disk_cache::DiskCache;
use crate::download;
use crate::filter::{Filters, Hidden};
use crate::http;
use crate::images::Images;
use crate::keys::{Action, KeyMap, Scope};
use crate::model::{Attachment, Board, FileKind, Link, Post, Thread, max_no, shrank};
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
mod footer;
use footer::Footer;
pub use footer::{Problem, Status};
mod gallery;
mod input;
mod generals;
mod loading;
mod thread_view;
mod goto;
mod hiding;
pub use hiding::{Changed, Hiding, Marks, ancestry, with_ancestry};
pub use goto::start_error;
pub use loading::Whole;
mod home;
mod links;
mod list;
pub use list::FilteredList;
mod posting;
pub use posting::{Art, Compose, Field, Stage, ViewAt, grid_cols};
mod saved;
mod saving;
mod board_images;
mod search;
mod session;
mod tab;
mod tabs;
mod settings;
mod sites;
pub use filters::{AddFilter, EDIT_ROWS, EditRow, problem as filters_problem, with_text as filters_with_text};
pub use focus::{Hints, Menu, MenuItem};
#[cfg(test)]
pub use focus::HintTo;
pub use gallery::Gallery;
pub use home::BoardRef;
pub use links::{ImageSearchPanel, LinkItem, LinksPanel};
pub use saving::Saving;
pub use search::Search;
#[cfg(test)]
pub use search::SavedSearch;
pub use saving::Downloads;
pub use thread_view::{LineCache, Media, Part, Reveal, Spot, ThreadLayout, ThreadView, parts};
#[cfg(test)]
pub use thread_view::{CONVERSATION_MAX, conversation_of};
pub use sites::{Adding, MySites, origin as site_origin};
#[cfg(test)]
pub use sites::BoardsUpdate;
pub use tab::{Catalog, Opening, Tab, Then};
pub use tabs::{MAX_TABS, Offline, TabPopup, ThreadCopy, Trail};
pub use settings::{SettingsPopup, key_rows, rows as setting_rows, settings, tilde};

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
/// What opening, copying or saving a file says when the site has neither it nor its thumbnail.
const NEITHER: &str = "Neither the file nor its thumbnail is available";

const SAVE_EVERY: Duration = Duration::from_secs(2);

/// Watched-thread refreshes running at once.
const MAX_REFRESHING: usize = 2;

/// A thread's refreshes slow down while it's quiet (`refresh_backoff`): each one that brings
/// no new post makes the next wait half as long again, up to `QUIET_TIMES` the interval set
/// and at most `QUIET_MAX` (unless the interval set is longer). New posts start over.
const QUIET_TIMES: u32 = 10;
const QUIET_MAX: Duration = Duration::from_secs(600);

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

/// What a list row shows, wherever it's moved to: a selection, a menu and a hint label keep
/// this, so they stay on what they were put on after the list changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKey {
    /// A home screen row other than a board: those move only by the user's own keys.
    Site(SiteRow),
    /// A favorite or recent board on the home screen, by its key (`site/board`): a board is
    /// one or the other, so starring one keeps it selected.
    HomeBoard(String),
    Board(String),
    /// A catalog thread.
    Thread(u64),
    /// A watched, history or saved thread.
    Listed(ThreadKey),
    /// A search hit: its thread (the saved copy's, or the archive's) and post.
    Hit(ThreadKey, u64),
    /// A setting, by its place in the fixed list.
    Setting(usize),
}

pub struct Site {
    pub cfg: SiteConfig,
    pub backend: Arc<dyn Backend>,
    pub boards: Option<Vec<Board>>,
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
    /// The thread: its area, and the line drawn at its top.
    Thread { area: Rect, scroll: usize },
    /// The catalog grid: its area, first visible item, columns, and cell size.
    Grid { area: Rect, offset: usize, cols: usize, cell: (u16, u16) },
    /// The settings screen, laid out as `settings::rows()` from `offset`.
    Settings { area: Rect, offset: usize },
}

impl Hit {
    /// The list row or grid cell drawn at a screen position (a thread's posts aren't rows:
    /// see `thread_part_at`).
    pub(super) fn row_at(self, col: u16, row: u16) -> Option<usize> {
        let pos = ratatui::layout::Position::new(col, row);
        match self {
            Hit::List { area, offset, item_height } if area.contains(pos) => {
                Some(offset + ((row - area.y) / item_height.max(1)) as usize)
            }
            Hit::Settings { area, offset } if area.contains(pos) => settings::rows().get(offset + (row - area.y) as usize)?.ok(),
            Hit::Grid { area, offset, cols, cell } if area.contains(pos) => {
                let c = ((col - area.x) / cell.0) as usize;
                (c < cols).then(|| offset + ((row - area.y) / cell.1) as usize * cols + c)
            }
            _ => None,
        }
    }
}

/// What the last frame drew where, for the mouse. Only `ui::draw` makes one, whole, through
/// `App::begin_frame`, stamped with what that frame showed; a click resolves against
/// nothing else, and nothing once the stamp is stale.
#[derive(Debug, Default)]
pub struct Drawn {
    /// The list, grid or thread; the rows of the popup on top (menu, links, image search);
    /// each tab's chip; and what the frame showed (`None` once that may have changed).
    pub body: Option<Hit>,
    pub popup: Option<Hit>,
    pub tabs: Vec<(Rect, usize)>,
    shown: Option<input::Shown>,
    /// Whether nothing since may have moved its rows (see `forget_frame`): a click lands only
    /// then, while a grid's columns hold until the screen changes.
    fresh: bool,
}

/// New posts in a watched thread, for a notification.
struct Note {
    key: ThreadKey,
    subject: String,
    new: usize,
    /// Of those, replies to your posts.
    replies: usize,
    /// Posts (or a new thread) a `notify` filter caught, and the first one's label.
    caught: usize,
    filter: String,
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
    /// The key help, scrolled this far (as last drawn).
    Help(ListState),
    /// Writing a post (`P`).
    Reply(Box<Compose>),
}

/// What's being typed in the footer: one thing at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typing {
    /// What's typed after `:`.
    Goto(String),
    /// An archive search query.
    ArchiveQuery(String),
    /// A thread search; the text is the thread's `search`.
    ThreadSearch,
    /// A list's filter; the text is the list's `filter`.
    ListFilter,
}

/// Popup with the posts the selected post quotes.
pub struct Preview {
    /// Numbers of the quoted posts in this thread (not indices: a refresh can drop posts).
    pub posts: Vec<u64>,
    /// Quoted post numbers that aren't in this thread.
    pub elsewhere: Vec<u64>,
    /// How far it's scrolled (as last drawn).
    pub scroll: ListState,
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
    /// What a tab's request (`Tab::req`) found, to apply in that tab: another tab's is
    /// applied there, and one its tab has moved on from is stale and dropped, unless it's
    /// worth keeping anyway (a finished board list).
    ForRequest { id: u64, keep_if_stale: bool, apply: Box<dyn FnOnce(&mut App) + Send> },
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
    /// What request `id` found: `apply` runs in its tab, if it's still waiting for it.
    fn request(id: u64, apply: impl FnOnce(&mut App) + Send + 'static) -> Msg {
        Msg::ForRequest { id, keep_if_stale: false, apply: Box::new(apply) }
    }

    /// What load `id` came to, `res`: `apply` runs in its tab, once, if it's still the tab's
    /// load. Only this hands a load's answer out.
    fn answer<T: Send + 'static>(id: u64, res: T, apply: impl FnOnce(&mut App, T) + Send + 'static) -> Msg {
        Msg::request(id, move |app| {
            if app.tab.answered(id) {
                apply(app, res);
            }
        })
    }

    /// `request`, but applied even once its tab has moved on.
    fn kept(id: u64, apply: impl FnOnce(&mut App) + Send + 'static) -> Msg {
        Msg::ForRequest { id, keep_if_stale: true, apply: Box::new(apply) }
    }
}

pub struct App {
    pub sites: Vec<Site>,
    pub site_list: FilteredList,
    /// Favorite boards (from the config), and board titles for the home screen.
    pub favorites: Vec<BoardRef>,
    pub home_titles: HashMap<String, String>,
    /// Sites left off the home screen (from the config), and whether they're shown anyway.
    pub hidden_sites: std::collections::BTreeSet<String>,
    pub show_hidden_sites: bool,
    /// Watched threads come first in catalogs (`watched_first`).
    pub watched_first: bool,
    /// The catalog layout for boards without their own (`catalog_layout` in the config).
    pub default_layout: CatalogLayout,
    /// The popup over the screen, if any: one at a time.
    pub popup: Option<Popup>,
    pub settings_list: FilteredList,
    /// The current theme's name, and the config's custom themes.
    pub theme_name: String,
    pub themes: BTreeMap<String, ThemeDef>,
    pub color_mode: ColorMode,
    /// Draw 24-bit colors (else the nearest of 256).
    pub truecolor: bool,
    pub images_mode: ImagesMode,
    pub watched_list: FilteredList,
    pub history_list: FilteredList,
    pub saved_list: FilteredList,
    /// The saved copy `x` was pressed on once: a second `x` removes it.
    pub saved_confirm: Option<ThreadKey>,
    /// The last search of saved threads started (each has its own number).
    saved_search: u64,
    /// Sites taken out of the config in Settings: off the home screen until ck restarts.
    pub removed_sites: std::collections::BTreeSet<String>,
    pub store: Store,
    refresh_thread: Duration,
    refresh_watched: Duration,
    watched_checked: HashMap<ThreadKey, Instant>,
    /// Refreshes of each watched thread in a row that brought nothing (`refresh_backoff`).
    watched_quiet: HashMap<ThreadKey, u32>,
    pub refresh_backoff: bool,
    /// Background refreshes in flight.
    pub refreshing: HashSet<ThreadKey>,
    /// Where watched threads' boards (site, board) have their threads, by index page; when
    /// each board was last asked (once a `refresh_watched` round), and the asks in flight.
    pub board_pages: HashMap<(String, String), backend::ThreadPages>,
    pages_asked: HashMap<(String, String), Instant>,
    pub pages_asking: HashSet<(String, String)>,
    /// The newest post seen in each watched thread by a refresh this session; notifications
    /// are for posts past it.
    notified_max: HashMap<ThreadKey, u64>,
    /// The last answer for each thread that came back cut short (by posts): the next is
    /// judged against it (`accept`).
    short: HashMap<ThreadKey, usize>,
    /// The same for followed generals' boards (site, board): the newest thread seen there,
    /// for `notify` filters.
    board_notified_max: HashMap<(String, String), u64>,
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
    /// Filters, hidden words, hidden replies and `Z`: what decides what's hidden.
    pub hiding: Hiding,
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
    footer: Footer,
    pub images: Images,
    /// Reverse image search engines (`R`), and the panel choosing one.
    pub image_search: Vec<crate::config::ImageSearch>,
    /// Save where you are and start there next time.
    pub restore_session: bool,
    /// Reading the end of a thread, new posts come into view (`follow_new_posts`).
    pub follow_new_posts: bool,
    /// `set_title`: the terminal's title says where ck is and what's new; and the title as
    /// last written.
    pub set_title: bool,
    title_shown: Option<String>,
    /// Images on boards the site marks NSFW (`nsfw_images`).
    pub nsfw_images: crate::config::NsfwImages,
    /// NSFW boards of sites whose board list isn't loaded, from their saved lists; and the
    /// sites asked for theirs, once, to know.
    nsfw_saved: HashMap<usize, HashSet<String>>,
    nsfw_asked: HashSet<usize>,
    /// The data directory has changes to write, and when it was last written.
    save_pending: bool,
    saved_at: Instant,
    /// When the session was last checked for saving.
    session_saved: Instant,
    /// What's being typed, if anything.
    pub typing: Option<Typing>,
    pub keys: KeyMap,
    /// The last text copied to the clipboard, and the last URL opened.
    pub copied: Option<String>,
    pub opened: Option<String>,
    /// The config file that settings are saved to (tests point it elsewhere).
    pub config_path: Option<std::path::PathBuf>,
    pub clock: Clock,
    pub downloads: Downloads,
    pub(crate) download_dir: Option<String>,
    /// Where the last frame drew what can be clicked.
    pub drawn: Drawn,
    /// Last left click: when, and the list index or thread post it hit.
    last_click: Option<(Instant, usize)>,
    pub tick: usize,
    pub quit: bool,
    /// Why ck quit on its own (the terminal stopped giving input), to report after.
    pub quit_because: Option<String>,
    /// The active tab's place; the other tabs (the active one's slot holds nothing useful),
    /// and which is active.
    pub tab: Tab,
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// The last request id given out (ids are unique across tabs).
    next_id: u64,
    /// ck-web, once started for posting, and the post it's working on.
    web: Option<crate::web::Helper>,
    web_for: Option<posting::Where>,
    /// `web_helper`: where ck-web is.
    web_helper: Option<String>,
    /// Posts written and not sent, by where they go.
    drafts: HashMap<posting::Where, Compose>,
    /// The name and options last posted with.
    poster: (String, String),
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl App {
    /// `keys` and `filters` are built from `cfg` by the caller, which reports their errors.
    pub fn new(cfg: Config, keys: KeyMap, filters: Filters, picker: Option<ratatui_image::picker::Picker>, store: Store) -> Self {
        // ck 0.2's [theme] table of overrides becomes a theme of its own, "legacy".
        let mut themes = cfg.themes.clone();
        let theme_name = match &cfg.theme {
            None => theme::default_name().to_string(),
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
        // At most a day: a bigger number is a typo, and would overflow the clock's arithmetic.
        let refresh_thread = Duration::from_secs(cfg.refresh_thread_secs.clamp(10, 86400));
        let refresh_watched = Duration::from_secs(cfg.refresh_watched_secs.clamp(60, 86400));
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
            site_list: FilteredList::fresh(),
            favorites: cfg.favorites.iter().filter_map(|f| BoardRef::parse(f)).collect(),
            home_titles: HashMap::new(),
            hidden_sites: cfg.hidden_sites.iter().cloned().collect(),
            show_hidden_sites: false,
            watched_first: cfg.watched_first,
            default_layout: store.settings.catalog_layout.unwrap_or(match store.settings.compact_catalog {
                Some(true) => CatalogLayout::Compact,
                Some(false) => CatalogLayout::Cards,
                None => layout,
            }),
            settings_list: FilteredList::fresh(),
            theme_name,
            themes,
            color_mode: cfg.color,
            truecolor: cfg.color.truecolor(),
            images_mode: cfg.images,
            watched_list: FilteredList::fresh(),
            history_list: FilteredList::fresh(),
            saved_list: FilteredList::fresh(),
            saved_confirm: None,
            saved_search: 0,
            removed_sites: Default::default(),
            store,
            refresh_thread,
            refresh_watched,
            watched_checked: HashMap::new(),
            watched_quiet: HashMap::new(),
            refresh_backoff: cfg.refresh_backoff,
            refreshing: HashSet::new(),
            board_pages: HashMap::new(),
            pages_asked: HashMap::new(),
            pages_asking: HashSet::new(),
            notified_max: HashMap::new(),
            short: HashMap::new(),
            board_notified_max: HashMap::new(),
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
            hiding: Hiding::new(filters, cfg.recursive_hiding),
            hidden_words: cfg.hidden_words.clone(),
            filter_cfgs: cfg.filters.clone(),
            scroll_margin: if cfg.scroll_margin.is_finite() { cfg.scroll_margin.clamp(0.0, 0.5) } else { 0.3 },
            pages: crate::pages::Pages::default_dir()
                .filter(|_| !cfg!(test) && cfg.page_cache_mb > 0)
                .map(|d| crate::pages::Pages::new(d, cfg.page_cache_mb.saturating_mul(1024 * 1024))),
            filter_undo: None,
            footer: Footer::default(),
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
            set_title: cfg.set_title,
            title_shown: None,
            nsfw_images: cfg.nsfw_images,
            nsfw_saved: HashMap::new(),
            nsfw_asked: HashSet::new(),
            save_pending: false,
            saved_at: Instant::now(),
            session_saved: Instant::now(),
            typing: None,
            keys,
            copied: None,
            opened: None,
            config_path: Config::path(),
            clock: Clock::default(),
            downloads: Downloads::default(),
            download_dir: cfg.download_dir.clone(),
            drawn: Drawn::default(),
            last_click: None,
            tick: 0,
            quit: false,
            quit_because: None,
            tab: Tab::new(0, Instant::now()),
            tabs: vec![Tab::new(0, Instant::now())],
            active: 0,
            next_id: 0,
            web: None,
            web_for: None,
            web_helper: cfg.web_helper.clone(),
            drafts: HashMap::new(),
            poster: (String::new(), String::new()),
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
                self.sites
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| !self.removed_sites.contains(&s.cfg.name))
                    .filter(|&(i, _)| self.show_hidden_sites || !self.is_site_hidden(i))
                    .map(|(i, _)| SiteRow::Site(i)),
            )
            .chain((!self.hidden_sites.is_empty()).then_some(SiteRow::HiddenSites))
            .collect();
        let name = |row: &SiteRow| match row {
            SiteRow::Watched => "Watched".to_string(),
            SiteRow::History => "History".to_string(),
            SiteRow::Saved => "Saved".to_string(),
            SiteRow::Favorite(i) => self.favorites.get(*i).map_or(String::new(), |f| format!("{} /{}/ {}", f.site, f.board, self.board_title(f))),
            SiteRow::Recent(i) => {
                let r = self.recent_board(*i);
                r.map_or(String::new(), |r| format!("{} /{}/ {}", r.site, r.board, self.board_title(&r)))
            }
            SiteRow::Site(i) => self.sites.get(*i).map_or(String::new(), |s| s.cfg.name.clone()),
            SiteRow::HiddenSites => "hidden sites".to_string(),
        };
        filtered(&self.site_list.filter, &rows, name).into_iter().filter_map(|i| rows.get(i).copied()).collect()
    }

    pub fn visible_watched(&self) -> Vec<usize> {
        filtered(&self.watched_list.filter, self.store.all_watched(), |w| format!("{} {} {} {}", w.key.site, w.key.board, w.key.no, w.subject))
    }

    pub fn visible_saved(&self) -> Vec<usize> {
        filtered(&self.saved_list.filter, &self.store.saved, |s| format!("{} {} {} {}", s.key.site, s.key.board, s.key.no, s.subject))
    }

    pub fn visible_history(&self) -> Vec<usize> {
        filtered(&self.history_list.filter, &self.store.history, |h| format!("{} {} {} {}", h.key.site, h.key.board, h.key.no, h.subject))
    }

    pub fn boards(&self) -> &[Board] {
        self.sites.get(self.tab.site).and_then(|s| s.boards.as_deref()).unwrap_or(&[])
    }

    pub fn visible_boards(&self) -> Vec<usize> {
        filtered(&self.tab.board_list.filter, self.boards(), |b| format!("{} {}", b.uri, b.title))
    }

    pub fn visible_catalog(&self) -> Vec<usize> {
        let needle = self.tab.catalog_list.filter.to_lowercase();
        let mut v: Vec<(usize, &Post)> = self.shown_catalog().filter(|(_, p)| p.search_text().contains(&needle)).collect();
        match self.tab.catalog_sort {
            Sort::Bump => {}
            Sort::Replies => v.sort_by_key(|&(_, p)| std::cmp::Reverse(p.replies.unwrap_or(0))),
            Sort::Newest => v.sort_by_key(|&(_, p)| std::cmp::Reverse((p.time, p.no))),
            Sort::Oldest => v.sort_by_key(|&(_, p)| (p.time, p.no)),
        }
        // What a `top` filter highlights first, then (`watched_first`) the threads you watch,
        // each in the sort's order. The filters are rules written to come first whatever the
        // sort; watching is a sort of its own.
        let (c, top) = (&self.tab.catalog, |&(i, _): &(usize, &Post)| self.tab.catalog.marks.top(i));
        let watched: HashSet<(&str, u64)> = match self.watched_first {
            true => self.store.all_watched().iter().filter(|w| w.key.site == c.site()).map(|w| (w.key.board.as_str(), w.key.no)).collect(),
            false => HashSet::new(),
        };
        let watching = |&(_, p): &(usize, &Post)| !watched.is_empty() && watched.contains(&(c.board_of(p).as_str(), p.no));
        if v.iter().any(|t| top(t) || watching(t)) {
            v.sort_by_cached_key(|t| (!top(t), !watching(t)));
        }
        v.into_iter().map(|(i, _)| i).collect()
    }

    /// Whether a thread in the tab's catalog is watched.
    pub fn catalog_watching(&self, p: &Post) -> bool {
        self.store.watched(&self.tab.catalog.key(p)).is_some()
    }

    /// What's on screen, broadly: when it changes, the screen is painted whole (and the
    /// last frame's hits are stale). The tab's moves tell one thread from the next.
    pub fn screen(&self) -> (View, usize, bool, bool, u32, CatalogLayout) {
        (self.tab.view(), self.active, self.tab.viewer().is_some(), self.tab.gallery.is_some(), self.tab.moves(), self.layout())
    }

    /// A new frame's `Drawn`, stamped with what it shows: the only place one is made.
    pub(crate) fn begin_frame(&mut self) -> &mut Drawn {
        self.drawn = Drawn { shown: Some(self.shown()), fresh: true, ..Drawn::default() };
        &mut self.drawn
    }

    /// What the last frame drew, while it still shows this tab and view (a popup closed since
    /// doesn't move what was under it: the menu's `hints` reads it).
    pub(super) fn drawn(&self) -> Option<&Drawn> {
        self.drawn_here().filter(|d| d.fresh)
    }

    /// The last frame, while its screen and size still show, even if a key moved its rows.
    fn drawn_here(&self) -> Option<&Drawn> {
        let (screen, tabs, _) = self.drawn.shown?;
        ((screen, tabs) == (self.screen(), self.tabs.len())).then_some(&self.drawn)
    }

    /// The columns of the grid (catalog or gallery) the last frame drew (`Some(None)`: none),
    /// while its screen still shows: only the screen (layout included) and its size decide them.
    pub(super) fn drawn_cols(&self) -> Option<Option<usize>> {
        self.drawn_here().map(|d| match d.body {
            Some(Hit::Grid { cols, .. }) => Some(cols),
            _ => None,
        })
    }

    /// What each row of the list `view` shows is (none in a thread).
    pub(super) fn row_keys(&self, view: View) -> Vec<RowKey> {
        match view {
            View::Sites => self.visible_sites().into_iter().map(|r| match r {
                SiteRow::Favorite(i) => RowKey::HomeBoard(self.favorites.get(i).map(BoardRef::key).unwrap_or_default()),
                SiteRow::Recent(i) => RowKey::HomeBoard(self.store.recent_boards.get(i).cloned().unwrap_or_default()),
                r => RowKey::Site(r),
            }).collect(),
            View::Boards => self.visible_boards().into_iter().filter_map(|i| self.boards().get(i)).map(|b| RowKey::Board(b.uri.clone())).collect(),
            View::Catalog => self.visible_catalog().into_iter().filter_map(|i| self.tab.catalog.posts().get(i)).map(|p| RowKey::Thread(p.no)).collect(),
            View::Watched => self.visible_watched().into_iter().filter_map(|i| self.store.all_watched().get(i)).map(|w| RowKey::Listed(w.key.clone())).collect(),
            View::History => self.visible_history().into_iter().filter_map(|i| self.store.history.get(i)).map(|h| RowKey::Listed(h.key.clone())).collect(),
            View::Saved => self.visible_saved().into_iter().filter_map(|i| self.store.saved.get(i)).map(|s| RowKey::Listed(s.key.clone())).collect(),
            View::Search => {
                let (Some(s), site) = (&self.tab.search, &self.current_site().cfg.name) else { return Vec::new() };
                self.visible_hits().into_iter().filter_map(|k| Some(RowKey::Hit(s.thread_of(k, site)?, s.hits.get(k)?.1.no))).collect()
            }
            View::Settings => (0..settings::settings().count()).map(RowKey::Setting).collect(),
            View::Thread => Vec::new(),
        }
    }

    /// To send what background work finds back to this thread.
    fn later(&self) -> Later {
        Later(self.tx.clone())
    }

    #[allow(clippy::indexing_slicing)] // the config always has a site, and tab.site is always one of them
    pub fn current_site(&self) -> &Site {
        &self.sites[self.tab.site]
    }

    /// Where the site of this name is in `sites`: saved keys name their site, and it
    /// may have been removed from the config since.
    pub fn site_index(&self, name: &str) -> Option<usize> {
        self.sites.iter().position(|s| s.cfg.name == name)
    }

    /// The site of this name, if it's still in the config.
    pub fn site_named(&self, name: &str) -> Option<&Site> {
        self.sites.iter().find(|s| s.cfg.name == name)
    }

    /// What's typed after `:`, while it's being typed.
    pub fn goto_text(&self) -> Option<&str> {
        match &self.typing {
            Some(Typing::Goto(text)) => Some(text),
            _ => None,
        }
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
        self.footer.tick(self.clock.instant(), self.status_on_screen());
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
                let total = many.iter().fold(0usize, |t, n| t.saturating_add(n.replies));
                messages.push(format!("{total} new replies to your posts in {} threads", many.len()));
            }
        }
        let caught: Vec<&Note> = notes.iter().filter(|n| n.caught > 0).collect();
        match caught.as_slice() {
            [] => {}
            [n] => messages.push(format!("{} caught by \"{}\" in {}", if n.caught == 1 { "A new post".to_string() } else { format!("{} new posts", n.caught) }, n.filter, place(n))),
            many => {
                let total = many.iter().fold(0usize, |t, n| t.saturating_add(n.caught));
                messages.push(format!("{total} new posts caught by your filters in {} threads", many.len()));
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
                self.error(e.context("Couldn't notify"));
            }
        }
        if let Some(m) = messages.first() {
            self.footer.offer(m.clone());
        }
        self.notified.extend(messages);
        let len = self.notified.len();
        self.notified.drain(..len.saturating_sub(20));
    }

    /// How long the main loop may sleep: until the next animation frame, status expiry or
    /// due refresh, and at most a second (relative times like "5s ago" stay current).
    pub fn next_wake(&self, now: Instant) -> Duration {
        let mut wake = Duration::from_secs(1);
        let animating = self.tab.loading().is_some() || !self.refreshing.is_empty() || self.downloads.running > 0 || self.tab.viewer().is_some();
        if animating {
            wake = wake.min(Duration::from_millis(100));
        }
        // `t + d`, or never if that's past what the clock can hold.
        let mut after = |t: Instant, d: Duration| {
            if let Some(t) = t.checked_add(d) {
                wake = wake.min(t.saturating_duration_since(now));
            }
        };
        if let Some(since) = self.notes_since {
            after(since, Duration::from_secs(3));
        }
        if self.save_pending {
            after(self.saved_at, SAVE_EVERY);
        }
        // An animated GIF in the viewer: its next frame.
        if let Some(t) = self.images.next_frame() {
            after(t, Duration::ZERO);
        }
        if let Some(t) = self.footer.due() {
            after(t, Duration::ZERO);
        }
        if self.tab.view() == View::Thread && self.tab.thread.is_some() && self.tab.loading().is_none() && self.tab.saved().is_none() {
            after(self.tab.thread_checked, self.thread_every());
        }
        // At capacity, a finished refresh wakes the loop anyway (and due ones mustn't spin it).
        if self.refreshing.len() < MAX_REFRESHING {
            for w in self.store.all_watched().iter().filter(|w| !w.status.is_dead() && !self.refreshing.contains(&w.key)) {
                match self.watched_checked.get(&w.key) {
                    Some(&t) => after(t, self.watched_every(&w.key)),
                    None => after(now, Duration::ZERO),
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
            let read = crate::guard::catching(|| {
                loop {
                    match event::read() {
                        Ok(ev) => {
                            crate::input_log::note(|| format!("read    {ev:?}"));
                            if tx.send(Msg::Input(ev)).is_err() {
                                return None;
                            }
                        }
                        Err(e) => return Some(format!("couldn't read the terminal: {e}")),
                    }
                }
            });
            // Without input ck can't even be quit: quit for the user, saying why.
            if let Some(why) = read.unwrap_or_else(|what| Some(format!("ck hit a bug reading input: {what}"))) {
                let _ = tx.send(Msg::Done(Box::new(move |app: &mut App| {
                    app.quit = true;
                    app.quit_because = Some(why);
                })));
            }
        });
    }

    /// SIGTERM, SIGHUP (the terminal went away) and SIGQUIT quit as `q` does, so what ck
    /// has is saved and the terminal restored. A second one, if ck is stuck, ends it at once.
    #[cfg(unix)]
    pub fn quit_on_signals(&self) -> std::io::Result<()> {
        use signal_hook::consts::{SIGHUP, SIGQUIT, SIGTERM};
        let mut signals = signal_hook::iterator::Signals::new([SIGTERM, SIGHUP, SIGQUIT])?;
        let later = self.later();
        std::thread::spawn(move || {
            let mut forever = signals.forever();
            if forever.next().is_some() {
                later.run(|app| app.quit = true);
            }
            if let Some(again) = forever.next() {
                let _ = signal_hook::low_level::emulate_default_handler(again);
            }
        });
        Ok(())
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            // A loaded image is drawn in the space it already had.
            Msg::Wake => return,
            Msg::Input(ev) => {
                let acted = !matches!(ev, Event::Key(k) if k.kind != KeyEventKind::Press);
                crate::input_log::note(|| format!("handle  {ev:?}{}", if acted { "" } else { "  (not a press: ignored)" }));
                match ev {
                    Event::Key(key) if acted => self.on_key(key),
                    Event::Mouse(m) => return self.on_mouse(m, self.clock.instant()),
                    Event::Paste(text) => self.paste(&text),
                    // Nothing the last frame drew holds at another size, a grid's columns too.
                    Event::Resize(..) => self.drawn.shown = None,
                    _ => {}
                }
            }
            Msg::Done(apply) => apply(self),
            // A response to another request: another tab's is handled there, the rest are
            // stale. It lands on the place, under the settings if they're open.
            Msg::ForRequest { id, keep_if_stale, apply } => match self.tab_of(id) {
                Some(i) => self.handle_in_tab(i, Box::new(move |app| app.land(apply))),
                None if keep_if_stale || Some(id) == self.tab.req() => self.land(apply),
                None => {}
            },
        }
        // Anything else may have moved what the last frame drew (a key, a list re-sorted in
        // the background): a click does nothing until the next frame.
        self.forget_frame();
    }

    /// Apply what a request found: it lands on the place, under the settings if they're open,
    /// and they stay open over wherever it leaves the tab.
    fn land(&mut self, apply: Box<dyn FnOnce(&mut App) + Send>) {
        let over = self.tab.close_settings();
        apply(self);
        if over {
            self.tab.navigate(View::Settings);
        }
    }

    /// The footer's message, if it has one.
    pub fn status(&self) -> Option<&Status> {
        self.footer.get()
    }

    /// Whether the next draw shows the footer's message: not behind the spinner or a prompt
    /// (ui's draw_footer and draw_viewer).
    pub(crate) fn status_on_screen(&self) -> bool {
        self.tab.viewer().is_some() || matches!((&self.typing, self.tab.loading()), (Some(Typing::Goto(_)), _) | (None, None))
    }

    /// Say something in the footer for a moment (unless an error is waiting to be seen).
    pub fn info(&mut self, text: impl Into<String>) {
        self.footer.say(Status { text: text.into(), error: false });
    }

    /// The tab's load failed: say so, and keep saying it where what it loads would be.
    fn load_failed(&mut self, why: impl Into<Problem>) {
        let why = why.into().0;
        self.tab.failed = Some(format!("{why}. {} tries again", self.keys.key(Action::Reload)));
        self.error(why);
    }

    /// Say something went wrong (shown a little longer).
    pub fn error(&mut self, e: impl Into<Problem>) {
        self.footer.say(Status { text: e.into().0, error: true });
    }

    /// What was saved to the config, or `done` until ck quits because it couldn't be; whether
    /// it was saved.
    pub(super) fn say_saved(&mut self, done: &str, saved: Result<String>) -> bool {
        let ok = saved.is_ok();
        match saved {
            Ok(text) => self.info(text),
            Err(e) => self.error(format!("{done} for now; couldn't save it: {}", http::plain(&e))),
        }
        ok
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
            self.error(e.context("Couldn't save to the data directory"));
        }
    }

    // ----- filters and hiding -----

    /// Note which catalog threads are new since the last visit (and remember them all).
    fn catalog_seen(&mut self) {
        let c = &self.tab.catalog;
        let nos: Vec<u64> = c.posts().iter().map(|p| p.no).collect();
        let (site, board) = (c.site().to_string(), c.board().map_or(String::new(), |b| b.uri.clone()));
        self.tab.catalog.new = self.store.catalog_seen(&site, &board, &nos, self.clock.now());
        self.store.board_opened(&site, &board);
        self.note_titles(self.tab.site);
        self.save();
    }

    /// Replies a catalog thread has gained since it was last opened.
    pub fn new_replies(&self, p: &Post) -> Option<u32> {
        let seen = self.store.replies_seen(self.tab.catalog.site(), &self.tab.catalog.board_of(p), p.no)?;
        p.replies.filter(|&r| r > seen).map(|r| r - seen)
    }

    /// `H`: hide or unhide the selected thread (catalog) or post (thread) by hand.
    fn toggle_hidden(&mut self) {
        let (key, what, mark) = match self.tab.view() {
            View::Catalog => {
                let Some(i) = self.selected_index() else { return };
                let Some(p) = self.tab.catalog.posts().get(i) else { return };
                (self.tab.catalog.key(p), "thread", self.tab.catalog.marks.why_hidden(i).cloned())
            }
            View::Thread => {
                let Some(t) = &self.tab.thread else { return };
                let (what, k) = (if t.selected == 0 { "thread" } else { "post" }, t.key());
                (ThreadKey { no: t.current().map_or(k.no, |p| p.no), ..k.clone() }, what, t.marks.why_hidden(t.selected).cloned())
            }
            _ => return,
        };
        match mark {
            Some(Hidden::ByFilter(label)) => return self.info(format!("Hidden by the filter \"{label}\"; Settings › Filters changes it")),
            Some(Hidden::Reply(to)) => return self.info(format!("Hidden as a reply to No.{to}, which is hidden; unhiding that one shows it")),
            _ => {}
        }
        let (no, hidden) = (key.no, self.rehide(|a| a.store.toggle_hidden(&key.site, &key.board, key.no)));
        self.save_now();
        let show = self.keys.key(Action::ShowHidden);
        self.info(if hidden { format!("Hid {what} {no} ({show} shows hidden ones)") } else { format!("Unhid {what} {no}") });
    }

    /// `Z`: show hidden threads and posts (dimmed), or leave them out again.
    fn toggle_show_hidden(&mut self) {
        let shown = self.rehide(|a| a.hiding.toggle_show());
        self.info(if shown { "Showing hidden threads and posts" } else { "Leaving out hidden threads and posts" });
    }

    // ----- downloads and settings -----

    /// The open catalog's board, as `site/board` (for its own sort, layout and images).
    fn board_key(&self) -> String {
        crate::store::board_key(self.tab.catalog.site(), self.tab.catalog.board().map_or("", |b| b.uri.as_str()))
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
            Ok(())
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
                self.info(format!("Default catalog layout: {name} (kept in the data directory: {})", http::plain(&e)));
            }
        }
    }

    /// A thread 404'd: say so, and offer the site's archive if it has one.
    ///
    /// With a saved copy, that comes first: a thread still on screen becomes its saved copy,
    /// and otherwise `enter` opens it.
    pub(crate) fn thread_gone(&mut self, key: &ThreadKey) {
        self.tab.archive_offer = self.archive_of(key);
        let in_archive = self.tab.archive_offer.as_ref().map(|a| format!("{} opens it in {}", self.keys.key(Action::Archive), a.site));
        let saved = self.store.saved(key).map(|m| m.saved);
        let shown = self.tab.thread.as_ref().is_some_and(|t| t.key() == key);
        let text = match saved {
            Some(at) if shown => {
                self.tab.copy = Some(ThreadCopy::Saved(Offline { saved: at, dead: true }));
                format!("Thread was deleted or archived: this is its saved copy{}", in_archive.map(|a| format!(" ({a})")).unwrap_or_default())
            }
            Some(at) => {
                self.tab.saved_offer = Some(key.clone());
                let ago = crate::ui::ago(at, self.clock);
                format!("Thread was deleted or archived. A saved copy from {ago}: enter opens it{}", in_archive.map(|a| format!(", {a}")).unwrap_or_default())
            }
            None => {
                // A copy shown while it loaded stays, marked as gone.
                if let Some(ThreadCopy::Cached(c)) = self.tab.copy.as_mut().filter(|_| shown) {
                    c.dead = true;
                }
                format!("Thread was deleted or archived{}", in_archive.map(|a| format!(": {a}")).unwrap_or_default())
            }
        };
        self.tab.failed = Some(text.clone());
        self.error(text);
    }

    fn toggle_watch(&mut self) {
        let (key, subject, posts, last_seen) = match self.tab.view() {
            View::Thread => {
                let Some(t) = &self.tab.thread else { return };
                let posts = t.live_posts();
                (t.key().clone(), thread_subject(&posts), t.known, max_no(&posts))
            }
            View::Catalog => {
                let Some(op) = self.selected_post() else { return };
                let posts = op.replies.map_or(1, |r| r as usize + 1);
                // Unknown until the first refresh, which then counts nothing as unread.
                (self.tab.catalog.key(op), thread_subject(std::slice::from_ref(op)), posts, 0)
            }
            _ => return,
        };
        // Unwatching forgets which posts are yours; watching changes nothing hidden.
        let watching = self.store.watch(key.clone(), subject, posts, last_seen);
        if !watching {
            self.rehide(|a| a.store.unwatch(&key));
        }
        if watching {
            self.keep_open_thread(&key);
        }
        // Refreshed soon, to learn where it's at, unless it was just fetched.
        let now = self.clock.instant();
        if self.watched_checked.get(&key).is_none_or(|t| now.saturating_duration_since(*t) >= http::MIN_REFETCH) {
            self.watched_checked.remove(&key);
        }
        self.watched_quiet.remove(&key);
        self.info(if watching { format!("Watching thread {}", key.no) } else { format!("Stopped watching thread {}", key.no) });
        self.save_now();
    }

    /// The thread being read: shown, with no gallery over it.
    fn reading(&mut self) -> Option<&mut ThreadView> {
        let shown = self.tab.view() == View::Thread && self.tab.gallery.is_none();
        self.tab.thread.as_mut().filter(|_| shown)
    }

    /// `c`: the selected post's conversation alone, or the whole thread again.
    fn toggle_conversation(&mut self) {
        let Some(t) = self.reading() else { return };
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

    /// `I`: the selected post's poster's posts alone, or the whole thread again.
    fn toggle_poster(&mut self) {
        let Some(t) = self.reading() else { return };
        if t.conversation.as_ref().is_some_and(|c| c.poster.is_some()) {
            t.leave_conversation();
            return;
        }
        match t.enter_poster() {
            Ok(n) => {
                let id = t.conversation.as_ref().and_then(|c| c.poster.clone()).unwrap_or_default();
                self.info(format!("{} by ID:{id}; esc or {} shows the whole thread", plural_posts(n), self.keys.key(Action::Poster)));
            }
            Err(e) => self.info(e),
        }
    }

    /// `M`: all posts, only those with files, all with their images hidden, and round again.
    fn cycle_media(&mut self) {
        let Some(t) = self.reading() else { return };
        let next = t.media.next();
        let n = t.set_media(next);
        let talking = t.conversation.is_some();
        let again = self.keys.key(Action::Media);
        self.info(match next {
            Media::Files if talking => format!("Only posts with files, once you leave the conversation; {again} again hides images"),
            Media::Files => format!("{} with files; {again} again shows all, images hidden", plural_posts(n)),
            Media::NoImages => format!("All posts, images hidden (none are asked for); {again} again shows them"),
            Media::All => "All posts and images".into(),
        });
    }

    /// Whether the open thread's images are shown: its board's setting, unless `M` hides
    /// them here.
    pub fn thread_images_on(&self) -> bool {
        self.tab.thread.as_ref().is_none_or(|t| t.media != Media::NoImages && self.images_on(&t.key().site, &t.key().board))
    }

    /// Keep a copy of a thread's posts in the data directory. Unchanged posts aren't written
    /// again.
    fn keep_copy(&mut self, key: &ThreadKey, t: &Whole) {
        let url = self.thread_link(key, None).unwrap_or_default();
        if self.store.keep_thread(key, &thread_subject(t.posts()), &url, t, self.clock.now()) {
            self.save();
        }
    }

    /// A thread just watched: if it's the one open (and loaded), it's saved at once. A saved
    /// copy open is kept as it is.
    fn keep_open_thread(&mut self, key: &ThreadKey) {
        if self.tab.view() != View::Thread || (self.tab.saved().is_some() && self.store.saved(key).is_some()) {
            return;
        }
        if let Some(w) = self.tab.thread.as_ref().filter(|t| t.key() == key).and_then(|t| self.shown_whole(key, t)) {
            self.keep_copy(key, &w);
        }
        if self.tab.saved().is_some_and(|o| o.dead) {
            self.store.saved_dead(key);
        }
    }

    /// `m`: mark the selected post as yours (or not), to hear about replies to it. The
    /// thread is watched if it isn't.
    fn toggle_mine(&mut self) {
        let Some(t) = &self.tab.thread else { return };
        let key = t.key().clone();
        let Some(no) = t.current().map(|p| p.no) else { return };
        if self.store.watched(&key).is_none() {
            let posts = t.live_posts();
            let (subject, len, max_no) = (thread_subject(&posts), t.known, max_no(&posts));
            self.store.watch(key.clone(), subject, len, max_no);
            self.keep_open_thread(&key);
        }
        let mine = self.rehide(|a| a.store.toggle_mine(&key, no));
        self.info(if mine { format!("Marked No.{no} as yours; replies to it will be counted and notified") } else { format!("No.{no} isn't marked as yours any more") });
        self.save_now();
    }

    /// Open a thread from Watched or History, switching site and board as needed.
    fn open_key(&mut self, key: &ThreadKey) {
        let back = Some(self.tab.place_view());
        if self.open_thread_key(key, Opening::default()) {
            self.tab.return_to = back;
        }
    }

    /// Open thread `key` on its site, as `open` says: whether that site is still in the config.
    fn open_thread_key(&mut self, key: &ThreadKey, open: Opening) -> bool {
        let Some(site) = self.site_index(&key.site) else {
            self.error(format!("No site named {} in the config", key.site));
            return false;
        };
        self.switch_site(site);
        self.open_thread_at(self.find_board(&key.board), key.no, open);
        true
    }

    /// Put the tab on `key`'s site and board, where the thread it shows is.
    fn place_on(&mut self, key: &ThreadKey) {
        // Another site's board of the same name is still another board.
        let site = self.site_index(&key.site).unwrap_or(self.tab.site);
        if site != self.tab.site || self.tab.board.as_ref().is_none_or(|b| b.uri != key.board) {
            self.switch_site(site);
            self.tab.board = Some(self.find_board(&key.board));
        }
    }

    /// Make `site` the current one, with its board list if it's saved (else fetched in the
    /// background), so going back to Boards shows it.
    fn switch_site(&mut self, site: usize) {
        if site != self.tab.site {
            self.tab.board_list = FilteredList::fresh();
            self.tab.site = site;
        }
        // Also for the site it's on already (the first, at startup: `ck 4chan` showed none).
        if self.sites.get(site).is_some_and(|s| s.boards.is_none()) {
            match self.known_boards(site) {
                Some(boards) => {
                    if let Some(s) = self.sites.get_mut(site) {
                        s.boards = Some(boards);
                    }
                }
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
            Some(link) => self.follow(&link),
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
                && (l.thread == Some(t.key().no) || (l.thread.is_none() && l.post.is_some_and(|p| t.index.contains_key(&p))));
            !in_thread
        };
        let links = &t.current()?.links;
        links.iter().filter(leaves).find(|l| l.post.is_some()).or_else(|| links.iter().find(leaves)).cloned()
    }

    /// Remember the thread shown and its selected post, for `u` to come back to.
    fn leave_trail(&mut self) {
        if let Some(here) = self.trail_here() {
            self.tab.trail.push(here);
        }
    }

    /// The thread shown and its selected post, as `u` comes back to it.
    fn trail_here(&self) -> Option<Trail> {
        let t = self.tab.thread.as_ref()?;
        Some((t.key().clone(), t.current().map_or_else(|| t.key().no, |p| p.no)))
    }

    /// Go where a quote link leads: a thread (remembered for `u`), a board, or a post whose
    /// thread the engine is asked for.
    fn follow(&mut self, link: &Link) {
        let Some(here) = &self.tab.board else { return };
        let target = match &link.board {
            Some(uri) if *uri != here.uri => self.find_board(uri),
            _ => here.clone(),
        };
        match (link.thread, link.post) {
            (Some(no), post) => {
                if self.tab.view() == View::Thread {
                    self.leave_trail();
                }
                self.open_thread_at(target, no, Opening::at(post));
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
                let (site, trail) = (self.tab.site, self.trail_here().filter(|_| self.tab.view() == View::Thread));
                self.spawn(self.current_site().backend.clone(), label, Then::Show, move |b, _, _| b.find_thread(&uri, post), move |app, r| app.thread_found(site, target, post, trail, r));
            }
        }
    }

    /// The engine said which thread on `site`'s `board` the quoted `post` is in: open it
    /// there (only now moving to that site), leaving `trail`, where the lookup was asked
    /// from, for `u`.
    fn thread_found(&mut self, site: usize, board: Board, post: u64, trail: Option<Trail>, res: Result<Option<u64>>) {
        match res {
            Ok(Some(no)) => {
                self.tab.trail.extend(trail);
                self.switch_site(site);
                self.open_thread_at(board, no, Opening::at(Some(post)));
            }
            Ok(None) => {
                self.error(format!("Post {post} isn't in this thread, and this site can't say which thread it's in"));
            }
            Err(e) if http::is_not_found(&e) => self.error(format!("Post {post} isn't on /{}/ (deleted, or never there)", board.uri)),
            Err(e) => self.error(e),
        }
    }

    /// A board by URI from the site's board list, or a bare one if the list isn't loaded.
    fn find_board(&self, uri: &str) -> Board {
        let known = self.boards().iter().find(|b| b.uri == uri).cloned();
        known.unwrap_or_else(|| Board { uri: uri.to_string(), title: String::new(), nsfw: None })
    }

    /// Open a thread on the current site, as `open` says once it arrives.
    fn open_thread_at(&mut self, board: Board, no: u64, open: Opening) {
        let key = ThreadKey { site: self.current_site().cfg.name.clone(), board: board.uri.clone(), no };
        self.tab.board = Some(board);
        self.tab.thread = None;
        self.tab.navigate(View::Thread);
        self.tab.from_catalog = false;
        self.load_thread(key, open);
    }

    /// The post whose files `v`, `i`, `d` act on: the selected catalog entry or thread post.
    pub(crate) fn selected_post(&self) -> Option<&Post> {
        match self.tab.view() {
            View::Catalog => self.selected_index().and_then(|i| self.tab.catalog.posts().get(i)),
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

    /// Open a file externally: videos in mpv when it's installed, everything else in the
    /// default opener; a file the site or archive didn't keep, as its thumbnail.
    pub fn open_file(&mut self, f: &Attachment) {
        let url = match (f.url.as_deref(), f.kind) {
            (Some(u), FileKind::Video) if on_path("mpv") && !crate::sandboxed() => u,
            (Some(u), _) => return self.open_url(u),
            (None, _) => return match &f.thumb {
                Some(t) => self.open_link(crate::model::THUMBNAIL_URL, t),
                None => self.info(NEITHER),
            },
        };
        let mut cmd = std::process::Command::new("mpv");
        cmd.arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        match cmd.spawn() {
            Ok(_) => self.info(format!("Playing {} in mpv", f.filename)),
            Err(e) => self.error(anyhow::Error::from(e).context("Couldn't start mpv")),
        }
    }

    fn remove_entry(&mut self) {
        if self.tab.view() == View::Sites {
            match self.selected_site_row() {
                Some(SiteRow::Favorite(i)) => {
                    let f = self.favorites.remove(i);
                    self.save_favorites(&format!("/{}/ off the favorites", f.board));
                }
                Some(SiteRow::Recent(i)) => {
                    self.store.recent_boards.remove(i);
                    self.save_now();
                }
                Some(SiteRow::Site(i)) => self.toggle_site_hidden(i),
                _ => {}
            }
            return;
        }
        let Some(i) = self.selected_index() else { return };
        match self.tab.view() {
            View::Watched => {
                let Some(key) = self.store.all_watched().get(i).map(|w| w.key.clone()) else { return };
                // As w does: its (You) marks go from every tab.
                self.rehide(|a| a.store.unwatch(&key));
                self.info(format!("Stopped watching thread {}", key.no));
            }
            View::History => {
                self.store.history.remove(i);
            }
            View::Saved => {
                let Some(key) = self.store.saved.get(i).map(|m| m.key.clone()) else { return };
                // Asked first: a copy can't be fetched again once the thread is gone. The
                // second press counts only while the question is still showing.
                let ask = format!("Press {} again to remove the saved copy of thread {}", self.keys.key(Action::Remove), key.no);
                let asked = self.footer.get().is_some_and(|s| s.text == ask);
                if self.saved_confirm.take().as_ref() != Some(&key) || !asked {
                    self.info(ask);
                    self.saved_confirm = Some(key);
                    return;
                }
                self.store.forget_saved(&key);
                self.info(format!("Removed the saved copy of thread {}", key.no));
            }
            _ => return,
        }
        self.save_now();
    }

    /// Index of the selected item in the current list's underlying data, found by its key.
    fn selected_index(&self) -> Option<usize> {
        let view = self.tab.view();
        match (view, self.selected_key(view)?) {
            (View::Boards, RowKey::Board(uri)) => self.boards().iter().position(|b| b.uri == uri),
            (View::Catalog, RowKey::Thread(no)) => self.tab.catalog.posts().iter().position(|p| p.no == no),
            (View::Watched, RowKey::Listed(k)) => self.store.all_watched().iter().position(|w| w.key == k),
            (View::History, RowKey::Listed(k)) => self.store.history.iter().position(|h| h.key == k),
            (View::Saved, RowKey::Listed(k)) => self.store.saved.iter().position(|s| s.key == k),
            _ => None,
        }
    }

    /// The selected thread in Watched, History or Saved: its key and subject.
    fn selected_listed(&self) -> Option<(&ThreadKey, &str)> {
        let i = self.selected_index()?;
        match self.tab.view() {
            View::Watched => self.store.all_watched().get(i).map(|w| (&w.key, w.subject.as_str())),
            View::History => self.store.history.get(i).map(|v| (&v.key, v.subject.as_str())),
            View::Saved => self.store.saved.get(i).map(|m| (&m.key, m.subject.as_str())),
            _ => None,
        }
    }

    fn enter(&mut self) {
        match (self.tab.view(), self.selected_index()) {
            (View::Settings, _) => self.activate_setting(),
            (View::Search, _) => self.open_search_hit(),
            (View::Sites, _) => match self.selected_site_row() {
                Some(SiteRow::Watched) => self.tab.navigate(View::Watched),
                Some(SiteRow::History) => self.tab.navigate(View::History),
                Some(SiteRow::Saved) => self.tab.navigate(View::Saved),
                Some(SiteRow::Favorite(i)) => self.open_favorite(i),
                Some(SiteRow::Recent(i)) => {
                    if let Some(b) = self.recent_board(i) {
                        self.open_board(&b);
                    }
                }
                Some(SiteRow::Site(i)) => self.enter_site(i),
                Some(SiteRow::HiddenSites) => {
                    self.show_hidden_sites = !self.show_hidden_sites;
                }
                None => {}
            },
            (View::Watched | View::History | View::Saved, Some(_)) => {
                let Some(key) = self.selected_listed().map(|(k, _)| k.clone()) else { return };
                if self.tab.view() == View::Saved { self.open_saved(&key, Opening::default()) } else { self.open_key(&key) }
            }
            (View::Boards, Some(i)) => {
                if let Some(b) = self.boards().get(i).cloned() {
                    self.open_catalog(b);
                }
            }
            (View::Catalog, Some(i)) => {
                let Some(p) = self.tab.catalog.posts().get(i) else { return };
                // On an overboard the thread lives on its own board.
                self.open_thread_at(self.find_board(&self.tab.catalog.board_of(p)), p.no, Opening::default());
                self.tab.return_to = None;
                self.tab.from_catalog = true;
            }
            _ => {}
        }
    }

    /// Show `board`'s catalog from the top, and load it.
    fn open_catalog(&mut self, board: Board) {
        self.tab.catalog = Catalog::default();
        self.tab.board = Some(board);
        self.tab.catalog_list = FilteredList::fresh();
        self.tab.navigate(View::Catalog);
        self.load_catalog();
    }

    fn enter_site(&mut self, i: usize) {
        if i != self.tab.site {
            self.tab.board_list = FilteredList::fresh();
        }
        self.tab.site = i;
        self.tab.navigate(View::Boards);
        if self.current_site().boards.is_some() {
            return;
        }
        // Board lists from the config need no request; fetched ones are saved, shown at
        // once next time, and refreshed in the background once a day.
        let cfg = &self.current_site().cfg;
        let saved = if cfg.boards.is_none() { self.store.load_boards(&cfg.name) } else { None };
        match saved {
            Some((boards, fetched)) => {
                if let Some(s) = self.sites.get_mut(i) {
                    s.boards = Some(boards);
                }
                if self.clock.now().saturating_sub(fetched) > BOARDS_MAX_AGE {
                    self.refresh_boards_in_background(i);
                }
            }
            None => self.load_boards(),
        }
    }

    fn back(&mut self) {
        // Closing the settings leaves the place under them as it was.
        if self.tab.close_settings() {
            return;
        }
        let from_catalog = self.tab.from_catalog;
        let to = match self.tab.view() {
            View::Search => self.close_search(),
            View::Catalog => View::Boards,
            View::Thread => self.tab.return_to.take().unwrap_or(View::Catalog),
            _ => View::Sites,
        };
        self.tab.navigate(to);
        self.tab.trail.clear();
        // Back to the catalog the thread was opened from (an overboard's, maybe).
        if to == View::Catalog && from_catalog {
            self.tab.board = self.tab.catalog.board().cloned();
        }
        // After following links to another board (or a board of the same name on another
        // site), the loaded catalog is for the old one.
        if to == View::Catalog
            && let Some(board) = self.tab.board.clone().filter(|b| !self.tab.catalog.is(&self.current_site().cfg.name, &b.uri))
        {
            self.open_catalog(board);
        }
    }

    fn refresh(&mut self) {
        match self.tab.view() {
            View::Sites | View::Watched | View::History | View::Saved | View::Settings => {}
            View::Search => {
                if let Some(s) = &mut self.tab.search {
                    s.hits.clear();
                    s.marks = Marks::default();
                    s.pages = 0;
                }
                self.load_search_page();
            }
            View::Boards => self.load_boards(),
            View::Catalog => self.load_catalog(),
            View::Thread if self.tab.saved().is_some() => self.refresh_saved(),
            // Not loaded yet (a failure, or another load took over): the thread asked for.
            View::Thread => {
                if let Some(key) = self.tab.thread.as_ref().map(|t| t.key().clone()).or_else(|| self.tab.pending_thread.clone()) {
                    self.load_thread(key, self.tab.opening());
                }
            }
        }
    }

    /// Copy the selected thing's text (or file URL in the viewer), or with `link` its URL.
    fn copy(&mut self, link: bool) {
        let what = if let Some(v) = self.tab.viewer() {
            let post_link = v.link.clone().or_else(|| self.viewer_post_link()).or_else(|| self.gallery_link(v.index));
            if link {
                post_link.map(|l| ("link", l))
            } else {
                let Some(f) = v.files.get(v.index) else { return };
                let Some((what, u)) = f.link() else { return self.info(NEITHER) };
                Some((what, u.to_string()))
            }
        } else if link {
            self.selected_link().map(|l| ("link", l))
        } else {
            let saved = |key: &ThreadKey, subject: &str| {
                let url = self.thread_link(key, None).unwrap_or_default();
                format!("{subject}\n{url}").trim().to_string()
            };
            match self.tab.view() {
                View::Thread => self.tab.thread.as_ref().and_then(ThreadView::current).map(|p| ("text", copy_text(p, false))),
                View::Catalog => self.selected_post().map(|p| ("text", copy_text(p, true))),
                View::Watched | View::History | View::Saved => self.selected_listed().map(|(key, subject)| ("text", saved(key, subject))),
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
            Err(e) => self.error(e.context("Couldn't copy")),
        }
        self.copied = Some(text);
    }

    /// A link to a thread (and post) on its site.
    fn thread_link(&self, key: &ThreadKey, post: Option<u64>) -> Option<String> {
        let site = self.site_named(&key.site)?;
        Some(match post {
            Some(p) if p != key.no => site.backend.post_url(&key.board, key.no, p),
            _ => site.backend.thread_url(&key.board, key.no),
        })
    }

    /// The link to what's selected: a post in a thread, a catalog thread, a saved thread, a board.
    fn selected_link(&self) -> Option<String> {
        match self.tab.view() {
            View::Watched | View::History | View::Saved => self.selected_listed().and_then(|(key, _)| self.thread_link(key, None)),
            View::Boards => self.selected_index().and_then(|i| self.boards().get(i)).map(|board| self.current_site().backend.board_url(&board.uri)),
            View::Catalog => self.selected_post().and_then(|p| self.thread_link(&self.tab.catalog.key(p), None)),
            View::Thread => self.tab.thread.as_ref().and_then(|t| self.thread_link(t.key(), Some(t.current()?.no))),
            _ => None,
        }
    }

    fn open_in_browser(&mut self) {
        if let Some(url) = self.selected_link() {
            self.open_url(&url);
        }
    }

    /// Open what [`Attachment::link`] or a focused part gave: a thumbnail standing in for
    /// its file says so.
    pub fn open_link(&mut self, what: &str, url: &str) {
        self.open_url(url);
        if what == crate::model::THUMBNAIL_URL && self.status().is_none_or(|s| !s.error) {
            self.info(format!("Only the thumbnail is available; opened {url}"));
        }
    }

    pub fn open_url(&mut self, url: &str) {
        self.opened = Some(url.to_string());
        if crate::sandboxed() {
            return;
        }
        match open::that_detached(url) {
            Ok(()) => self.info(format!("Opened {url}")),
            Err(e) => self.error(anyhow::Error::from(e).context(format!("Couldn't open {url}"))),
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

/// The indices of the items whose `text` contains the filter (any case). The text is only
/// made while there's a filter.
fn filtered<T>(filter: &str, items: &[T], text: impl Fn(&T) -> String) -> Vec<usize> {
    let needle = filter.to_lowercase();
    items.iter().enumerate().filter(|(_, x)| needle.is_empty() || text(x).to_lowercase().contains(&needle)).map(|(i, _)| i).collect()
}

#[cfg(test)]
mod fuzz;
#[cfg(test)]
pub mod tests;
