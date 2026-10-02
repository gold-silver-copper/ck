//! Watched threads and history, saved as JSON in $XDG_DATA_HOME/ck (~/.local/share/ck).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::model::Board;

const HISTORY_LEN: usize = 100;

/// Identifies a thread across sites.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadKey {
    pub site: String,
    pub board: String,
    pub no: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watched {
    #[serde(flatten)]
    pub key: ThreadKey,
    pub subject: String,
    pub posts: usize,
    /// Highest post number seen while the thread was open; 0 until first known.
    pub last_seen: u64,
    pub unread: usize,
    /// The thread 404'd: archived or deleted. It's no longer refreshed.
    #[serde(default)]
    pub dead: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Visit {
    #[serde(flatten)]
    pub key: ThreadKey,
    pub subject: String,
    pub last_seen: u64,
    /// Unix time of the last visit.
    pub opened: i64,
}

/// UI settings that couldn't be saved in config.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub compact_catalog: Option<bool>,
}

#[derive(Default)]
pub struct Store {
    dir: Option<PathBuf>,
    pub watched: Vec<Watched>,
    /// Most recent first.
    pub history: Vec<Visit>,
    pub settings: Settings,
}

/// A site's fetched board list, saved so the next start can show it at once.
#[derive(Serialize, Deserialize)]
struct SavedBoards {
    /// Unix time it was fetched.
    fetched: i64,
    boards: Vec<Board>,
}

impl Store {
    pub fn dir() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))?;
        Some(base.join("ck"))
    }

    /// Load everything from `dir`. Problems are returned as messages, never fatal: a corrupt
    /// file is moved aside and ck starts with an empty list.
    pub fn load(dir: Option<PathBuf>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut store = Store { dir, ..Default::default() };
        if let Some(dir) = &store.dir {
            store.watched = load_file(&dir.join("watched.json"), &mut warnings);
            store.history = load_file(&dir.join("history.json"), &mut warnings);
            store.settings = load_file(&dir.join("settings.json"), &mut warnings);
        }
        (store, warnings)
    }

    pub fn save(&self) -> Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        write_atomic(&dir.join("watched.json"), &serde_json::to_vec_pretty(&self.watched)?)?;
        write_atomic(&dir.join("history.json"), &serde_json::to_vec_pretty(&self.history)?)?;
        if self.settings.compact_catalog.is_some() {
            write_atomic(&dir.join("settings.json"), &serde_json::to_vec_pretty(&self.settings)?)?;
        }
        Ok(())
    }

    fn boards_path(&self, site: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join("boards").join(format!("{}.json", crate::download::sanitize(site))))
    }

    /// A saved board list and when it was fetched. Unreadable or corrupt files count as none.
    pub fn load_boards(&self, site: &str) -> Option<(Vec<Board>, i64)> {
        let text = std::fs::read_to_string(self.boards_path(site)?).ok()?;
        let saved: SavedBoards = serde_json::from_str(&text).ok()?;
        Some((saved.boards, saved.fetched))
    }

    pub fn save_boards(&self, site: &str, boards: &[Board], now: i64) -> Result<()> {
        let Some(path) = self.boards_path(site) else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let saved = SavedBoards { fetched: now, boards: boards.to_vec() };
        write_atomic(&path, &serde_json::to_vec(&saved)?)
    }

    pub fn watched(&self, key: &ThreadKey) -> Option<&Watched> {
        self.watched.iter().find(|w| &w.key == key)
    }

    pub fn watched_mut(&mut self, key: &ThreadKey) -> Option<&mut Watched> {
        self.watched.iter_mut().find(|w| &w.key == key)
    }

    /// Start or stop watching; returns whether it's watched now.
    pub fn toggle_watch(&mut self, key: ThreadKey, subject: String, posts: usize, last_seen: u64) -> bool {
        if let Some(i) = self.watched.iter().position(|w| w.key == key) {
            self.watched.remove(i);
            return false;
        }
        self.watched.push(Watched { key, subject, posts, last_seen, unread: 0, dead: false });
        true
    }

    /// The highest post number seen on the previous visit, from Watched or History.
    pub fn last_seen(&self, key: &ThreadKey) -> u64 {
        let w = self.watched(key).map_or(0, |w| w.last_seen);
        let h = self.history.iter().find(|v| &v.key == key).map_or(0, |v| v.last_seen);
        w.max(h)
    }

    /// Record a visit: move it to the front of the history and mark everything seen.
    pub fn visit(&mut self, key: &ThreadKey, subject: &str, posts: usize, max_no: u64, now: i64) {
        self.history.retain(|v| &v.key != key);
        let visit = Visit { key: key.clone(), subject: subject.to_string(), last_seen: max_no, opened: now };
        self.history.insert(0, visit);
        self.history.truncate(HISTORY_LEN);
        if let Some(w) = self.watched_mut(key) {
            w.subject = subject.to_string();
            w.posts = posts;
            w.last_seen = w.last_seen.max(max_no);
            w.unread = 0;
            w.dead = false;
        }
    }
}

fn load_file<T: DeserializeOwned + Default>(path: &Path, warnings: &mut Vec<String>) -> T {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return T::default(),
        Err(e) => {
            warnings.push(format!("Couldn't read {}: {e}", path.display()));
            return T::default();
        }
    };
    match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            // Keep the broken file for the user instead of overwriting it on the next save.
            let aside = path.with_extension("json.corrupt");
            let _ = std::fs::rename(path, &aside);
            warnings.push(format!("{} was corrupt ({e}); moved it to {}", path.display(), aside.display()));
            T::default()
        }
    }
}

/// Write to a temp file in the same directory, then rename over the target.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(no: u64) -> ThreadKey {
        ThreadKey { site: "4chan".into(), board: "g".into(), no }
    }

    #[test]
    fn roundtrip_and_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, warnings) = Store::load(Some(dir.path().to_path_buf()));
        assert!(warnings.is_empty() && s.watched.is_empty());
        assert!(s.toggle_watch(key(1), "one".into(), 5, 105));
        s.visit(&key(2), "two", 3, 203, 1000);
        s.save().unwrap();
        assert!(!dir.path().join("watched.json.tmp").exists());

        let (s2, _) = Store::load(Some(dir.path().to_path_buf()));
        assert_eq!(s2.watched[0].key, key(1));
        assert_eq!(s2.last_seen(&key(1)), 105);
        assert_eq!(s2.last_seen(&key(2)), 203);

        std::fs::write(dir.path().join("history.json"), "{not json").unwrap();
        let (s3, warnings) = Store::load(Some(dir.path().to_path_buf()));
        assert_eq!(warnings.len(), 1);
        assert!(s3.history.is_empty() && s3.watched.len() == 1);
        assert!(dir.path().join("history.json.corrupt").exists());
    }

    #[test]
    fn saved_board_lists() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = Store::load(Some(dir.path().to_path_buf()));
        assert!(s.load_boards("endchan").is_none());
        let boards = vec![Board { uri: "b".into(), title: "Random".into(), nsfw: Some(true) }];
        s.save_boards("endchan", &boards, 1234).unwrap();
        let (got, fetched) = s.load_boards("endchan").unwrap();
        assert_eq!((got[0].uri.as_str(), got[0].nsfw, fetched), ("b", Some(true), 1234));
        // Odd site names stay inside the boards directory; corrupt files count as none.
        s.save_boards("../x", &boards, 1).unwrap();
        assert!(dir.path().join("boards").join("_x.json").exists());
        std::fs::write(dir.path().join("boards").join("endchan.json"), "{nope").unwrap();
        assert!(s.load_boards("endchan").is_none());
    }

    #[test]
    fn history_is_recent_first_and_bounded() {
        let mut s = Store::default();
        for no in 0..(HISTORY_LEN as u64 + 10) {
            s.visit(&key(no), "", 1, no, no as i64);
        }
        s.visit(&key(5), "", 1, 5, 9999);
        assert_eq!(s.history.len(), HISTORY_LEN);
        assert_eq!(s.history[0].key, key(5));
        assert_eq!(s.history.iter().filter(|v| v.key == key(5)).count(), 1);
    }

    #[test]
    fn toggle_and_visit_clear_unread() {
        let mut s = Store::default();
        s.toggle_watch(key(1), "x".into(), 10, 110);
        s.watched_mut(&key(1)).unwrap().unread = 4;
        s.visit(&key(1), "x", 14, 114, 0);
        assert_eq!((s.watched[0].unread, s.watched[0].last_seen, s.watched[0].posts), (0, 114, 14));
        assert!(!s.toggle_watch(key(1), String::new(), 0, 0));
        assert!(s.watched.is_empty());
    }
}
