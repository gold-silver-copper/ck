//! Watched threads and history, saved as JSON in $XDG_DATA_HOME/ck (~/.local/share/ck).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::model::Board;

const HISTORY_LEN: usize = 100;
/// Boards remembered as recently opened.
const RECENT_BOARDS: usize = 20;
/// Threads and posts hidden by hand, kept per board; the oldest go first.
const HIDDEN_PER_BOARD: usize = 3000;

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
    /// Posts marked as yours (`m`); replies to them are counted and notified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mine: Vec<u64>,
    /// Unread replies to your posts.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub replies: usize,
    /// Followed as a general: when the thread dies or hits the bump limit, the next thread
    /// whose subject matches this is watched instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub general: Option<String>,
    /// The thread has reached its bump limit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub at_limit: bool,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
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

/// A catalog thread as last seen: when it was last in the catalog, and how many replies it
/// had when it was last opened.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeenThread {
    pub last: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replies: Option<u32>,
}

/// A board's own catalog sort and layout (set with `s` and `c` there).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<crate::config::CatalogLayout>,
}

/// Threads gone from the catalog this long are forgotten.
const SEEN_FOR: i64 = 7 * 24 * 3600;

/// Where you were: one place per tab.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub tabs: Vec<Place>,
    #[serde(default)]
    pub active: usize,
}

/// A view and what's open in it, by name and number (indices change between runs).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// sites, boards, catalog, thread, watched, history.
    pub view: String,
    pub site: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<u64>,
    /// The selected post (thread) or thread (catalog).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filter: String,
}

/// UI settings that couldn't be saved in config.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub compact_catalog: Option<bool>,
    #[serde(default)]
    pub catalog_layout: Option<crate::config::CatalogLayout>,
}

#[derive(Default)]
pub struct Store {
    dir: Option<PathBuf>,
    pub watched: Vec<Watched>,
    /// Most recent first.
    pub history: Vec<Visit>,
    pub settings: Settings,
    /// Thread and post numbers hidden by hand, by `site/board`, oldest first.
    pub hidden: std::collections::BTreeMap<String, Vec<u64>>,
    /// Per-board catalog sort and layout, by `site/board`.
    pub board_prefs: std::collections::BTreeMap<String, BoardPrefs>,
    /// Boards whose catalogs were opened, `site/board`, most recent first.
    pub recent_boards: Vec<String>,
    /// Catalog threads seen, by `site/board`.
    pub seen: std::collections::BTreeMap<String, std::collections::BTreeMap<u64, SeenThread>>,
    /// `seen` changed since it was last written.
    seen_dirty: std::cell::Cell<bool>,
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
            store.hidden = load_file(&dir.join("hidden.json"), &mut warnings);
            store.seen = load_file(&dir.join("seen.json"), &mut warnings);
            store.recent_boards = load_file(&dir.join("recent_boards.json"), &mut warnings);
            store.board_prefs = load_file(&dir.join("board_prefs.json"), &mut warnings);
        }
        (store, warnings)
    }

    pub fn save(&self) -> Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        write_atomic(&dir.join("watched.json"), &serde_json::to_vec_pretty(&self.watched)?)?;
        write_atomic(&dir.join("history.json"), &serde_json::to_vec_pretty(&self.history)?)?;
        if !self.hidden.is_empty() || dir.join("hidden.json").exists() {
            write_atomic(&dir.join("hidden.json"), &serde_json::to_vec(&self.hidden)?)?;
        }
        if !self.recent_boards.is_empty() || dir.join("recent_boards.json").exists() {
            write_atomic(&dir.join("recent_boards.json"), &serde_json::to_vec_pretty(&self.recent_boards)?)?;
        }
        if !self.board_prefs.is_empty() || dir.join("board_prefs.json").exists() {
            write_atomic(&dir.join("board_prefs.json"), &serde_json::to_vec_pretty(&self.board_prefs)?)?;
        }
        if self.seen_dirty.replace(false) {
            write_atomic(&dir.join("seen.json"), &serde_json::to_vec(&self.seen)?)?;
        }
        if self.settings.compact_catalog.is_some() || self.settings.catalog_layout.is_some() {
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

    /// A board's catalog was loaded: remember its threads, and return the ones that weren't
    /// there on the previous load (none on the first).
    pub fn catalog_seen(&mut self, site: &str, board: &str, threads: &[u64], now: i64) -> std::collections::HashSet<u64> {
        let key = format!("{site}/{board}");
        let first = !self.seen.contains_key(&key);
        let map = self.seen.entry(key).or_default();
        let new = if first { Default::default() } else { threads.iter().copied().filter(|no| !map.contains_key(no)).collect() };
        for &no in threads {
            map.entry(no).or_default().last = now;
        }
        map.retain(|_, t| now - t.last <= SEEN_FOR);
        self.seen_dirty.set(true);
        new
    }

    /// A board's catalog was opened: it goes to the front of the recent boards.
    pub fn board_opened(&mut self, site: &str, board: &str) {
        let key = format!("{site}/{board}");
        self.recent_boards.retain(|b| *b != key);
        self.recent_boards.insert(0, key);
        self.recent_boards.truncate(RECENT_BOARDS);
    }

    /// A thread was opened with `replies` replies.
    pub fn opened(&mut self, site: &str, board: &str, no: u64, replies: u32, now: i64) {
        let t = self.seen.entry(format!("{site}/{board}")).or_default().entry(no).or_default();
        t.replies = Some(replies);
        t.last = t.last.max(now);
        self.seen_dirty.set(true);
    }

    /// Replies the thread had when it was last opened.
    pub fn replies_seen(&self, site: &str, board: &str, no: u64) -> Option<u32> {
        self.seen.get(&format!("{site}/{board}"))?.get(&no)?.replies
    }

    pub fn load_session(&self) -> Option<Session> {
        let path = self.dir.as_ref()?.join("session.json");
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    pub fn save_session(&self, session: &Session) -> Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        write_atomic(&dir.join("session.json"), &serde_json::to_vec_pretty(session)?)
    }

    pub fn is_hidden(&self, site: &str, board: &str, no: u64) -> bool {
        self.hidden.get(&format!("{site}/{board}")).is_some_and(|v| v.contains(&no))
    }

    /// Hide a thread or post, or unhide it; returns whether it's hidden now.
    pub fn toggle_hidden(&mut self, site: &str, board: &str, no: u64) -> bool {
        let key = format!("{site}/{board}");
        let list = self.hidden.entry(key.clone()).or_default();
        if let Some(i) = list.iter().position(|&n| n == no) {
            list.remove(i);
            if list.is_empty() {
                self.hidden.remove(&key);
            }
            return false;
        }
        list.push(no);
        if list.len() > HIDDEN_PER_BOARD {
            list.remove(0);
        }
        true
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
        self.watched.push(Watched {
            key,
            subject,
            posts,
            last_seen,
            unread: 0,
            dead: false,
            mine: Vec::new(),
            replies: 0,
            general: None,
            at_limit: false,
        });
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
            w.replies = 0;
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
    fn hidden_threads_and_posts() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        assert!(s.toggle_hidden("4chan", "g", 5));
        assert!(s.is_hidden("4chan", "g", 5) && !s.is_hidden("4chan", "v", 5));
        s.save().unwrap();
        let (mut s, w) = Store::load(Some(dir.path().to_path_buf()));
        assert!(w.is_empty() && s.is_hidden("4chan", "g", 5));
        assert!(!s.toggle_hidden("4chan", "g", 5));
        assert!(s.hidden.is_empty());
        // Bounded per board, oldest out first.
        for no in 0..HIDDEN_PER_BOARD as u64 + 2 {
            s.toggle_hidden("4chan", "g", no);
        }
        assert!(!s.is_hidden("4chan", "g", 0) && !s.is_hidden("4chan", "g", 1) && s.is_hidden("4chan", "g", 2));
    }

    #[test]
    fn recent_boards_are_recent_first_and_bounded() {
        let mut s = Store::default();
        for i in 0..25 {
            s.board_opened("4chan", &format!("b{i}"));
        }
        s.board_opened("4chan", "b10");
        assert_eq!(s.recent_boards.len(), RECENT_BOARDS);
        assert_eq!(s.recent_boards[..2], ["4chan/b10", "4chan/b24"]);
        assert_eq!(s.recent_boards.iter().filter(|b| *b == "4chan/b10").count(), 1);
    }

    #[test]
    fn catalog_threads_seen() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        let day = 24 * 3600;
        // The first visit marks nothing new; later ones mark what wasn't there.
        assert!(s.catalog_seen("4chan", "g", &[1, 2, 3], 0).is_empty());
        assert_eq!(s.catalog_seen("4chan", "g", &[2, 3, 4], day), [4].into());
        s.opened("4chan", "g", 3, 10, day);
        s.save().unwrap();
        let (mut s, w) = Store::load(Some(dir.path().to_path_buf()));
        assert!(w.is_empty());
        assert_eq!((s.replies_seen("4chan", "g", 3), s.replies_seen("4chan", "g", 2)), (Some(10), None));
        // Threads gone for over a week are forgotten (and would count as new again).
        s.catalog_seen("4chan", "g", &[4], 9 * day);
        assert!(!s.seen["4chan/g"].contains_key(&1) && !s.seen["4chan/g"].contains_key(&3));
        assert_eq!(s.catalog_seen("4chan", "g", &[1, 4], 9 * day), [1].into());
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
