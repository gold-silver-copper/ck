use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::keys::Binding;
use crate::theme::{LEGACY_KEYS, ThemeDef, ThemeSetting};

pub const DEFAULT_CONFIG: &str = include_str!("../config.example.toml");

/// Unknown keys are errors, here and in `[[site]]` and `[[image_search]]`: a misspelled
/// setting would otherwise do nothing, without a word.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub images: ImagesMode,
    /// Seconds between background refreshes of the open thread (at least 10).
    #[serde(default = "default_refresh_thread")]
    pub refresh_thread_secs: u64,
    /// Seconds between background refreshes of each watched thread (at least 60).
    #[serde(default = "default_refresh_watched")]
    pub refresh_watched_secs: u64,
    /// Saved copies of threads (see the Saved view) are kept to this many megabytes: past it,
    /// the oldest ones of threads not watched go. 0 keeps everything.
    #[serde(default = "default_saved_max_mb")]
    pub saved_max_mb: u64,
    /// The last copy of each catalog and thread opened is kept (in the cache directory) to
    /// show at once next time, up to this many megabytes. 0 keeps none.
    #[serde(default = "default_page_cache_mb")]
    pub page_cache_mb: u64,
    /// While reading down a thread, the selected post is kept this far (a fraction of the
    /// screen, 0 to 0.5) from the screen's top and bottom.
    #[serde(default = "default_scroll_margin")]
    pub scroll_margin: f32,
    /// Sites left off the home screen (by name).
    #[serde(default)]
    pub hidden_sites: Vec<String>,
    /// Favorite boards, `site/board`, shown at the top of the home screen.
    #[serde(default)]
    pub favorites: Vec<String>,
    /// Start where the last run left off.
    #[serde(default = "default_true")]
    pub restore_session: bool,
    /// Images on boards the site marks NSFW (a board's own setting comes first).
    #[serde(default)]
    pub nsfw_images: NsfwImages,
    /// Reading the end of a thread, new posts from a refresh come into view.
    #[serde(default = "default_true")]
    pub follow_new_posts: bool,
    /// Each refresh of a thread that brings nothing new waits longer for the next.
    #[serde(default = "default_true")]
    pub refresh_backoff: bool,
    /// The terminal's title says where ck is and what's new.
    #[serde(default = "default_true")]
    pub set_title: bool,
    /// Threads you watch come first in catalogs (after those a `top` filter puts first), then
    /// the rest in the sort's order.
    #[serde(default)]
    pub watched_first: bool,
    /// Catalog layout (cycled with `c`).
    #[serde(default)]
    pub catalog_layout: Option<CatalogLayout>,
    /// ck 0.2: `true` for the compact layout.
    #[serde(default)]
    pub compact_catalog: bool,
    /// Where `d`/`D` save files; `{site}`, `{board}` and `{thread}` are filled in.
    #[serde(default)]
    pub download_dir: Option<String>,
    /// Key overrides: `action = "key"` or `action = ["key", "key"]`.
    #[serde(default)]
    pub keys: HashMap<String, Binding>,
    /// A theme name (or, from ck 0.2, a table of color overrides).
    #[serde(default)]
    pub theme: Option<ThemeSetting>,
    /// Custom themes: `[themes.NAME]`.
    #[serde(default)]
    pub themes: BTreeMap<String, ThemeDef>,
    #[serde(default)]
    pub color: ColorMode,
    /// Tell about new posts in watched threads: a desktop notification, the bell, or nothing.
    #[serde(default)]
    pub notify: crate::notify::NotifyMode,
    /// A command to notify with instead, `{title}` and `{body}` filled in.
    #[serde(default)]
    pub notify_command: Option<Vec<String>>,
    /// `[[image_search]]`: reverse image search engines for `R` (default: a few well-known ones).
    #[serde(default, rename = "image_search")]
    pub image_search: Vec<ImageSearch>,
    /// Posts and threads with any of these words are hidden, everywhere.
    #[serde(default)]
    pub hidden_words: Vec<String>,
    /// In a thread, replies to a hidden post (and theirs) are hidden too.
    #[serde(default)]
    pub recursive_hiding: bool,
    /// Where ck-web, the browser helper for posting to 4chan, is, if you built your own
    /// (default: the one ck downloads, then next to ck, then on the PATH).
    #[serde(default)]
    pub web_helper: Option<String>,
    /// `[[filter]]`: hide or highlight threads and posts.
    #[serde(default, rename = "filter")]
    pub filters: Vec<crate::filter::FilterConfig>,
    /// `[[site]]`: sites you added, or built-in ones you changed (by name).
    #[serde(default, rename = "site")]
    pub sites: Vec<SiteConfig>,
    /// `false`: only the sites in this file, none of the built-in ones.
    #[serde(default = "default_true")]
    pub default_sites: bool,
}

/// A path from the config or typed in, with `~/` for the home directory.
pub fn expand_home(path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => std::path::PathBuf::from(path),
    }
}

fn default_true() -> bool {
    true
}

fn default_refresh_thread() -> u64 {
    10
}

fn default_refresh_watched() -> u64 {
    60
}

fn default_saved_max_mb() -> u64 {
    500
}

fn default_page_cache_mb() -> u64 {
    100
}

fn default_scroll_margin() -> f32 {
    0.3
}

/// Catalog sort orders, cycled with `s`. Saved under their labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Sort {
    /// The site's order (by last bump).
    #[default]
    #[serde(rename = "bump order")]
    Bump,
    #[serde(rename = "most replies")]
    Replies,
    #[serde(rename = "newest")]
    Newest,
    #[serde(rename = "oldest")]
    Oldest,
}

cycle!(Sort { Bump => "bump order", Replies => "most replies", Newest => "newest", Oldest => "oldest" });

/// A reverse image search engine: `{url}` in `url` becomes the file's (encoded) URL.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImageSearch {
    pub name: String,
    pub url: String,
}

impl ImageSearch {
    pub fn defaults() -> Vec<Self> {
        [
            ("SauceNAO", "https://saucenao.com/search.php?url={url}"),
            ("Google Lens", "https://lens.google.com/uploadbyurl?url={url}"),
            ("Yandex", "https://yandex.com/images/search?rpt=imageview&url={url}"),
            ("IQDB", "https://iqdb.org/?url={url}"),
        ]
        .into_iter()
        .map(|(name, url)| Self { name: name.into(), url: url.into() })
        .collect()
    }

    /// The search page for an image.
    pub fn link(&self, image: &str) -> String {
        self.url.replace("{url}", &crate::http::encode_segment(image))
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CatalogLayout {
    /// A card per thread, with a thumbnail and the start of the OP.
    #[default]
    Cards,
    /// A line per thread.
    Compact,
    /// Thumbnails in columns.
    Grid,
}

cycle!(CatalogLayout { Cards => "cards", Compact => "compact", Grid => "grid" });

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImagesMode {
    /// What the terminal says it can show.
    #[default]
    Auto,
    /// One protocol, whatever the terminal says.
    Halfblocks,
    Sixel,
    Kitty,
    Iterm2,
    Off,
}

cycle!(ImagesMode { Auto => "auto", Halfblocks => "halfblocks", Sixel => "sixel", Kitty => "kitty", Iterm2 => "iterm2", Off => "off" });

/// Images on boards the site marks NSFW, unless a board has its own setting.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NsfwImages {
    #[default]
    Show,
    Off,
}

cycle!(NsfwImages { Show => "show", Off => "off" });

/// Color depth: 24-bit if the terminal says it supports it, or forced either way.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    #[default]
    Auto,
    Truecolor,
    #[serde(rename = "256")]
    Ansi256,
}

cycle!(ColorMode { Auto => "auto", Truecolor => "truecolor", Ansi256 => "256" });

impl ColorMode {
    pub fn truecolor(self) -> bool {
        match self {
            ColorMode::Auto => crate::theme::truecolor_terminal(),
            ColorMode::Truecolor => true,
            ColorMode::Ansi256 => false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SiteConfig {
    pub name: String,
    pub kind: SiteKind,
    /// Base URL. Optional for 4chan.
    #[serde(default)]
    pub url: Option<String>,
    /// Board list override. Required for vichan sites, which have no board-list API.
    #[serde(default)]
    pub boards: Option<Vec<BoardConfig>>,
    /// vichan only: the site's fixed thumbnail extension (e.g. "png"), if it has one.
    #[serde(default)]
    pub thumb_ext: Option<String>,
    /// Name of another configured site (a FoolFuuka archive) to offer when a thread 404s.
    #[serde(default)]
    pub archive: Option<String>,
    /// Base URL for files, when the site serves them from another host (makaba, vichan).
    #[serde(default)]
    pub media_url: Option<String>,
}

/// The engines, described in `backend::ENGINES`. (This order is the one serde lists them in
/// when a config names another.)
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SiteKind {
    #[serde(alias = "4chan")]
    Fourchan,
    /// vichan / tinyboard / infinity / lainchan: 4chan-compatible JSON.
    Vichan,
    Lynxchan,
    /// FoolFuuka 4chan archives (desuarchive, ...).
    Foolfuuka,
    Jschan,
    /// 2ch.hk's engine.
    Makaba,
}

impl SiteKind {
    /// As written in the config.
    pub fn as_str(self) -> &'static str {
        self.engine().name
    }

    /// The engine's name, for people.
    pub fn label(self) -> &'static str {
        self.engine().label
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum BoardConfig {
    Uri(String),
    Full {
        uri: String,
        #[serde(default)]
        title: String,
    },
}

impl BoardConfig {
    pub fn uri(&self) -> &str {
        match self {
            BoardConfig::Uri(uri) | BoardConfig::Full { uri, .. } => uri,
        }
    }
}

/// Change a config file (the user's is `Config::path()`), keeping its comments and layout.
/// A missing file is first created from the default config. An edit that refuses writes nothing.
pub fn try_edit_at(path: &Path, f: impl FnOnce(&mut DocumentMut) -> Result<()>) -> Result<()> {
    let mut doc: DocumentMut = match std::fs::read_to_string(path) {
        Ok(text) => text.parse().with_context(|| format!("parsing {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fresh(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    f(&mut doc)?;
    // Don't write something ck itself couldn't read back.
    let out = doc.to_string();
    toml::from_str::<Config>(&out).with_context(|| format!("the edited {} wouldn't load", path.display()))?;
    crate::atomic::write(path, out.as_bytes())
}

/// Make `theme = name` the active theme. A ck 0.2 `[theme]` table is kept as
/// `[themes.legacy]`, so its colors aren't lost.
pub fn set_theme(doc: &mut DocumentMut, name: &str) {
    if let Some(old) = doc.get("theme").and_then(Item::as_table).cloned() {
        let mut legacy = Table::new();
        for (k, v) in old.iter() {
            let role = LEGACY_KEYS.iter().find(|(l, _)| *l == k).map_or(k, |(_, r)| *r);
            legacy.insert(role, v.clone());
        }
        doc.remove("theme");
        if let Some(themes) = themes_table(doc) {
            themes.insert("legacy", Item::Table(legacy));
        }
    }
    doc["theme"] = value(name);
}

/// Set (or with `None`, remove) one role's color in `[themes.NAME]`, creating the table with
/// `base = base` if it doesn't exist.
pub fn set_theme_color(doc: &mut DocumentMut, name: &str, base: Option<&str>, role: &str, color: Option<&str>) {
    let Some(themes) = themes_table(doc) else { return };
    let mut new = Table::new();
    if let Some(b) = base {
        new.insert("base", value(b));
    }
    let Some(t) = themes.entry(name).or_insert(Item::Table(new)).as_table_mut() else { return };
    match color {
        Some(c) => t[role] = value(c),
        None => {
            t.remove(role);
        }
    }
}

/// A change to the config's `[[filter]]` tables. A change or removal names the table by
/// position and what ck read there, and is refused if the file says otherwise now.
#[derive(Clone, Copy)]
pub enum FilterEdit<'a> {
    Add(&'a crate::filter::FilterConfig),
    Change(usize, &'a crate::filter::FilterConfig, &'a crate::filter::FilterConfig),
    Remove(usize, &'a crate::filter::FilterConfig),
}

pub fn edit_filters(doc: &mut DocumentMut, edit: FilterEdit) -> Result<()> {
    use crate::filter::FilterConfig;
    // The first filter goes at the very end, after the comments there (they'd read as its
    // own otherwise); it takes them along as its prefix.
    let mut moved = None;
    if doc.get("filter").is_none() {
        doc.insert("filter", Item::ArrayOfTables(Default::default()));
        moved = doc.trailing().as_str().map(String::from).filter(|t| !t.trim().is_empty());
        if moved.is_some() {
            doc.set_trailing("");
        }
    }
    let tables = doc.get_mut("filter").and_then(Item::as_array_of_tables_mut).context("`filter` in the config isn't a list of [[filter]] tables")?;
    let check = |tables: &toml_edit::ArrayOfTables, i: usize, old: &FilterConfig| -> Result<()> {
        let now = tables.get(i).map(|t| DocumentMut::from(t.clone()).to_string()).and_then(|text| toml::from_str::<FilterConfig>(&text).ok());
        anyhow::ensure!(now.as_ref() == Some(old), "filter #{} in the config changed since ck read it (restart ck to edit it here)", i + 1);
        Ok(())
    };
    match edit {
        FilterEdit::Add(new) => {
            let mut t = Table::new();
            new.write(&mut t, None);
            if let Some(m) = moved {
                t.decor_mut().set_prefix(format!("{m}\n"));
            }
            tables.push(t);
        }
        FilterEdit::Change(i, old, new) => {
            check(tables, i, old)?;
            if let Some(t) = tables.get_mut(i) {
                new.write(t, Some(old));
            }
        }
        FilterEdit::Remove(i, old) => {
            check(tables, i, old)?;
            let at = tables.get(i).and_then(Table::position);
            let comments = tables.get(i).and_then(|t| t.decor().prefix()?.as_str().map(String::from)).filter(|p| p.contains('#'));
            tables.remove(i);
            if let Some(c) = comments {
                keep_comments(doc, &c, at);
            }
        }
    }
    if doc.get("filter").and_then(Item::as_array_of_tables).is_some_and(|t| t.is_empty()) {
        doc.remove("filter");
    }
    Ok(())
}

/// Comments that were above a removed table stay: above the table after it (by position),
/// or at the end of the file.
fn keep_comments(doc: &mut DocumentMut, comments: &str, after: Option<isize>) {
    let mut next: Option<&mut Table> = None;
    for (_, item) in doc.iter_mut() {
        let tables: Vec<&mut Table> = match item {
            Item::Table(t) => vec![t],
            Item::ArrayOfTables(a) => a.iter_mut().collect(),
            _ => continue,
        };
        for t in tables {
            let later = matches!((t.position(), after), (Some(p), Some(a)) if p > a);
            if later && next.as_ref().is_none_or(|n| n.position() > t.position()) {
                next = Some(t);
            }
        }
    }
    match next {
        Some(t) => {
            let rest = t.decor().prefix().and_then(|p| p.as_str()).unwrap_or("").to_string();
            t.decor_mut().set_prefix(format!("{comments}{rest}"));
        }
        None => {
            let trailing = doc.trailing().as_str().unwrap_or("").to_string();
            // Undoing a first filter leaves the end as it was.
            let comments = comments.strip_suffix('\n').filter(|c| c.ends_with('\n') && trailing.is_empty()).unwrap_or(comments);
            doc.set_trailing(format!("{comments}{trailing}"));
        }
    }
}

/// A new config file: the default config without its `[[site]]` tables, so the built-in
/// sites aren't frozen in it (they're added when it loads, and can change with ck).
pub fn fresh() -> DocumentMut {
    let mut doc: DocumentMut = DEFAULT_CONFIG.parse().unwrap_or_default();
    let tables = doc.get("site").and_then(Item::as_array_of_tables);
    // Comments above the first site belong to the setting before it; the rest are about sites.
    let first = tables.and_then(|t| t.get(0)).and_then(|t| t.decor().prefix()?.as_str().map(String::from));
    let last = tables.and_then(|t| t.iter().filter_map(Table::position).max());
    doc.remove("site");
    if let Some(c) = first.filter(|c| c.contains('#')) {
        keep_comments(&mut doc, &format!("{}\n", c.trim_end()), last);
    }
    doc
}

/// The built-in sites, as `[[site]]` tables (to copy one and change it).
pub fn builtin_sites_text() -> String {
    let doc: DocumentMut = DEFAULT_CONFIG.parse().unwrap_or_default();
    let mut out = DocumentMut::new();
    if let Some(mut sites) = doc.get("site").and_then(Item::as_array_of_tables).cloned() {
        // The first one's comments are about the setting before it.
        if let Some(t) = sites.get_mut(0) {
            t.decor_mut().set_prefix("");
        }
        out.insert("site", Item::ArrayOfTables(sites));
    }
    out.to_string().trim_start().to_string()
}

/// The built-in sites.
pub fn builtin_sites() -> &'static [SiteConfig] {
    static SITES: std::sync::LazyLock<Vec<SiteConfig>> =
        std::sync::LazyLock::new(|| toml::from_str::<Config>(DEFAULT_CONFIG).map(|c| c.sites).unwrap_or_default());
    &SITES
}

/// The built-in sites in their order, each replaced by yours of the same name, then the
/// ones you added.
pub fn merge_sites(mine: Vec<SiteConfig>, builtin: Vec<SiteConfig>) -> Vec<SiteConfig> {
    let same = |a: &SiteConfig, b: &SiteConfig| a.name.eq_ignore_ascii_case(&b.name);
    let mut mine: Vec<Option<SiteConfig>> = mine.into_iter().map(Some).collect();
    let mut out: Vec<SiteConfig> = builtin
        .into_iter()
        .map(|b| mine.iter_mut().find(|m| m.as_ref().is_some_and(|m| same(m, &b))).and_then(Option::take).unwrap_or(b))
        .collect();
    out.extend(mine.into_iter().flatten());
    out
}

/// A site as a `[[site]]` table.
fn site_table(site: &SiteConfig) -> Table {
    let mut t = Table::new();
    t.insert("name", value(site.name.as_str()));
    t.insert("kind", value(site.kind.as_str()));
    let optional = [("url", &site.url), ("thumb_ext", &site.thumb_ext), ("media_url", &site.media_url), ("archive", &site.archive)];
    for (key, v) in optional {
        if let Some(v) = v {
            t.insert(key, value(v.as_str()));
        }
    }
    if let Some(boards) = &site.boards {
        t.insert("boards", value(boards_array(boards)));
    }
    t
}

/// `["b"]`, or one board a line, as the built-in sites have them.
fn boards_array(boards: &[BoardConfig]) -> toml_edit::Array {
    let mut a = toml_edit::Array::new();
    for b in boards {
        match b {
            BoardConfig::Uri(uri) => a.push(uri.as_str()),
            BoardConfig::Full { uri, title } if title.is_empty() => a.push(uri.as_str()),
            BoardConfig::Full { uri, title } => {
                let mut t = toml_edit::InlineTable::new();
                t.insert("uri", uri.as_str().into());
                t.insert("title", title.as_str().into());
                a.push(t);
            }
        }
    }
    if a.len() > 1 {
        for v in a.iter_mut() {
            v.decor_mut().set_prefix("\n  ");
        }
        a.set_trailing_comma(true);
        a.set_trailing("\n");
    }
    a
}

/// Add a `[[site]]` (after the comments at the end, when it's the first).
pub fn add_site(doc: &mut DocumentMut, site: &SiteConfig) -> Result<()> {
    let mut moved = None;
    if doc.get("site").is_none() {
        doc.insert("site", Item::ArrayOfTables(Default::default()));
        moved = doc.trailing().as_str().map(String::from).filter(|t| !t.trim().is_empty());
        if moved.is_some() {
            doc.set_trailing("");
        }
    }
    let tables = doc.get_mut("site").and_then(Item::as_array_of_tables_mut).context("`site` in the config isn't a list of [[site]] tables")?;
    let mut t = site_table(site);
    if let Some(m) = moved {
        t.decor_mut().set_prefix(format!("{m}\n"));
    }
    tables.push(t);
    Ok(())
}

/// Give a site these boards: in its `[[site]]` table, or (a built-in site) in a new one.
pub fn set_site_boards(doc: &mut DocumentMut, site: &SiteConfig, boards: &[BoardConfig]) -> Result<()> {
    let mine = doc
        .get_mut("site")
        .and_then(Item::as_array_of_tables_mut)
        .and_then(|ts| ts.iter_mut().find(|t| t.get("name").and_then(Item::as_str).is_some_and(|n| n.eq_ignore_ascii_case(&site.name))));
    match mine {
        Some(t) => {
            t["boards"] = value(boards_array(boards));
            Ok(())
        }
        None => add_site(doc, &SiteConfig { boards: Some(boards.to_vec()), ..site.clone() }),
    }
}

/// Remove `[[site]]` number `i`, which ck read as `old` (refused if the file says otherwise
/// now). Its comments stay.
pub fn remove_site(doc: &mut DocumentMut, i: usize, old: &SiteConfig) -> Result<()> {
    let tables = doc.get_mut("site").and_then(Item::as_array_of_tables_mut).context("the config has no [[site]] tables")?;
    let now = tables.get(i).map(|t| DocumentMut::from(t.clone()).to_string()).and_then(|text| toml::from_str::<SiteConfig>(&text).ok());
    anyhow::ensure!(now.as_ref() == Some(old), "site #{} in the config changed since ck read it (restart ck to remove it here)", i + 1);
    let at = tables.get(i).and_then(Table::position);
    let comments = tables.get(i).and_then(|t| t.decor().prefix()?.as_str().map(String::from)).filter(|p| p.contains('#'));
    tables.remove(i);
    if tables.is_empty() {
        doc.remove("site");
    }
    if let Some(c) = comments {
        keep_comments(doc, &c, at);
    }
    Ok(())
}

/// The `[[site]]` tables of a config file, as ck reads them (none if it doesn't exist).
pub fn sites_in(path: &Path) -> Result<Vec<SiteConfig>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(toml::from_str::<Config>(&text).with_context(|| format!("parsing {}", path.display()))?.sites),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Set `hidden_words` (taken out when there are none).
pub fn set_hidden_words(doc: &mut DocumentMut, words: &[String]) {
    if words.is_empty() {
        doc.remove("hidden_words");
    } else {
        doc["hidden_words"] = value(words.iter().map(String::as_str).collect::<toml_edit::Array>());
    }
}

/// Set an action's keys in `[keys]`, or with `None` (the default) remove its entry.
pub fn set_key(doc: &mut DocumentMut, action: &str, binding: Option<&Binding>) {
    if !doc.contains_key("keys") {
        doc.insert("keys", Item::Table(Table::new()));
    }
    let Some(keys) = doc["keys"].as_table_mut() else { return };
    match binding {
        Some(Binding::One(k)) => keys[action] = value(k.as_str()),
        Some(Binding::Many(v)) => keys[action] = value(v.iter().map(String::as_str).collect::<toml_edit::Array>()),
        None => {
            keys.remove(action);
        }
    }
}

fn themes_table(doc: &mut DocumentMut) -> Option<&mut Table> {
    let mut implicit = Table::new();
    implicit.set_implicit(true);
    doc.entry("themes").or_insert(Item::Table(implicit)).as_table_mut()
}

impl Config {
    /// The layout to start with: `catalog_layout`, or ck 0.2's `compact_catalog`.
    pub fn layout(&self) -> CatalogLayout {
        self.catalog_layout.unwrap_or(if self.compact_catalog { CatalogLayout::Compact } else { CatalogLayout::Cards })
    }

    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;
        Some(base.join("ck").join("config.toml"))
    }

    /// Load the user config if it exists, otherwise the built-in defaults. Its sites come
    /// with the built-in ones (see `merge_sites`).
    pub fn load() -> Result<Self> {
        let Some(path) = Self::path().filter(|p| p.exists()) else {
            return toml::from_str(DEFAULT_CONFIG).context("parsing the built-in config");
        };
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.with_builtin_sites().with_context(|| format!("in {}", path.display()))
    }

    /// The sites to show: the built-in ones and this config's (unless `default_sites = false`).
    pub fn with_builtin_sites(mut self) -> Result<Self> {
        if self.default_sites {
            self.sites = merge_sites(std::mem::take(&mut self.sites), builtin_sites().to_vec());
        }
        anyhow::ensure!(!self.sites.is_empty(), "no sites: add a [[site]], or take out `default_sites = false`");
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Binding;
    use crate::theme::ThemeSetting;

    fn edit_at(path: &Path, f: impl FnOnce(&mut DocumentMut)) -> Result<()> {
        try_edit_at(path, |d| {
            f(d);
            Ok(())
        })
    }

    #[test]
    fn default_config_parses() {
        let c: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert!(!c.sites.is_empty());
        assert!(!c.compact_catalog && c.keys.is_empty());
    }

    #[test]
    fn misspelled_settings_are_errors() {
        for (text, key) in [("colour = \"256\"", "colour"), ("[[site]]\nname = \"x\"\nkind = \"vichan\"\nurll = \"https://x\"", "urll"), ("[[image_search]]\nname = \"x\"\nlink = \"y\"", "link")] {
            let e = toml::from_str::<Config>(text).unwrap_err().to_string();
            assert!(e.contains(&format!("unknown field `{key}`")), "{e}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn edits_keep_a_linked_config_and_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (real, link) = (dir.path().join("dotfiles/ck.toml"), dir.path().join("config.toml"));
        std::fs::create_dir(dir.path().join("dotfiles")).unwrap();
        std::fs::write(&real, "[[site]]\nname = \"x\"\nkind = \"4chan\"\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink("dotfiles/ck.toml", &link).unwrap();
        edit_at(&link, |d| d["compact_catalog"] = value(true)).unwrap();
        // The link is still a link, and the file it points to has the edit and its mode.
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(std::fs::read_to_string(&real).unwrap().contains("compact_catalog = true"));
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
        // No temp file is left beside either.
        let names = |d: &Path| {
            let mut names: Vec<_> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
            names.sort();
            names
        };
        assert_eq!(names(&dir.path().join("dotfiles")), ["ck.toml"]);
        assert_eq!(names(dir.path()), ["config.toml", "dotfiles"]);
    }

    #[test]
    fn key_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[[site]]\nname = \"x\"\nkind = \"4chan\"\n\n# my keys\n[keys]\nsort = \"z\"\n").unwrap();
        edit_at(&path, |d| {
            set_key(d, "watch", Some(&Binding::Many(vec!["W".into(), "alt-w".into()])));
            set_key(d, "sort", None);
            set_key(d, "help", Some(&Binding::One("f1".into())));
        })
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my keys"), "{text}");
        let c: Config = toml::from_str(&text).unwrap();
        assert_eq!(c.keys.len(), 2);
        assert_eq!(c.keys["watch"], Binding::Many(vec!["W".into(), "alt-w".into()]));
        assert!(crate::keys::KeyMap::new(&c.keys).is_ok());
    }
    #[test]
    fn edits_keep_comments_and_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# keep me\nimages = \"off\"\n\n[[site]]\nname = \"x\" # and me\nkind = \"4chan\"\n").unwrap();
        edit_at(&path, |d| d["compact_catalog"] = toml_edit::value(true)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me") && text.contains("# and me"), "{text}");
        let c: Config = toml::from_str(&text).unwrap();
        assert!(c.compact_catalog);
        assert_eq!(c.layout(), CatalogLayout::Compact);
        assert_eq!(c.sites.len(), 1);
        edit_at(&path, |d| d["catalog_layout"] = toml_edit::value("grid")).unwrap();
        let c: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(c.layout(), CatalogLayout::Grid);
    }

    #[test]
    fn missing_config_starts_from_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ck").join("config.toml");
        edit_at(&path, |d| set_theme(d, "nord")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let c: Config = toml::from_str(&text).unwrap();
        assert!(matches!(c.theme, Some(ThemeSetting::Name(ref n)) if n == "nord"));
        // Without the built-in sites, which come with it when it loads; the comments before
        // them stay.
        assert!(c.sites.is_empty() && !text.contains("\n[[site]]\n"), "{text}");
        assert!(text.contains("# download_dir = ") && text.contains("[keys]"), "{text}");
        assert_eq!(c.with_builtin_sites().unwrap().sites, builtin_sites());
    }

    #[test]
    fn your_sites_come_with_the_built_in_ones() {
        let parse = |text: &str| toml::from_str::<Config>(text).unwrap().with_builtin_sites();
        let builtin = builtin_sites();
        // None of yours: the built-in ones.
        assert_eq!(parse("").unwrap().sites, builtin);
        // Yours after them; one of the same name instead of it, in its place.
        let c = parse("[[site]]\nname = \"mine\"\nkind = \"jschan\"\nurl = \"https://mine.example\"\n\n[[site]]\nname = \"Lainchan\"\nkind = \"vichan\"\nurl = \"https://lainchan.org\"\nboards = [\"λ\"]\n").unwrap();
        let names: Vec<&str> = c.sites.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names.len(), builtin.len() + 1);
        assert_eq!(names.last(), Some(&"mine"));
        let lain = builtin.iter().position(|s| s.name == "lainchan").unwrap();
        assert_eq!((names[lain], c.sites[lain].boards.as_ref().unwrap().len()), ("Lainchan", 1));
        // A copy of all the defaults (an older config) keeps them as they were.
        assert_eq!(parse(DEFAULT_CONFIG).unwrap().sites, builtin);
        // Only yours.
        let c = parse("default_sites = false\n[[site]]\nname = \"mine\"\nkind = \"4chan\"\n").unwrap();
        assert_eq!(c.sites.len(), 1);
        assert!(parse("default_sites = false\n").unwrap_err().to_string().contains("no sites"));
    }

    #[test]
    fn adding_and_removing_sites() {
        let site = |name: &str, boards: Option<Vec<BoardConfig>>| SiteConfig {
            name: name.into(),
            kind: SiteKind::Vichan,
            url: Some(format!("https://{name}.example")),
            boards,
            thumb_ext: None,
            archive: None,
            media_url: None,
        };
        let mut d = fresh();
        let a = site("a", Some(vec![BoardConfig::Uri("b".into())]));
        add_site(&mut d, &a).unwrap();
        let text = d.to_string();
        assert!(text.ends_with("[[site]]\nname = \"a\"\nkind = \"vichan\"\nurl = \"https://a.example\"\nboards = [\"b\"]\n"), "{text}");
        let c: Config = toml::from_str(&text).unwrap();
        assert_eq!(c.sites, std::slice::from_ref(&a));
        // Another board, written into its table.
        let boards = vec![BoardConfig::Uri("b".into()), BoardConfig::Full { uri: "tech".into(), title: "Tech".into() }];
        set_site_boards(&mut d, &a, &boards).unwrap();
        let c: Config = toml::from_str(&d.to_string()).unwrap();
        assert_eq!(c.sites[0].boards.as_deref(), Some(&boards[..]));
        // A built-in site's board: a [[site]] of its own, which replaces the built-in one.
        let lain = builtin_sites().iter().find(|s| s.name == "lainchan").unwrap().clone();
        set_site_boards(&mut d, &lain, &[BoardConfig::Uri("mega".into())]).unwrap();
        let c: Config = toml::from_str(&d.to_string()).unwrap();
        assert_eq!((c.sites[1].name.as_str(), c.sites[1].thumb_ext.as_deref(), c.sites[1].boards.as_ref().unwrap().len()), ("lainchan", lain.thumb_ext.as_deref(), 1));
        // Removing: refused if the file changed, and the last one takes the list along.
        let a2 = SiteConfig { boards: Some(boards.clone()), ..a.clone() };
        assert!(remove_site(&mut d, 0, &a).unwrap_err().to_string().contains("changed since ck read it"));
        remove_site(&mut d, 0, &a2).unwrap();
        remove_site(&mut d, 0, &c.sites[1]).unwrap();
        assert_eq!(d.to_string(), fresh().to_string());
        // Printing them for copying: tables only.
        assert!(builtin_sites_text().starts_with("[[site]]\nname = \"4chan\""));
    }

    #[test]
    fn theme_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // A ck 0.2 [theme] table becomes [themes.legacy] when another theme is chosen.
        std::fs::write(&path, "[theme]\naccent = \"cyan\"\n\n[[site]]\nname = \"x\"\nkind = \"4chan\"\n").unwrap();
        edit_at(&path, |d| set_theme(d, "gruvbox")).unwrap();
        let c: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(matches!(c.theme, Some(ThemeSetting::Name(ref n)) if n == "gruvbox"));
        assert_eq!(c.themes["legacy"].colors["primary"], "cyan");
        // Colors go in [themes.NAME], created with a base.
        edit_at(&path, |d| {
            set_theme_color(d, "custom", Some("nord"), "primary", Some("#123456"));
            set_theme_color(d, "custom", Some("nord"), "text", Some("white"));
            set_theme_color(d, "custom", Some("nord"), "text", None);
            set_theme(d, "custom");
        })
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[themes.custom]"), "{text}");
        let c: Config = toml::from_str(&text).unwrap();
        let t = &c.themes["custom"];
        assert_eq!((t.base.as_deref(), t.colors.len()), (Some("nord"), 1));
        assert!(crate::theme::from_config(c.theme.as_ref(), &c.themes).is_ok());
    }

    #[test]
    fn edits_that_would_break_the_config_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[[site]]\nname = \"x\"\nkind = \"4chan\"\n").unwrap();
        assert!(edit_at(&path, |d| d["images"] = toml_edit::value("sometimes")).is_err());
        assert!(!std::fs::read_to_string(&path).unwrap().contains("sometimes"));
    }
}


#[cfg(test)]
mod tests_filters {
    use super::*;
    use crate::filter::{Field, FilterConfig};

    #[test]
    fn filter_tables_go_after_the_comments_and_removing_keeps_them() {
        let f = FilterConfig::new("x".into(), &[Field::Name]);
        // The default config ends with comments (under [keys]): the first filter goes after them.
        let mut d: DocumentMut = DEFAULT_CONFIG.parse().unwrap();
        edit_filters(&mut d, FilterEdit::Add(&f)).unwrap();
        let text = d.to_string();
        assert!(text.trim_end().ends_with("[[filter]]\npattern = \"x\"\nfield = \"name\"\naction = \"hide\""), "{text}");
        assert!(text.contains("[keys]\n# watch = \"W\""), "{text}");
        // Taking it back leaves the file as it was.
        edit_filters(&mut d, FilterEdit::Remove(0, &f)).unwrap();
        assert_eq!(d.to_string(), DEFAULT_CONFIG);
        // A removed filter's comments stay, above what came after it.
        let text = "[[filter]]\npattern = \"a\"\n\n# about b\n[[filter]]\npattern = \"b\"\n\n# sites\n[[site]]\nname = \"s\"\n";
        let mut d: DocumentMut = text.parse().unwrap();
        let b: FilterConfig = toml::from_str("pattern = \"b\"").unwrap();
        edit_filters(&mut d, FilterEdit::Remove(1, &b)).unwrap();
        assert_eq!(d.to_string(), "[[filter]]\npattern = \"a\"\n\n# about b\n\n# sites\n[[site]]\nname = \"s\"\n");
    }
}
