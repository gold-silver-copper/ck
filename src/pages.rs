//! The last copy of each catalog and thread opened, in `$XDG_CACHE_HOME/ck/pages`: the API
//! responses as fetched (see `http::Copy`), so opening one again shows it at once (parsed
//! from the copy, without a request) while it's refreshed, and so the first request after a
//! restart can ask `If-Modified-Since`. A cache: bounded, and safe to delete.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::http::Copy;

/// The format of a page file.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Page {
    version: u32,
    /// When it was fetched (Unix seconds).
    fetched: i64,
    copies: Vec<Copy>,
}

#[derive(Debug, Clone)]
pub struct Pages {
    dir: PathBuf,
    /// The most bytes kept; the least recently written go first.
    budget: u64,
}

impl Pages {
    pub fn new(dir: PathBuf, budget: u64) -> Self {
        Pages { dir, budget }
    }

    pub fn default_dir() -> Option<PathBuf> {
        Some(crate::disk_cache::DiskCache::default_dir()?.parent()?.join("pages"))
    }

    /// A catalog's file (no thread), or a thread's.
    fn path(&self, site: &str, board: &str, thread: Option<u64>) -> PathBuf {
        let name = thread.map_or_else(|| "catalog.json".to_string(), |no| format!("{no}.json"));
        self.dir.join(crate::saved::component(site)).join(crate::saved::component(board)).join(name)
    }

    /// The copies kept for a catalog or thread, and when they were fetched. A file that
    /// won't load (broken, or from another version) is removed.
    pub fn read(&self, site: &str, board: &str, thread: Option<u64>) -> Option<(Vec<Copy>, i64)> {
        let path = self.path(site, board, thread);
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice::<Page>(&bytes) {
            Ok(p) if p.version == VERSION && !p.copies.is_empty() => Some((p.copies, p.fetched)),
            _ => {
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    /// Keep the copies a catalog or thread was made from, unless they're what's kept
    /// already; then remove the oldest pages past the budget.
    pub fn write(&self, site: &str, board: &str, thread: Option<u64>, copies: &[Copy], fetched: i64) {
        if copies.is_empty() || self.read(site, board, thread).is_some_and(|(old, _)| old == copies) {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(&Page { version: VERSION, fetched, copies: copies.to_vec() }) else { return };
        if crate::store::write_atomic(&self.path(site, board, thread), &bytes).is_ok() {
            self.prune();
        }
    }

    /// Past the budget, remove the least recently written pages.
    fn prune(&self) {
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
        let dirs = |p: &std::path::Path| std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).collect::<Vec<_>>();
        for site in dirs(&self.dir) {
            for board in dirs(&site) {
                for file in dirs(&board) {
                    if let Some(m) = file.metadata().ok().filter(|m| m.is_file()) {
                        files.push((m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), file));
                    }
                }
            }
        }
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort();
        for (_, len, file) in files {
            if total <= self.budget {
                break;
            }
            if std::fs::remove_file(&file).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(url: &str, v: i64) -> Copy {
        Copy { url: url.into(), last_modified: Some(format!("day {v}")), body: serde_json::json!({ "v": v, "pad": "x".repeat(200) }) }
    }

    #[test]
    fn kept_read_back_bounded_and_broken_ones_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let pages = Pages::new(dir.path().to_path_buf(), 1_500);
        assert!(pages.read("4chan", "g", None).is_none());
        pages.write("4chan", "g", None, &[copy("u1", 1), copy("u2", 2)], 100);
        let (copies, fetched) = pages.read("4chan", "g", None).unwrap();
        assert_eq!((copies.len(), fetched), (2, 100));
        // The same copies again: not rewritten (the time stays).
        pages.write("4chan", "g", None, &[copy("u1", 1), copy("u2", 2)], 200);
        assert_eq!(pages.read("4chan", "g", None).unwrap().1, 100);
        // Odd names stay inside; threads and catalogs apart.
        pages.write("../x", "λ", Some(5), &[copy("u3", 3)], 300);
        assert!(pages.read("../x", "λ", Some(5)).is_some() && pages.read("../x", "λ", None).is_none());
        // Past the budget, the oldest go.
        for no in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            pages.write("4chan", "g", Some(no), &[copy("t", no as i64)], 400);
        }
        assert!(pages.read("4chan", "g", Some(9)).is_some());
        assert!(pages.read("4chan", "g", None).is_none());
        // A broken file is dropped.
        std::fs::write(pages.path("4chan", "g", Some(9)), "{").unwrap();
        assert!(pages.read("4chan", "g", Some(9)).is_none());
        assert!(!pages.path("4chan", "g", Some(9)).exists());
    }
}
