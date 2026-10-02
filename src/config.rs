use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

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
    /// Key overrides: `action = "key"`.
    #[serde(default)]
    pub keys: HashMap<String, String>,
    #[serde(default)]
    pub theme: crate::theme::ThemeConfig,
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

/// Save `compact_catalog` into the user's config file, keeping its comments and layout.
/// `Ok(false)` when there's no config file to save it in.
pub fn save_compact(value: bool) -> Result<bool> {
    let Some(path) = Config::path().filter(|p| p.exists()) else { return Ok(false) };
    save_compact_to(&path, value)?;
    Ok(true)
}

fn save_compact_to(path: &std::path::Path, value: bool) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc: toml_edit::DocumentMut = text.parse().with_context(|| format!("parsing {}", path.display()))?;
    doc["compact_catalog"] = toml_edit::value(value);
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string()).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
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

    #[test]
    fn saving_compact_keeps_comments_and_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# keep me\nimages = \"off\"\n\n[[site]]\nname = \"x\" # and me\nkind = \"4chan\"\n").unwrap();
        super::save_compact_to(&path, true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me") && text.contains("# and me"), "{text}");
        let c: super::Config = toml::from_str(&text).unwrap();
        assert!(c.compact_catalog);
        assert_eq!(c.sites.len(), 1);
        super::save_compact_to(&path, false).unwrap();
        let c: super::Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!c.compact_catalog);
    }
}
