//! Watched threads and history, saved as JSON in $XDG_DATA_HOME/ck (~/.local/share/ck).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::model::{Board, Post};
use crate::saved::{self, SavedMeta, SavedPost, SavedThread};

const HISTORY_LEN: usize = 100;
/// Boards remembered as recently opened.
const RECENT_BOARDS: usize = 20;
/// Threads and posts hidden by hand, kept per board; the oldest go first.
const HIDDEN_PER_BOARD: usize = 3000;

/// Identifies a thread across sites.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadKey {
    pub site: String,
    pub board: String,
    pub no: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    pub sort: Option<crate::config::Sort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<crate::config::CatalogLayout>,
    /// Images on this board: its own setting, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<bool>,
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
    pub sort: Option<crate::config::Sort>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filter: String,
    /// The post whose conversation was shown (`c`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<u64>,
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
    /// Threads kept in `threads/` (see `saved`), newest first.
    pub saved: Vec<SavedMeta>,
    /// Past this many bytes, the oldest dead, unwatched copies are removed.
    pub saved_max: u64,
    /// Writes saved copies in the background (with a data directory).
    writer: Option<crate::writer::Writer<Wrote>>,
    /// A hash of each file's content as last read or written, so unchanged files aren't
    /// written again.
    written: std::cell::RefCell<std::collections::HashMap<&'static str, u64>>,
}

/// What a background write of a saved copy came to.
pub enum Wrote {
    /// Written: its size.
    Saved(ThreadKey, u64),
    Failed(String),
    /// Nothing to tell (a removal, or marking a copy that isn't there dead).
    Nothing,
}

/// A cheap fingerprint of a thread's posts, to tell whether they changed: each post's
/// number, name and subject length, files, and how much text it has.
pub fn signature(posts: &[Post]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    posts.len().hash(&mut h);
    for p in posts {
        let text: usize = p.body.iter().map(|l| l.spans.iter().map(|s| s.content.len()).sum::<usize>() + 1).sum();
        (p.no, p.name.len(), p.subject.as_ref().map(String::len), p.files.len(), p.body.len(), text).hash(&mut h);
        for f in &p.files {
            f.url.hash(&mut h);
        }
    }
    h.finish()
}

/// `saved.json`: the list of saved threads.
#[derive(Default, Serialize, Deserialize)]
struct SavedIndex {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    threads: Vec<SavedMeta>,
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
        let writer = dir.is_some().then(|| crate::writer::Writer::new(|what| Wrote::Failed(format!("Couldn't save a copy: ck hit a bug ({what})"))));
        let mut store = Store { dir, writer, ..Default::default() };
        if let Some(dir) = &store.dir {
            store.watched = load_file(&dir.join("watched.json"), &mut warnings);
            store.history = load_file(&dir.join("history.json"), &mut warnings);
            store.settings = load_file(&dir.join("settings.json"), &mut warnings);
            store.hidden = load_file(&dir.join("hidden.json"), &mut warnings);
            store.seen = load_file(&dir.join("seen.json"), &mut warnings);
            store.recent_boards = load_file(&dir.join("recent_boards.json"), &mut warnings);
            store.board_prefs = load_file(&dir.join("board_prefs.json"), &mut warnings);
            store.saved = load_file::<SavedIndex>(&dir.join("saved.json"), &mut warnings).threads;
            // No index (or a broken one): rebuild it from the copies themselves.
            if !dir.join("saved.json").exists() {
                store.saved = saved::scan(dir);
            }
        }
        // What was just read counts as written.
        if let Ok(files) = store.files() {
            store.written.replace(files.iter().map(|(name, bytes)| (*name, hash(bytes))).collect());
        }
        (store, warnings)
    }

    /// Every file with its content.
    fn files(&self) -> Result<[(&'static str, Vec<u8>); 8]> {
        Ok([
            ("watched.json", serde_json::to_vec_pretty(&self.watched)?),
            ("history.json", serde_json::to_vec_pretty(&self.history)?),
            ("hidden.json", serde_json::to_vec(&self.hidden)?),
            ("recent_boards.json", serde_json::to_vec_pretty(&self.recent_boards)?),
            ("board_prefs.json", serde_json::to_vec_pretty(&self.board_prefs)?),
            ("seen.json", serde_json::to_vec(&self.seen)?),
            ("settings.json", serde_json::to_vec_pretty(&self.settings)?),
            ("saved.json", serde_json::to_vec_pretty(&SavedIndex { version: saved::VERSION, threads: self.saved.clone() })?),
        ])
    }

    /// Write the files whose content changed.
    pub fn save(&self) -> Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        for (name, bytes) in self.files()? {
            let h = hash(&bytes);
            if self.written.borrow().get(name) != Some(&h) {
                write_atomic(&dir.join(name), &bytes)?;
                self.written.borrow_mut().insert(name, h);
            }
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
        map.retain(|_, t| now.saturating_sub(t.last) <= SEEN_FOR);
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
        // A thread opened from elsewhere (not seen in a catalog) is kept a while too.
        if t.last == 0 {
            t.last = now;
        }
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
        write_atomic(&dir.join("session.json"), &serde_json::to_vec_pretty(session)?)
    }

    /// What's hidden by hand on a board.
    pub fn hidden_on(&self, site: &str, board: &str) -> std::collections::HashSet<u64> {
        self.hidden.get(&format!("{site}/{board}")).into_iter().flatten().copied().collect()
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
        self.watched.push(Watched { key, subject, posts, last_seen, ..Default::default() });
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

impl Store {
    pub fn saved(&self, key: &ThreadKey) -> Option<&SavedMeta> {
        self.saved.iter().find(|m| &m.key == key)
    }

    /// Keep a copy of a thread's posts; returns whether it's being written. Posts that
    /// haven't changed (by `signature`) aren't. The list is updated at once; the copy is
    /// converted and written in the background (see `settle`).
    pub fn keep_thread(&mut self, key: &ThreadKey, subject: &str, url: &str, posts: &[Post], now: i64) -> bool {
        let (Some(dir), Some(writer)) = (self.dir.clone(), &self.writer) else { return false };
        if posts.is_empty() {
            return false;
        }
        let sig = signature(posts);
        let before = self.saved(key);
        if before.is_some_and(|m| m.hash == sig && !m.dead) {
            return false;
        }
        let bytes = before.map_or(0, |m| m.bytes);
        let (count, newest) = (posts.len(), posts.iter().map(|p| p.no).max().unwrap_or(0));
        let (posts, key2, subject2, url2) = (posts.to_vec(), key.clone(), subject.to_string(), url.to_string());
        writer.run(move || {
            let thread = SavedThread {
                version: saved::VERSION,
                site: key2.site.clone(),
                board: key2.board.clone(),
                no: key2.no,
                subject: subject2,
                saved: now,
                dead: false,
                url: url2,
                posts: posts.iter().map(SavedPost::from).collect(),
            };
            match saved::write(&dir, &thread) {
                Ok(bytes) => Wrote::Saved(key2, bytes),
                Err(e) => Wrote::Failed(format!("Couldn't save a copy of thread {}: {e:#}", key2.no)),
            }
        });
        self.saved.retain(|m| &m.key != key);
        let meta = SavedMeta { key: key.clone(), subject: subject.to_string(), saved: now, dead: false, bytes, posts: count, newest, hash: sig };
        // Newest first.
        let at = self.saved.iter().position(|m| m.saved <= now).unwrap_or(self.saved.len());
        self.saved.insert(at, meta);
        true
    }

    /// Collect what background writes came to: copies' sizes (then the oldest dead copies
    /// go, past `saved_max`), and errors to tell about.
    pub fn settle(&mut self) -> Vec<String> {
        let Some(writer) = &self.writer else { return Vec::new() };
        let mut errors = Vec::new();
        let mut wrote = false;
        for r in writer.results() {
            match r {
                Wrote::Saved(key, bytes) => {
                    if let Some(m) = self.saved.iter_mut().find(|m| m.key == key) {
                        m.bytes = bytes;
                        wrote = true;
                    }
                }
                Wrote::Failed(e) => errors.push(e),
                Wrote::Nothing => {}
            }
        }
        if wrote {
            self.prune_saved();
        }
        errors
    }

    /// Wait (at most `within`) for background writes, then `settle`, and again for what
    /// that started (pruning removes files in the background too).
    pub fn flush(&mut self, within: std::time::Duration) -> Vec<String> {
        let deadline = std::time::Instant::now() + within;
        let mut errors = Vec::new();
        for _ in 0..4 {
            if let Some(w) = &self.writer {
                w.flush(deadline.saturating_duration_since(std::time::Instant::now()));
            }
            errors.extend(self.settle());
            if self.writer.as_ref().is_none_or(crate::writer::Writer::is_idle) {
                break;
            }
        }
        errors
    }

    /// The thread 404'd: its copy is marked dead (and kept), after any write still going.
    pub fn saved_dead(&mut self, key: &ThreadKey) {
        let (Some(dir), Some(writer)) = (self.dir.clone(), &self.writer) else { return };
        let Some(m) = self.saved.iter_mut().find(|m| &m.key == key) else { return };
        if m.dead {
            return;
        }
        m.dead = true;
        let key = key.clone();
        writer.run(move || match saved::read(&dir, &key) {
            Ok(mut t) => {
                t.dead = true;
                saved::write(&dir, &t).map_or(Wrote::Nothing, |bytes| Wrote::Saved(key, bytes))
            }
            Err(_) => Wrote::Nothing,
        });
    }

    /// A thread's copy (once any write of it has finished).
    /// Where saved copies are kept, and the copies newest first, for searching them.
    pub fn saved_files(&self) -> Option<(PathBuf, Vec<ThreadKey>)> {
        let mut metas: Vec<&SavedMeta> = self.saved.iter().collect();
        metas.sort_by_key(|m| std::cmp::Reverse(m.saved));
        Some((self.dir.clone()?, metas.into_iter().map(|m| m.key.clone()).collect()))
    }

    pub fn load_saved(&mut self, key: &ThreadKey) -> Result<SavedThread> {
        let dir = self.dir.clone().context("no data folder")?;
        if let Some(w) = &self.writer {
            w.flush(std::time::Duration::from_secs(5));
        }
        let t = saved::read(&dir, key);
        // A copy that's gone (or was set aside) leaves the list.
        if t.is_err() {
            self.saved.retain(|m| &m.key != key);
        }
        t
    }

    /// Remove a thread's copy (after any write of it still going).
    pub fn forget_saved(&mut self, key: &ThreadKey) {
        self.saved.retain(|m| &m.key != key);
        if let (Some(dir), Some(writer)) = (self.dir.clone(), &self.writer) {
            let path = saved::path(&dir, key);
            writer.run(move || {
                let _ = std::fs::remove_file(path);
                Wrote::Nothing
            });
        }
    }

    /// Past `saved_max` bytes, remove the oldest copies that are dead and not watched (never
    /// a watched thread's).
    fn prune_saved(&mut self) {
        if self.saved_max == 0 {
            return;
        }
        let mut total = self.saved.iter().fold(0u64, |sum, m| sum.saturating_add(m.bytes));
        while total > self.saved_max {
            let watched = |m: &SavedMeta| self.watched.iter().any(|w| w.key == m.key);
            let Some(i) = self.saved.iter().rposition(|m| m.dead && !watched(m)) else { break };
            total = total.saturating_sub(self.saved[i].bytes);
            let key = self.saved[i].key.clone();
            self.forget_saved(&key);
        }
    }
}

fn load_file<T: DeserializeOwned + Default>(path: &Path, warnings: &mut Vec<String>) -> T {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return T::default(),
        Err(e) => {
            warnings.push(format!("Couldn't read {}: {e}", path.display()));
            return T::default();
        }
    };
    // Not UTF-8 counts as corrupt too, rather than being overwritten later.
    match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            // Keep the broken file for the user instead of overwriting it on the next save.
            let aside = path.with_extension("json.corrupt");
            warnings.push(match std::fs::rename(path, &aside) {
                Ok(()) => format!("{} was corrupt ({e}); moved it to {}", path.display(), aside.display()),
                Err(r) => format!("{} was corrupt ({e}), and couldn't be moved aside ({r}): it will be overwritten", path.display()),
            });
            T::default()
        }
    }
}

/// A content hash, to tell whether a file changed.
fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Write a file whole or not at all: to `<path>.tmp`, then renamed into place (its folder
/// is created if needed).
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, data).with_context(|| format!("writing {}", path.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
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
        // Where it can't be moved (a directory is in the way), the message says so.
        std::fs::write(dir.path().join("watched.json"), "{not json").unwrap();
        std::fs::create_dir_all(dir.path().join("watched.json.corrupt/full")).unwrap();
        let (_, warnings) = Store::load(Some(dir.path().to_path_buf()));
        assert!(warnings.iter().any(|w| w.contains("couldn't be moved aside")), "{warnings:?}");
    }

    #[test]
    fn hidden_threads_and_posts() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        assert!(s.toggle_hidden("4chan", "g", 5));
        assert!(s.hidden_on("4chan", "g").contains(&5) && !s.hidden_on("4chan", "v").contains(&5));
        s.save().unwrap();
        let (mut s, w) = Store::load(Some(dir.path().to_path_buf()));
        assert!(w.is_empty() && s.hidden_on("4chan", "g").contains(&5));
        assert!(!s.toggle_hidden("4chan", "g", 5));
        assert!(s.hidden.is_empty());
        // Bounded per board, oldest out first.
        for no in 0..HIDDEN_PER_BOARD as u64 + 2 {
            s.toggle_hidden("4chan", "g", no);
        }
        assert!(!s.hidden_on("4chan", "g").contains(&0) && !s.hidden_on("4chan", "g").contains(&1) && s.hidden_on("4chan", "g").contains(&2));
    }

    #[test]
    fn only_changed_files_are_written() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        // Nothing changed: nothing written (not even empty files).
        s.save().unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        s.toggle_watch(key(1), "x".into(), 1, 1);
        s.save().unwrap();
        assert!(dir.path().join("watched.json").exists() && !dir.path().join("history.json").exists());
        // Written once; an unchanged store doesn't write it again.
        std::fs::remove_file(dir.path().join("watched.json")).unwrap();
        s.save().unwrap();
        assert!(!dir.path().join("watched.json").exists());
        // A store loaded from disk has nothing to write either.
        s.board_opened("4chan", "g");
        s.save().unwrap();
        let (s, _) = Store::load(Some(dir.path().to_path_buf()));
        let before = std::fs::metadata(dir.path().join("recent_boards.json")).unwrap().modified().unwrap();
        s.save().unwrap();
        assert_eq!(std::fs::metadata(dir.path().join("recent_boards.json")).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn sorts_are_saved_by_their_labels() {
        let prefs: BoardPrefs = serde_json::from_str(r#"{"sort": "most replies"}"#).unwrap();
        assert_eq!(prefs.sort, Some(crate::config::Sort::Replies));
        assert_eq!(serde_json::to_string(&prefs).unwrap(), r#"{"sort":"most replies"}"#);
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
        // A corrupt time in the file is just old.
        s.seen.get_mut("4chan/g").unwrap().insert(7, SeenThread { last: i64::MIN, replies: None });
        s.catalog_seen("4chan", "g", &[4], i64::MAX);
        assert!(!s.seen["4chan/g"].contains_key(&7));
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

    fn posts(nos: &[u64]) -> Vec<Post> {
        nos.iter().map(|&no| Post { no, body: vec![format!("post {no}").into()], ..Default::default() }).collect()
    }

    #[test]
    fn saved_copies_kept_only_when_changed() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        let file = dir.path().join("threads/4chan/g/1.json");
        assert!(s.keep_thread(&key(1), "one", "u", &posts(&[1, 2]), 10));
        // std::time::Duration::from_secs(10)ritten in the background; the list knows at once.
        assert_eq!(s.saved[0].posts, 2);
        assert!(s.flush(std::time::Duration::from_secs(10)).is_empty());
        assert!(file.exists() && s.saved[0].bytes > 0);
        // The same posts again: not written.
        std::fs::remove_file(&file).unwrap();
        assert!(!s.keep_thread(&key(1), "one", "u", &posts(&[1, 2]), 20));
        s.flush(std::time::Duration::from_secs(10));
        assert!(!file.exists());
        // An edited post (more text) or a file added is noticed.
        let mut edited = posts(&[1, 2]);
        edited[1].body.push("more".into());
        assert!(s.keep_thread(&key(1), "one", "u", &edited, 25));
        edited[0].files.push(crate::model::Attachment { url: "f".into(), ..Default::default() });
        assert!(s.keep_thread(&key(1), "one", "u", &edited, 26));
        assert!(s.keep_thread(&key(1), "one", "u", &posts(&[1, 2, 3]), 30));
        assert_eq!((s.saved[0].posts, s.saved[0].newest, s.saved[0].saved), (3, 3, 30));
        // std::time::Duration::from_secs(10)ritten in order: the last one is what's on disk.
        s.flush(std::time::Duration::from_secs(10));
        assert_eq!(s.load_saved(&key(1)).unwrap().posts.len(), 3);
        // Nothing for an empty thread.
        assert!(!s.keep_thread(&key(2), "", "u", &[], 30));

        // Dead: marked in the list and the file (after the writes before it), and kept.
        assert!(s.keep_thread(&key(1), "one", "u", &posts(&[1, 2, 3, 4]), 31));
        s.saved_dead(&key(1));
        assert!(s.saved(&key(1)).unwrap().dead);
        assert!(s.load_saved(&key(1)).unwrap().dead);
        s.save().unwrap();
        let (mut s, w) = Store::load(Some(dir.path().to_path_buf()));
        assert!(w.is_empty() && s.saved(&key(1)).unwrap().dead);

        // Without its index, the list is rebuilt from the copies.
        std::fs::remove_file(dir.path().join("saved.json")).unwrap();
        let (s2, _) = Store::load(Some(dir.path().to_path_buf()));
        assert_eq!(s2.saved.len(), 1);

        // A broken copy is set aside and leaves the list.
        std::fs::write(&file, "{").unwrap();
        assert!(s.load_saved(&key(1)).is_err());
        assert!(s.saved.is_empty() && dir.path().join("threads/4chan/g/1.json.corrupt").exists());
    }

    #[test]
    fn pruning_spares_watched_and_live_copies() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        let many: Vec<u64> = (1..50).collect();
        for no in 1..=4 {
            s.keep_thread(&key(no), "", "u", &posts(&many), no as i64);
        }
        s.flush(std::time::Duration::from_secs(10));
        let one = s.saved[0].bytes;
        // 1 and 2 dead; 1 watched.
        s.saved_dead(&key(1));
        s.saved_dead(&key(2));
        s.toggle_watch(key(1), String::new(), 0, 0);
        s.saved_max = one * 2;
        s.keep_thread(&key(5), "", "u", &posts(&many), 5);
        // Once written, only 2 can go; the rest stay over the limit.
        s.flush(std::time::Duration::from_secs(10));
        let left: Vec<u64> = s.saved.iter().map(|m| m.key.no).collect();
        assert_eq!(left, [5, 4, 3, 1]);
        assert!(!dir.path().join("threads/4chan/g/2.json").exists());
        assert!(dir.path().join("threads/4chan/g/1.json").exists());
        s.forget_saved(&key(4));
        s.flush(std::time::Duration::from_secs(10));
        assert!(!dir.path().join("threads/4chan/g/4.json").exists() && s.saved(&key(4)).is_none());
        // A write that fails is told about.
        std::fs::remove_dir_all(dir.path().join("threads")).unwrap();
        std::fs::write(dir.path().join("threads"), "not a folder").unwrap();
        s.keep_thread(&key(6), "", "u", &posts(&many), 6);
        let errors = s.flush(std::time::Duration::from_secs(10));
        assert!(errors.len() == 1 && errors[0].contains("Couldn't save a copy of thread 6"), "{errors:?}");
    }
}
