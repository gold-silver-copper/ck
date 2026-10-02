//! Following a general (`F`): when a recurring thread like /lmg/ dies or hits the bump
//! limit, find the next one in the board's catalog and watch it instead.

use std::time::{Duration, Instant};

use super::{App, Msg, View, thread_subject};
use crate::model::Post;
use crate::store::{ThreadKey, Watched};

/// How often a dead (or full) general's board is checked for the next thread.
const RECHECK: Duration = Duration::from_secs(10 * 60);

/// What identifies a general in thread subjects: its `/tag/` (`/lmg/ - Local Models
/// General` → `/lmg/`), or else the subject without a trailing number (`Coffee Brewing #12`
/// → `Coffee Brewing`).
pub fn general_pattern(subject: &str) -> Option<String> {
    // The pieces between two slashes: all but the first and last of a split.
    let parts: Vec<&str> = subject.split('/').collect();
    let between = parts.get(1..parts.len().saturating_sub(1)).unwrap_or_default();
    let is_tag = |p: &&&str| (2..=12).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric() || "+-_".contains(c));
    if let Some(tag) = between.iter().find(is_tag) {
        return Some(format!("/{tag}/"));
    }
    let base = subject.trim().trim_end_matches(|c: char| c.is_ascii_digit() || " #.-:".contains(c));
    let base = base.strip_suffix(" Thread").or_else(|| base.strip_suffix(" thread")).unwrap_or(base).trim();
    (base.chars().count() >= 3).then(|| base.to_string())
}

/// Whether a thread is (another) one of the general.
pub fn is_general(pattern: &str, op: &Post) -> bool {
    let p = pattern.to_lowercase();
    let text = op.subject.clone().unwrap_or_else(|| op.plain_text().chars().take(200).collect());
    text.to_lowercase().contains(&p)
}

impl App {
    /// `F`: follow (or stop following) the open or selected thread as a general. Following
    /// also watches it.
    pub fn toggle_follow(&mut self) {
        let (key, subject, posts, max_no) = match (self.tab.view, &self.tab.board) {
            (View::Thread, _) => {
                let Some(t) = &self.tab.thread else { return };
                (self.key(&t.board, t.no), thread_subject(&t.posts), t.posts.len(), t.posts.iter().map(|p| p.no).max().unwrap_or(0))
            }
            (View::Catalog, Some(_)) => {
                let Some(i) = self.selected_index() else { return };
                let op = &self.tab.catalog[i];
                (self.key(&self.board_of(op), op.no), thread_subject(std::slice::from_ref(op)), 1, 0)
            }
            (View::Watched, _) => {
                let Some(i) = self.selected_index() else { return };
                let w = &self.store.watched[i];
                (w.key.clone(), w.subject.clone(), w.posts, w.last_seen)
            }
            _ => return,
        };
        if let Some(w) = self.store.watched_mut(&key)
            && let Some(pattern) = w.general.take()
        {
            self.info(format!("Stopped following {pattern} (still watching the thread)"));
            self.save();
            return;
        }
        let Some(pattern) = general_pattern(&subject) else {
            self.error("Can't tell which general this is: its subject has no /tag/ or name");
            return;
        };
        if self.store.watched(&key).is_none() {
            self.store.toggle_watch(key.clone(), subject, posts, max_no);
        }
        if let Some(w) = self.store.watched_mut(&key) {
            w.general = Some(pattern.clone());
        }
        self.save();
        let msg = format!("Following {pattern}: when this thread dies or fills up, the next one is watched");
        self.info(msg);
    }

    /// Generals whose thread is dead or full: look for the next one (in the background, at
    /// most every few minutes each).
    pub(super) fn check_generals(&mut self, now: Instant) {
        let due: Vec<ThreadKey> = self
            .store
            .watched
            .iter()
            .filter(|w| w.general.is_some() && (w.dead || w.at_limit))
            .filter(|w| !self.generals_searching.contains(&w.key))
            .filter(|w| self.generals_checked.get(&w.key).is_none_or(|t| now.duration_since(*t) >= RECHECK))
            .map(|w| w.key.clone())
            .collect();
        for key in due {
            let Some(site) = self.sites.iter().find(|s| s.cfg.name == key.site) else { continue };
            let backend = site.backend.clone();
            let tx = self.tx.clone();
            self.generals_checked.insert(key.clone(), now);
            self.generals_searching.insert(key.clone());
            std::thread::spawn(move || {
                let res = crate::http::background(|| backend.catalog(&key.board, &|_| {}));
                let _ = tx.send(Msg::GeneralCatalog(key, res));
            });
        }
    }

    /// A followed general's board catalog arrived: watch the newest matching thread.
    pub(super) fn general_catalog(&mut self, key: ThreadKey, res: anyhow::Result<Vec<Post>>) {
        self.generals_searching.remove(&key);
        let Ok(catalog) = res else { return };
        let Some(w) = self.store.watched(&key) else { return };
        let Some(pattern) = w.general.clone() else { return };
        let dead = w.dead;
        let next = catalog
            .iter()
            .filter(|op| op.no > key.no && op.board.as_deref().is_none_or(|b| b == key.board) && is_general(&pattern, op))
            .max_by_key(|op| op.no);
        let Some(op) = next else { return };
        let new_key = ThreadKey { site: key.site.clone(), board: key.board.clone(), no: op.no };
        let subject = thread_subject(std::slice::from_ref(op));
        if self.store.watched(&new_key).is_none() {
            self.store.toggle_watch(new_key.clone(), subject.clone(), op.replies.map_or(1, |r| r as usize + 1), 0);
        }
        if let Some(n) = self.store.watched_mut(&new_key) {
            n.general = Some(pattern.clone());
        }
        // A dead thread has nothing more to show; one at its bump limit is still going.
        if dead {
            self.store.watched.retain(|w| w.key != key);
        } else if let Some(old) = self.store.watched_mut(&key) {
            old.general = None;
        }
        self.generals_checked.remove(&key);
        self.save();
        let msg = format!("New {pattern} thread on /{}/: {}", key.board, subject.chars().take(60).collect::<String>());
        let method = crate::notify::method(self.notify_mode, self.notify_command.as_deref(), &|k| std::env::var(k).ok());
        let _ = crate::notify::send(&method, "ck", &msg);
        self.notified.push(msg.clone());
        self.info(msg);
    }
}

/// Whether a watched thread, refreshed, has reached its bump limit.
pub fn note_limit(w: &mut Watched, posts: &[Post]) {
    w.at_limit = posts.first().is_some_and(|op| op.bumplimit);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert_eq!(general_pattern("/lmg/ - Local Models General").as_deref(), Some("/lmg/"));
        assert_eq!(general_pattern("Home server general /hsg/ edition").as_deref(), Some("/hsg/"));
        assert_eq!(general_pattern("Coffee Brewing #12").as_deref(), Some("Coffee Brewing"));
        assert_eq!(general_pattern("Stupid Questions Thread 4").as_deref(), Some("Stupid Questions"));
        assert!(general_pattern("42").is_none());
        let op = |subject: &str| Post { subject: Some(subject.into()), ..Default::default() };
        assert!(is_general("/lmg/", &op("/LMG/ - Local Models General")));
        assert!(!is_general("/lmg/", &op("/ldg/ - Local Diffusion General")));
        // A real catalog: the /vcg/ general is found by its tag, and nothing else matches.
        let path = format!("{}/tests/fixtures/4chan_catalog.json", env!("CARGO_MANIFEST_DIR"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let catalog = crate::backend::futaba::Futaba::fourchan(None).parse_catalog("g", &v);
        let pattern = general_pattern("/vcg/ — Vibe-coding General").unwrap();
        let found: Vec<&Post> = catalog.iter().filter(|p| is_general(&pattern, p)).collect();
        assert_eq!(found.len(), 1);
        // "Stylechan / Kurobaex" has slashes but no tag.
        assert_eq!(general_pattern("4chan X(T) + Stylechan / Kurobaex + Chance + Clover Updates").as_deref(), Some("4chan X(T) + Stylechan / Kurobaex + Chance + Clover Updates"));
    }
}
