use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::keys::Binding;
use crate::theme::{LEGACY_KEYS, ThemeDef, ThemeSetting};

pub const DEFAULT_CONFIG: &str = include_str!("../config.example.toml");

#[derive(Debug, Deserialize)]
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
    /// the oldest dead, unwatched ones go. 0 keeps everything.
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
    /// `[[filter]]`: hide or highlight threads and posts.
    #[serde(default, rename = "filter")]
    pub filters: Vec<crate::filter::FilterConfig>,
    #[serde(rename = "site")]
    pub sites: Vec<SiteConfig>,
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum BoardConfig {
    Uri(String),
    Full {
        uri: String,
        #[serde(default)]
        title: String,
    },
}

/// Change a config file (the user's is `Config::path()`), keeping its comments and layout.
/// A missing file is first created from the default config.
pub fn edit_at(path: &Path, f: impl FnOnce(&mut DocumentMut)) -> Result<()> {
    try_edit_at(path, |d| {
        f(d);
        Ok(())
    })
}

/// `edit_at`, with an edit that can refuse (nothing is written then).
pub fn try_edit_at(path: &Path, f: impl FnOnce(&mut DocumentMut) -> Result<()>) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_CONFIG.to_string(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut doc: DocumentMut = text.parse().with_context(|| format!("parsing {}", path.display()))?;
    f(&mut doc)?;
    // Don't write something ck itself couldn't read back.
    let out = doc.to_string();
    toml::from_str::<Config>(&out).with_context(|| format!("the edited {} wouldn't load", path.display()))?;
    crate::store::write_atomic(path, out.as_bytes())
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

/// Set an action's keys in `[keys]`, or with `None` (the default) remove its entry.
/// A change to the config's `[[filter]]` tables. A change or removal names the table by
/// position and what ck read there, and is refused if the file says otherwise now.
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

    /// Load the user config if it exists, otherwise the built-in defaults.
    pub fn load() -> Result<Self> {
        if let Some(path) = Self::path().filter(|p| p.exists()) {
            let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            return toml::from_str(&text).with_context(|| format!("parsing {}", path.display()));
        }
        toml::from_str(DEFAULT_CONFIG).context("parsing the built-in config")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Binding;
    use crate::theme::ThemeSetting;

    #[test]
    fn default_config_parses() {
        let c: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert!(!c.sites.is_empty());
        assert!(!c.compact_catalog && c.keys.is_empty());
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
        let c: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(matches!(c.theme, Some(ThemeSetting::Name(ref n)) if n == "nord"));
        assert!(c.sites.len() > 5);
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
