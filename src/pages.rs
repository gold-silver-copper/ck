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
        if crate::atomic::write(&self.path(site, board, thread), &bytes).is_ok() {
            crate::atomic::trim(&self.dir, 2, self.budget); // <site>/<board>/<page>
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

    #[test]
    fn pruning_spares_another_writers_temp_file() {
        // Another request thread is midway through writing a page: its temp file sits
        // beside the pages, under the name ck gives one now or gave one before. Pruning
        // must neither count it nor remove it.
        for tag in [String::new(), format!(".ck-{}-0", std::process::id())] {
            let dir = tempfile::tempdir().unwrap();
            let pages = Pages::new(dir.path().to_path_buf(), 1_000);
            let mut tmp = pages.path("4chan", "g", Some(7)).into_os_string();
            tmp.push(format!("{tag}.tmp"));
            let tmp = PathBuf::from(tmp);
            std::fs::create_dir_all(tmp.parent().unwrap()).unwrap();
            std::fs::write(&tmp, "x".repeat(900)).unwrap();
            let f = std::fs::File::options().write(true).open(&tmp).unwrap();
            f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600)).unwrap();
            pages.write("4chan", "g", Some(1), &[copy("u1", 1)], 100);
            assert!(tmp.exists(), "an in-flight temp file was pruned");
            assert!(pages.read("4chan", "g", Some(1)).is_some());
        }
    }
}
