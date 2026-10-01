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
    }
}
