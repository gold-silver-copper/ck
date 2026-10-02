use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};


use anyhow::{Context, Result};
use serde::Deserialize;
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
    /// One line per thread in catalogs (toggled with `c`).
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
    /// `[[filter]]`: hide or highlight threads and posts.
    #[serde(default, rename = "filter")]
    pub filters: Vec<crate::filter::FilterConfig>,
    #[serde(rename = "site")]
    pub sites: Vec<SiteConfig>,
}

fn default_refresh_thread() -> u64 {
    10
}

fn default_refresh_watched() -> u64 {
    60
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImagesMode {
    #[default]
    Auto,
    Off,
}

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

impl ColorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ColorMode::Auto => "auto",
            ColorMode::Truecolor => "truecolor",
            ColorMode::Ansi256 => "256",
        }
    }

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
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_CONFIG.to_string(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut doc: DocumentMut = text.parse().with_context(|| format!("parsing {}", path.display()))?;
    f(&mut doc);
    // Don't write something ck itself couldn't read back.
    let out = doc.to_string();
    toml::from_str::<Config>(&out).with_context(|| format!("the edited {} wouldn't load", path.display()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, out).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
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
        themes_table(doc).insert("legacy", Item::Table(legacy));
    }
    doc["theme"] = value(name);
}

/// Set (or with `None`, remove) one role's color in `[themes.NAME]`, creating the table with
/// `base = base` if it doesn't exist.
pub fn set_theme_color(doc: &mut DocumentMut, name: &str, base: Option<&str>, role: &str, color: Option<&str>) {
    let themes = themes_table(doc);
    if !themes.contains_key(name) {
        let mut t = Table::new();
        if let Some(b) = base {
            t.insert("base", value(b));
        }
        themes.insert(name, Item::Table(t));
    }
    let Some(t) = themes[name].as_table_mut() else { return };
    match color {
        Some(c) => t[role] = value(c),
        None => {
            t.remove(role);
        }
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

fn themes_table(doc: &mut DocumentMut) -> &mut Table {
    if !doc.contains_key("themes") {
        let mut t = Table::new();
        t.set_implicit(true);
        doc.insert("themes", Item::Table(t));
    }
    doc["themes"].as_table_mut().expect("themes is a table")
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;
        Some(base.join("ck").join("config.toml"))
    }

    /// Load the user config if it exists, otherwise the built-in defaults.
    pub fn load() -> Result<Self> {
        if let Some(path) = Self::path().filter(|p| p.exists()) {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            return toml::from_str(&text).with_context(|| format!("parsing {}", path.display()));
        }
        Ok(toml::from_str(DEFAULT_CONFIG).expect("built-in config is valid"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_config_parses() {
        let c: super::Config = toml::from_str(super::DEFAULT_CONFIG).unwrap();
        assert!(!c.sites.is_empty());
        assert!(!c.compact_catalog && c.keys.is_empty());
    }

    use super::{Config, edit_at, set_key, set_theme, set_theme_color};
    use crate::keys::Binding;

    #[test]
    fn key_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[[site]]\nname = \"x\"\nkind = \"4chan\"\n\n# my keys\n[keys]\nsort = \"z\"\n").unwrap();
        edit_at(&path, |d| {
            set_key(d, "watch", Some(&Binding::Many(vec!["W".into(), "ctrl-w".into()])));
            set_key(d, "sort", None);
            set_key(d, "help", Some(&Binding::One("f1".into())));
        })
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my keys"), "{text}");
        let c: Config = toml::from_str(&text).unwrap();
        assert_eq!(c.keys.len(), 2);
        assert_eq!(c.keys["watch"], Binding::Many(vec!["W".into(), "ctrl-w".into()]));
        assert!(crate::keys::KeyMap::new(&c.keys).is_ok());
    }
    use crate::theme::ThemeSetting;

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
        assert_eq!(c.sites.len(), 1);
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
