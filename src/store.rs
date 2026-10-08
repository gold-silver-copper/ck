//! Watched threads and history, saved as JSON in $XDG_DATA_HOME/ck (~/.local/share/ck).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::app::{Changed, Whole};
use crate::atomic::{Owned, Read};
use crate::model::{Board, Post, max_no};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watched {
    #[serde(flatten)]
    pub key: ThreadKey,
    pub subject: String,
    pub posts: usize,
    /// Highest post number seen while the thread was open; 0 until first known.
    pub last_seen: u64,
    /// Live with what's new since it was read, or 404'd (archived or deleted) with nothing new.
    #[serde(flatten)]
    pub status: Status,
    /// Posts marked as yours (`m`); replies to them are counted and notified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mine: Vec<u64>,
    /// Followed as a general: when the thread dies or hits the bump limit, the next thread
    /// whose subject matches this is watched instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub general: Option<String>,
    /// The thread has reached its bump limit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub at_limit: bool,
    /// The posts new since `last_seen` as last refreshed, with what decides whether they're
    /// hidden (`with_ancestry`): to count them again when that changes. In memory only.
    #[serde(skip)]
    pub fresh: Vec<Post>,
}

impl Watched {
    /// Newly watched: live, with nothing new yet.
    pub fn new(key: ThreadKey, subject: String, posts: usize, last_seen: u64) -> Self {
        Watched { key, subject, posts, last_seen, status: Status::READ, mine: Vec::new(), general: None, at_limit: false, fresh: Vec::new() }
    }

    /// The posts marked as yours (`Store::toggle_mine`).
    pub fn mine(&self) -> &[u64] {
        &self.mine
    }

    /// Live, with what a refresh found new: `fresh` (`with_ancestry`), counted as
    /// `(unread, replies)`.
    pub fn refreshed(&mut self, (unread, replies): (usize, usize), fresh: Vec<Post>) {
        self.status = Status::Live { unread, replies };
        self.fresh = fresh;
    }

    /// Its new posts counted again, as what's hidden changed (`App::rehide`). A dead thread
    /// stays dead, with nothing new.
    pub fn recount(&mut self, (unread, replies): (usize, usize)) {
        if !self.status.is_dead() {
            self.status = Status::Live { unread, replies };
        }
    }
}

/// A watched thread: refreshed, with what's new in it, or 404'd and no longer refreshed.
/// Anything counted as new belongs in Live, so a dead thread can't keep or show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "StatusFile", into = "StatusFile")]
pub enum Status {
    Live { unread: usize, replies: usize },
    Dead,
}

impl Status {
    /// Live with nothing new: newly watched, or just read. It revives a dead thread, so it's
    /// set only on a thread known to be there.
    pub const READ: Status = Status::Live { unread: 0, replies: 0 };

    /// (unread posts, unread replies to you); a dead thread has none.
    pub fn counts(self) -> (usize, usize) {
        match self {
            Status::Live { unread, replies } => (unread, replies),
            Status::Dead => (0, 0),
        }
    }

    pub fn is_dead(self) -> bool {
        self == Status::Dead
    }
}

/// Status as watched.json has always kept it, in flat fields. Dead with counts (as older
/// files can have) loads as Dead.
#[derive(Serialize, Deserialize)]
struct StatusFile {
    unread: usize,
    #[serde(default)]
    dead: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    replies: usize,
}

impl From<StatusFile> for Status {
    fn from(f: StatusFile) -> Self {
        if f.dead { Status::Dead } else { Status::Live { unread: f.unread, replies: f.replies } }
    }
}

impl From<Status> for StatusFile {
    fn from(s: Status) -> Self {
        let (unread, replies) = s.counts();
        StatusFile { unread, dead: s.is_dead(), replies }
    }
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
    /// Opened, but not (yet) seen in its board's catalog: `last` is when it was last opened.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opened_only: bool,
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

/// The `site/board` a board's hidden numbers, preferences, seen threads and place in the
/// recent boards are kept under, in memory and in their files.
pub fn board_key(site: &str, board: &str) -> String {
    format!("{site}/{board}")
}

#[derive(Default)]
pub struct Store {
    dir: Option<PathBuf>,
    /// Changed only through the methods below, which say what changes what's hidden or
    /// yours (`Changed`).
    watched: Vec<Watched>,
    /// Most recent first.
    pub history: Vec<Visit>,
    pub settings: Settings,
    /// Thread and post numbers hidden by hand, by `site/board`, oldest first.
    hidden: std::collections::BTreeMap<String, Vec<u64>>,
    /// Per-board catalog sort and layout, by `site/board`.
    pub board_prefs: std::collections::BTreeMap<String, BoardPrefs>,
    /// Boards whose catalogs were opened, `site/board`, most recent first.
    pub recent_boards: Vec<String>,
    /// Catalog threads seen, by `site/board`.
    pub seen: std::collections::BTreeMap<String, std::collections::BTreeMap<u64, SeenThread>>,
    /// Threads kept in `threads/` (see `saved`), newest first.
    pub saved: Vec<SavedMeta>,
    /// Where you were, as last saved (`save_session`).
    pub session: Session,
    /// Past this many bytes, the oldest unwatched copies are removed.
    pub saved_max: u64,
    /// Writes saved copies in the background (with a data directory).
    writer: Option<crate::writer::Writer<Wrote>>,
    /// The permission to write each file, which one that couldn't be read hasn't got.
    owned: std::collections::HashMap<&'static str, Owned>,
}

/// What a background write of a saved copy came to.
pub enum Wrote {
    /// Written: its size, and the `signature` of the posts written (none when only marked dead).
    Saved(ThreadKey, u64, Option<u64>),
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
            (&f.url, f.kind).hash(&mut h);
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
    /// file is moved aside and ck starts with an empty list; one that can't be read starts
    /// empty too, and isn't saved over.
    pub fn load(dir: Option<PathBuf>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let writer = dir.is_some().then(|| crate::writer::Writer::new(|what| Wrote::Failed(format!("Couldn't save a copy: ck hit a bug ({what})"))));
        let mut store = Store { dir, writer, ..Default::default() };
        if let Some(dir) = &store.dir {
            store.watched = load_file(dir, "watched.json", &mut store.owned, &mut warnings);
            store.history = load_file(dir, "history.json", &mut store.owned, &mut warnings);
            store.settings = load_file(dir, "settings.json", &mut store.owned, &mut warnings);
            store.hidden = load_file(dir, "hidden.json", &mut store.owned, &mut warnings);
            store.seen = load_file(dir, "seen.json", &mut store.owned, &mut warnings);
            store.recent_boards = load_file(dir, "recent_boards.json", &mut store.owned, &mut warnings);
            store.board_prefs = load_file(dir, "board_prefs.json", &mut store.owned, &mut warnings);
            store.session = load_file(dir, "session.json", &mut store.owned, &mut warnings);
            let index: Option<SavedIndex> = load_file(dir, "saved.json", &mut store.owned, &mut warnings);
            store.saved = match index {
                Some(index) => index.threads,
                // No index (or one that's broken or can't be read): rebuild it from the
                // copies themselves, but don't write one that leaves out a copy it couldn't read.
                None => {
                    let (found, whole) = saved::scan(dir, &mut warnings);
                    if !whole && store.owned.remove("saved.json").is_some() {
                        warnings.push("Some saved threads couldn't be read: ck won't save changes to saved.json this run".into());
                    }
                    found
                }
            };
        }
        // What was just read counts as written.
        for (name, bytes) in store.files().into_iter().flatten() {
            if let Some(o) = store.owned.get(name) {
                o.mark(&bytes);
            }
        }
        (store, warnings)
    }

    /// Every file with its content.
    fn files(&self) -> Result<[(&'static str, Vec<u8>); 9]> {
        Ok([
            ("watched.json", serde_json::to_vec_pretty(&self.watched)?),
            ("history.json", serde_json::to_vec_pretty(&self.history)?),
            ("hidden.json", serde_json::to_vec(&self.hidden)?),
            ("recent_boards.json", serde_json::to_vec_pretty(&self.recent_boards)?),
            ("board_prefs.json", serde_json::to_vec_pretty(&self.board_prefs)?),
            ("seen.json", serde_json::to_vec(&self.seen)?),
            ("settings.json", serde_json::to_vec_pretty(&self.settings)?),
            ("saved.json", serde_json::to_vec_pretty(&SavedIndex { version: saved::VERSION, threads: self.saved.clone() })?),
            ("session.json", serde_json::to_vec_pretty(&self.session)?),
        ])
    }

    /// Write the files whose content changed, but none that couldn't be read. Each is
    /// tried; the first failure is told, with how many more there were.
    pub fn save(&self) -> Result<()> {
        let mut failed = self.files()?.into_iter().filter_map(|(name, bytes)| self.owned.get(name)?.write(&bytes).err());
        let Some(first) = failed.next() else { return Ok(()) };
        match failed.count() {
            0 => Err(first),
            more => Err(anyhow::anyhow!("{first:#} (and {more} more files)")),
        }
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
        crate::atomic::write(&path, &serde_json::to_vec(&saved)?)
    }

    /// A board's catalog was loaded: remember its threads, and return the ones that weren't
    /// there on the previous load (none on the first: threads only opened don't make one).
    pub fn catalog_seen(&mut self, site: &str, board: &str, threads: &[u64], now: i64) -> std::collections::HashSet<u64> {
        let map = self.seen.entry(board_key(site, board)).or_default();
        let first = map.values().all(|t| t.opened_only);
        let new = if first { Default::default() } else { threads.iter().copied().filter(|no| !map.contains_key(no)).collect() };
        for &no in threads {
            let t = map.entry(no).or_default();
            t.last = now;
            t.opened_only = false;
        }
        map.retain(|_, t| now.saturating_sub(t.last) <= SEEN_FOR);
        new
    }

    /// A board's catalog was opened: it goes to the front of the recent boards.
    pub fn board_opened(&mut self, site: &str, board: &str) {
        let key = board_key(site, board);
        self.recent_boards.retain(|b| *b != key);
        self.recent_boards.insert(0, key);
        self.recent_boards.truncate(RECENT_BOARDS);
    }

    /// A thread was opened with `replies` replies.
    pub fn opened(&mut self, site: &str, board: &str, no: u64, replies: u32, now: i64) {
        // Threads only opened are forgotten a week after their last opening, on every board
        // (a catalog's own threads go when it's loaded).
        for map in self.seen.values_mut() {
            map.retain(|_, t| !t.opened_only || now.saturating_sub(t.last) <= SEEN_FOR);
        }
        self.seen.retain(|_, map| !map.is_empty());
        let t = self.seen.entry(board_key(site, board)).or_default().entry(no).or_insert_with(|| SeenThread { opened_only: true, ..Default::default() });
        t.replies = Some(replies);
        // A thread opened from elsewhere (not seen in a catalog) is kept a while too.
        if t.opened_only || t.last == 0 {
            t.last = now;
        }
    }

    /// Replies the thread had when it was last opened.
    pub fn replies_seen(&self, site: &str, board: &str, no: u64) -> Option<u32> {
        self.seen.get(&board_key(site, board))?.get(&no)?.replies
    }

    /// Remember where you are, and write it now (if it changed).
    pub fn save_session(&mut self, session: Session) -> Result<()> {
        self.session = session;
        let Some(owned) = self.owned.get("session.json") else { return Ok(()) };
        owned.write(&serde_json::to_vec_pretty(&self.session)?)
    }

    /// What's hidden by hand on a board.
    pub fn hidden_on(&self, site: &str, board: &str) -> std::collections::HashSet<u64> {
        self.hidden.get(&board_key(site, board)).into_iter().flatten().copied().collect()
    }

    /// Whether a thread or post is hidden by hand.
    pub fn is_hidden(&self, site: &str, board: &str, no: u64) -> bool {
        self.hidden.get(&board_key(site, board)).is_some_and(|l| l.contains(&no))
    }

    /// How many threads and posts are hidden by hand, on every board.
    pub fn hidden_count(&self) -> usize {
        self.hidden.values().map(Vec::len).sum()
    }

    /// Hide a thread or post, or unhide it; whether it's hidden now.
    pub fn toggle_hidden(&mut self, site: &str, board: &str, no: u64) -> Changed<bool> {
        let key = board_key(site, board);
        let list = self.hidden.entry(key.clone()).or_default();
        if let Some(i) = list.iter().position(|&n| n == no) {
            list.remove(i);
            if list.is_empty() {
                self.hidden.remove(&key);
            }
            return Changed::new(false);
        }
        list.push(no);
        if list.len() > HIDDEN_PER_BOARD {
            list.remove(0);
        }
        Changed::new(true)
    }

    pub fn all_watched(&self) -> &[Watched] {
        &self.watched
    }

    /// Watched, to change in tests as they like.
    #[cfg(test)]
    pub fn watched_vec(&mut self) -> &mut Vec<Watched> {
        &mut self.watched
    }

    pub fn watched(&self, key: &ThreadKey) -> Option<&Watched> {
        self.watched.iter().find(|w| &w.key == key)
    }

    pub fn watched_mut(&mut self, key: &ThreadKey) -> Option<&mut Watched> {
        self.watched.iter_mut().find(|w| &w.key == key)
    }

    /// All watched threads' unread posts and replies to you (dead ones have none).
    pub fn watched_new(&self) -> (usize, usize) {
        self.watched.iter().map(|w| w.status.counts()).fold((0, 0), |(n, y), (u, r)| (n.saturating_add(u), y.saturating_add(r)))
    }

    /// The thread 404'd. If it's watched, it's now dead with nothing new (nor anything for
    /// `App::rehide` to count again); its saved copy is marked dead. Returns whether it's
    /// watched (so Watched is to be saved).
    pub fn mark_dead(&mut self, key: &ThreadKey) -> bool {
        self.saved_dead(key);
        let Some(w) = self.watched_mut(key) else { return false };
        w.status = Status::Dead;
        w.fresh.clear();
        true
    }

    /// Watch a thread, unless it's watched already; whether it wasn't. Nothing hiding reads
    /// changes (none of its posts are yours yet), so it's no `Changed`.
    pub fn watch(&mut self, key: ThreadKey, subject: String, posts: usize, last_seen: u64) -> bool {
        if self.watched(&key).is_some() {
            return false;
        }
        self.watched.push(Watched::new(key, subject, posts, last_seen));
        true
    }

    pub fn unwatch(&mut self, key: &ThreadKey) -> Changed<()> {
        self.watched.retain(|w| &w.key != key);
        Changed::new(())
    }

    /// Mark a post of a watched thread as yours, or not; whether it's yours now.
    pub fn toggle_mine(&mut self, key: &ThreadKey, no: u64) -> Changed<bool> {
        let Some(w) = self.watched_mut(key) else { return Changed::new(false) };
        let mine = !w.mine.contains(&no);
        if mine {
            w.mine.push(no);
        } else {
            w.mine.retain(|&n| n != no);
        }
        Changed::new(mine)
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
            w.status = Status::READ;
            w.fresh.clear();
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
    pub fn keep_thread(&mut self, key: &ThreadKey, subject: &str, url: &str, t: &Whole, now: i64) -> bool {
        let (Some(dir), Some(writer)) = (self.dir.clone(), &self.writer) else { return false };
        let posts = t.posts();
        let sig = signature(posts);
        let before = self.saved(key);
        if before.is_some_and(|m| m.hash == sig && !m.dead) {
            return false;
        }
        let bytes = before.map_or(0, |m| m.bytes);
        let (count, newest) = (posts.len(), max_no(posts));
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
                Ok(bytes) => Wrote::Saved(key2, bytes, Some(sig)),
                Err(e) => Wrote::Failed(format!("Couldn't save a copy of thread {}: {e:#}", key2.no)),
            }
        });
        // The posts count as saved once they're written (see `settle`): a write that fails,
        // or hasn't finished at exit, is tried again with the same posts.
        let hash = before.map_or(0, |m| m.hash);
        self.saved.retain(|m| &m.key != key);
        let meta = SavedMeta { key: key.clone(), subject: subject.to_string(), saved: now, dead: false, bytes, posts: count, newest, hash };
        // Newest first.
        let at = self.saved.iter().position(|m| m.saved <= now).unwrap_or(self.saved.len());
        self.saved.insert(at, meta);
        true
    }

    /// Collect what background writes came to: copies' sizes (then the oldest unwatched
    /// copies go, past `saved_max`), and errors to tell about.
    pub fn settle(&mut self) -> Vec<String> {
        let Some(writer) = &self.writer else { return Vec::new() };
        let mut errors = Vec::new();
        let mut wrote = false;
        for r in writer.results() {
            match r {
                Wrote::Saved(key, bytes, sig) => {
                    if let Some(m) = self.saved.iter_mut().find(|m| m.key == key) {
                        m.bytes = bytes;
                        m.hash = sig.unwrap_or(m.hash);
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
        writer.run(move || {
            let Read::Loaded(mut t, _) = crate::atomic::read::<SavedThread>(&saved::path(&dir, &key)) else { return Wrote::Nothing };
            t.dead = true;
            saved::write(&dir, &t).map_or(Wrote::Nothing, |bytes| Wrote::Saved(key, bytes, None))
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
        let file = saved::path(&dir, key);
        let gone = match crate::atomic::read(&file) {
            Read::Loaded(t, _) => return Ok(t),
            Read::Missing(_) => anyhow::anyhow!("{} is gone", file.display()),
            Read::Corrupt(problem, _) => anyhow::anyhow!(problem),
            // Only unreadable for now: it stays listed.
            Read::Unreadable(problem) => anyhow::bail!(problem),
        };
        // A copy that's gone (or was set aside) leaves the list.
        self.saved.retain(|m| &m.key != key);
        Err(gone)
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

    /// Past `saved_max` bytes, remove the oldest copies of threads not watched, dead or not
    /// (a thread that's still up is only known to be once it's refetched, which an unwatched
    /// one isn't). Never a watched thread's.
    fn prune_saved(&mut self) {
        // With a watched list that couldn't be read, which copies are watched isn't known.
        if self.saved_max == 0 || self.dir.is_some() && !self.owned.contains_key("watched.json") {
            return;
        }
        let mut total = self.saved.iter().fold(0u64, |sum, m| sum.saturating_add(m.bytes));
        while total > self.saved_max {
            let watched = |m: &SavedMeta| self.watched.iter().any(|w| w.key == m.key);
            let Some(m) = self.saved.iter().rev().find(|m| !watched(m)) else { break };
            total = total.saturating_sub(m.bytes);
            let key = m.key.clone();
            self.forget_saved(&key);
        }
    }
}

/// A data file's content, or the default when there's none to load; with the permission to
/// write it, unless it couldn't be read.
fn load_file<T: DeserializeOwned + Default>(dir: &Path, name: &'static str, owned: &mut std::collections::HashMap<&'static str, Owned>, warnings: &mut Vec<String>) -> T {
    let (v, o) = match crate::atomic::read(&dir.join(name)) {
        Read::Loaded(v, o) => (v, o),
        Read::Missing(o) => (T::default(), o),
        Read::Corrupt(problem, o) => {
            warnings.push(problem);
            (T::default(), o)
        }
        Read::Unreadable(problem) => {
            warnings.push(format!("{problem}: ck won't save changes to it this run"));
            return T::default();
        }
    };
    owned.insert(name, o);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::whole;

    fn key(no: u64) -> ThreadKey {
        ThreadKey { site: "4chan".into(), board: "g".into(), no }
    }

    #[test]
    fn roundtrip_and_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, warnings) = Store::load(Some(dir.path().to_path_buf()));
        assert!(warnings.is_empty() && s.watched.is_empty());
        assert!(s.watch(key(1), "one".into(), 5, 105));
        s.visit(&key(2), "two", 3, 203, 1000);
        s.save().unwrap();
        // No temp file is left behind.
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|e| e.unwrap().path().extension().is_none_or(|x| x != "tmp")));

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
        assert!(s.toggle_hidden("4chan", "g", 5).unchecked());
        assert!(s.hidden_on("4chan", "g").contains(&5) && !s.hidden_on("4chan", "v").contains(&5));
        s.save().unwrap();
        let (mut s, w) = Store::load(Some(dir.path().to_path_buf()));
        assert!(w.is_empty() && s.hidden_on("4chan", "g").contains(&5));
        assert!(!s.toggle_hidden("4chan", "g", 5).unchecked());
        assert!(s.hidden.is_empty());
        // Bounded per board, oldest out first.
        for no in 0..HIDDEN_PER_BOARD as u64 + 2 {
            s.toggle_hidden("4chan", "g", no).unchecked();
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
        s.watch(key(1), "x".into(), 1, 1);
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
        s.seen.get_mut("4chan/g").unwrap().insert(7, SeenThread { last: i64::MIN, ..Default::default() });
        s.catalog_seen("4chan", "g", &[4], i64::MAX);
        assert!(!s.seen["4chan/g"].contains_key(&7));
    }

    #[test]
    fn opening_a_thread_first_leaves_the_catalog_its_first_load() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        let day = 24 * 3600;
        // A thread opened from a link before the board's catalog was ever loaded.
        s.opened("4chan", "g", 2, 5, 0);
        s.save().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        assert!(s.catalog_seen("4chan", "g", &[1, 2, 3], day).is_empty());
        assert_eq!(s.replies_seen("4chan", "g", 2), Some(5));
        assert_eq!(s.catalog_seen("4chan", "g", &[1, 2, 3, 4], day), [4].into());
        // On a board whose catalog is never loaded, opened threads are forgotten a week
        // after they were last opened.
        s.opened("4chan", "v", 7, 1, 0);
        s.opened("4chan", "v", 8, 1, 0);
        s.opened("4chan", "v", 8, 2, 3 * day);
        s.opened("4chan", "g", 9, 1, 9 * day);
        assert_eq!((s.replies_seen("4chan", "v", 7), s.replies_seen("4chan", "v", 8)), (None, Some(2)));
        s.opened("4chan", "g", 9, 1, 11 * day);
        assert!(!s.seen.contains_key("4chan/v"));
        // A catalog's threads go only with its own loads.
        assert_eq!(s.replies_seen("4chan", "g", 2), Some(5));
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
        s.watch(key(1), "x".into(), 10, 110);
        s.watched_mut(&key(1)).unwrap().status = Status::Live { unread: 4, replies: 1 };
        s.visit(&key(1), "x", 14, 114, 0);
        assert_eq!((s.watched[0].status, s.watched[0].last_seen, s.watched[0].posts), (Status::READ, 114, 14));
        assert!(!s.watch(key(1), String::new(), 0, 0));
        s.unwatch(&key(1)).unchecked();
        assert!(s.watched.is_empty());
    }

    #[test]
    fn watched_status_keeps_its_file_format() {
        let json = |w: &Watched| serde_json::to_string(w).unwrap();
        let head = r#"{"site":"4chan","board":"g","no":1,"subject":"s","posts":2,"last_seen":3,"#;
        // A dead thread with counts, as older files have it: dead, with nothing new.
        let old: Watched = serde_json::from_str(&format!(r#"{head}"unread":3,"dead":true,"replies":2}}"#)).unwrap();
        assert_eq!(old.status, Status::Dead);
        assert_eq!(json(&old), format!(r#"{head}"unread":0,"dead":true}}"#));
        // A live one is written as it always was.
        let live = format!(r#"{head}"unread":3,"dead":false,"replies":2}}"#);
        let w: Watched = serde_json::from_str(&live).unwrap();
        assert_eq!((w.status, json(&w)), (Status::Live { unread: 3, replies: 2 }, live));
        // `unread` is still required.
        assert!(serde_json::from_str::<Watched>(&format!(r#"{head}"dead":true}}"#)).is_err());
    }

    fn posts(nos: &[u64]) -> Vec<Post> {
        nos.iter().map(|&no| Post { no, body: vec![format!("post {no}").into()], ..Default::default() }).collect()
    }

    #[test]
    fn saved_copies_kept_only_when_changed() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        let file = dir.path().join("threads/4chan/g/1.json");
        assert!(s.keep_thread(&key(1), "one", "u", &whole(&posts(&[1, 2])), 10));
        // std::time::Duration::from_secs(10)ritten in the background; the list knows at once.
        assert_eq!(s.saved[0].posts, 2);
        assert!(s.flush(std::time::Duration::from_secs(10)).is_empty());
        assert!(file.exists() && s.saved[0].bytes > 0);
        // The same posts again: not written.
        std::fs::remove_file(&file).unwrap();
        assert!(!s.keep_thread(&key(1), "one", "u", &whole(&posts(&[1, 2])), 20));
        s.flush(std::time::Duration::from_secs(10));
        assert!(!file.exists());
        // An edited post (more text) or a file added is noticed.
        let mut edited = posts(&[1, 2]);
        edited[1].body.push("more".into());
        assert!(s.keep_thread(&key(1), "one", "u", &whole(&edited), 25));
        edited[0].files.push(crate::model::Attachment::at("f"));
        assert!(s.keep_thread(&key(1), "one", "u", &whole(&edited), 26));
        assert!(s.keep_thread(&key(1), "one", "u", &whole(&posts(&[1, 2, 3])), 30));
        assert_eq!((s.saved[0].posts, s.saved[0].newest, s.saved[0].saved), (3, 3, 30));
        // std::time::Duration::from_secs(10)ritten in order: the last one is what's on disk.
        s.flush(std::time::Duration::from_secs(10));
        assert_eq!(s.load_saved(&key(1)).unwrap().posts.len(), 3);

        // Dead: marked in the list and the file (after the writes before it), and kept.
        assert!(s.keep_thread(&key(1), "one", "u", &whole(&posts(&[1, 2, 3, 4])), 31));
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
    fn pruning_spares_only_watched_copies() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        let many: Vec<u64> = (1..50).collect();
        for no in 1..=4 {
            s.keep_thread(&key(no), "", "u", &whole(&posts(&many)), no as i64);
        }
        s.flush(std::time::Duration::from_secs(10));
        let one = s.saved[0].bytes;
        // 1 and 2 dead; 1, 4 and 5 watched. 3 is up but unwatched (say, saved as a page).
        s.saved_dead(&key(1));
        s.saved_dead(&key(2));
        for no in [1, 4, 5] {
            s.watch(key(no), String::new(), 0, 0);
        }
        s.saved_max = one * 2;
        s.keep_thread(&key(5), "", "u", &whole(&posts(&many)), 5);
        // Once written, the unwatched ones go, oldest first, dead or not; the watched ones
        // stay, even over the limit.
        s.flush(std::time::Duration::from_secs(10));
        let left: Vec<u64> = s.saved.iter().map(|m| m.key.no).collect();
        assert_eq!(left, [5, 4, 1]);
        assert!(!dir.path().join("threads/4chan/g/2.json").exists() && !dir.path().join("threads/4chan/g/3.json").exists());
        assert!(dir.path().join("threads/4chan/g/1.json").exists());
        s.forget_saved(&key(4));
        s.flush(std::time::Duration::from_secs(10));
        assert!(!dir.path().join("threads/4chan/g/4.json").exists() && s.saved(&key(4)).is_none());
        // A write that fails is told about.
        std::fs::remove_dir_all(dir.path().join("threads")).unwrap();
        std::fs::write(dir.path().join("threads"), "not a folder").unwrap();
        // (Watched, so it stays over the limit.)
        s.watch(key(6), String::new(), 0, 0);
        s.keep_thread(&key(6), "", "u", &whole(&posts(&many)), 6);
        let errors = s.flush(std::time::Duration::from_secs(10));
        assert!(errors.len() == 1 && errors[0].contains("Couldn't save a copy of thread 6"), "{errors:?}");
        // And the same posts are written again next time.
        std::fs::remove_file(dir.path().join("threads")).unwrap();
        assert!(s.keep_thread(&key(6), "", "u", &whole(&posts(&many)), 7));
        assert!(s.flush(std::time::Duration::from_secs(10)).is_empty());
        assert!(dir.path().join("threads/4chan/g/6.json").exists());
    }

    #[test]
    fn a_copy_counts_as_saved_once_written() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        assert!(s.keep_thread(&key(1), "", "u", &whole(&posts(&[1, 2])), 1));
        // Saved before the write is known to be done (as at exit, when it takes too long):
        // the next start doesn't take the posts for written.
        s.save().unwrap();
        let next = tempfile::tempdir().unwrap();
        std::fs::copy(dir.path().join("saved.json"), next.path().join("saved.json")).unwrap();
        let (mut s2, _) = Store::load(Some(next.path().to_path_buf()));
        assert!(s2.keep_thread(&key(1), "", "u", &whole(&posts(&[1, 2])), 2));
        // Once it's done, it does.
        s.flush(std::time::Duration::from_secs(10));
        assert!(!s.keep_thread(&key(1), "", "u", &whole(&posts(&[1, 2])), 3));
    }

    /// Makes `path` unreadable (as a root-owned 0600 file left by `sudo ck` would be) and
    /// says whether that took: as root, nothing is unreadable, so the test has nothing to show.
    #[cfg(unix)]
    fn lock_out(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        let folder = path.is_dir();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if folder { std::fs::read_dir(path).is_err() } else { std::fs::read(path).is_err() }
    }

    #[cfg(unix)]
    fn let_in(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_watched_list_that_cant_be_read_isnt_saved_over() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("watched.json");
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        s.watch(key(1), "one".into(), 1, 1);
        s.watch(key(2), "two".into(), 1, 1);
        s.save().unwrap();
        if !lock_out(&file) {
            return;
        }
        let (mut s, warnings) = Store::load(Some(dir.path().to_path_buf()));
        assert!(!warnings.is_empty());
        // Watching another thread this run must not replace the list it couldn't read.
        s.watch(key(3), "three".into(), 1, 1);
        let _ = s.save();
        let_in(&file);
        let (s, _) = Store::load(Some(dir.path().to_path_buf()));
        assert!(s.watched(&key(1)).is_some() && s.watched(&key(2)).is_some(), "watched.json was saved over: {:?}", s.watched.iter().map(|w| w.key.no).collect::<Vec<_>>());
    }

    #[cfg(unix)]
    #[test]
    fn a_saved_index_that_cant_be_read_doesnt_lose_the_copies() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("saved.json");
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        for no in [1, 2] {
            s.keep_thread(&key(no), "", "u", &whole(&posts(&[1, 2])), no as i64);
        }
        assert!(s.flush(std::time::Duration::from_secs(10)).is_empty());
        s.save().unwrap();
        if !lock_out(&index) {
            return;
        }
        let (mut s, warnings) = Store::load(Some(dir.path().to_path_buf()));
        assert!(!warnings.is_empty());
        s.keep_thread(&key(3), "", "u", &whole(&posts(&[1, 2])), 3);
        s.flush(std::time::Duration::from_secs(10));
        let _ = s.save();
        let_in(&index);
        // The copies already on disk are still listed (so searched and pruned) next run.
        let (s, _) = Store::load(Some(dir.path().to_path_buf()));
        assert!(s.saved(&key(1)).is_some() && s.saved(&key(2)).is_some(), "copies left out of saved.json: {:?}", s.saved.iter().map(|m| m.key.no).collect::<Vec<_>>());
    }

    #[cfg(unix)]
    #[test]
    fn an_index_rebuilt_without_a_copy_that_cant_be_read_isnt_written() {
        // saved.json is gone, and one of the copies, or the folder it's in, can't be read
        // (as after `sudo ck`).
        let other = |no| ThreadKey { board: "a".into(), ..key(no) };
        for folder in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
            for k in [key(1), other(2)] {
                s.keep_thread(&k, "", "u", &whole(&posts(&[1, 2])), k.no as i64);
            }
            assert!(s.flush(std::time::Duration::from_secs(10)).is_empty());
            s.save().unwrap();
            std::fs::remove_file(dir.path().join("saved.json")).unwrap();
            let copy = saved::path(dir.path(), &key(1));
            let locked = if folder { copy.parent().unwrap().to_path_buf() } else { copy };
            if !lock_out(&locked) {
                return;
            }
            let (mut s, warnings) = Store::load(Some(dir.path().to_path_buf()));
            assert!(!warnings.is_empty(), "no warning with {locked:?} locked");
            s.keep_thread(&other(3), "", "u", &whole(&posts(&[1, 2])), 3);
            s.flush(std::time::Duration::from_secs(10));
            let _ = s.save();
            let_in(&locked);
            let (s, _) = Store::load(Some(dir.path().to_path_buf()));
            assert!(s.saved(&key(1)).is_some(), "the copy that couldn't be read was dropped from saved.json, with {locked:?} locked");
        }
    }

    #[cfg(unix)]
    #[test]
    fn copies_of_threads_on_a_watched_list_that_cant_be_read_arent_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        for no in [1, 2] {
            s.watch(key(no), String::new(), 0, 0);
            s.keep_thread(&key(no), "", "u", &whole(&posts(&[1, 2])), no as i64);
        }
        assert!(s.flush(std::time::Duration::from_secs(10)).is_empty());
        s.save().unwrap();
        let one = s.saved[0].bytes;
        if !lock_out(&dir.path().join("watched.json")) {
            return;
        }
        // Over the limit, with nothing known to be watched: none of the copies goes.
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        s.saved_max = one * 2;
        s.keep_thread(&key(3), "", "u", &whole(&posts(&[1, 2])), 3);
        s.flush(std::time::Duration::from_secs(10));
        for no in [1, 2] {
            assert!(saved::path(dir.path(), &key(no)).exists(), "the copy of watched thread {no} was pruned");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_that_cant_be_opened_right_now_stays_listed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("threads/4chan/g/1.json");
        let (mut s, _) = Store::load(Some(dir.path().to_path_buf()));
        s.keep_thread(&key(1), "one", "u", &whole(&posts(&[1, 2])), 1);
        assert!(s.flush(std::time::Duration::from_secs(10)).is_empty());
        if !lock_out(&file) {
            return;
        }
        assert!(s.load_saved(&key(1)).is_err());
        // Not gone and not corrupt, only unreadable for now: the entry stays.
        assert!(s.saved(&key(1)).is_some(), "an unreadable copy was dropped from the list");
        let_in(&file);
        assert_eq!(s.load_saved(&key(1)).unwrap().posts.len(), 2);
    }
}
