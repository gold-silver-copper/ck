//! A bounded on-disk cache of thumbnail bytes, keyed by URL, in $XDG_CACHE_HOME/ck/thumbs.
//! Thumbnails never change at a given URL, so a hit needs no request at all.

use crate::http::lock;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

/// Default size limit; the oldest files go first when it's exceeded.
pub const BUDGET: u64 = 200 * 1024 * 1024;

pub struct DiskCache {
    dir: PathBuf,
    budget: u64,
    /// Bytes in the cache, counted on first write.
    used: Mutex<Option<u64>>,
}

impl DiskCache {
    pub fn default_dir() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))?;
        Some(base.join("ck").join("thumbs"))
    }

    pub fn new(dir: PathBuf, budget: u64) -> Self {
        Self { dir, budget, used: Mutex::new(None) }
    }

    fn path(&self, url: &str) -> PathBuf {
        self.dir.join(format!("{:032x}", fnv1a128(url.as_bytes())))
    }

    pub fn contains(&self, url: &str) -> bool {
        self.path(url).is_file()
    }

    /// The cached bytes, marking the entry as recently used.
    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        let path = self.path(url);
        let bytes = fs::read(&path).ok()?;
        #[allow(clippy::disallowed_methods)] // opened to touch its time, never written
        let touched = fs::File::options().write(true).open(&path);
        if let Ok(f) = touched {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(bytes)
    }

    pub fn remove(&self, url: &str) {
        if let Ok(meta) = fs::metadata(self.path(url))
            && fs::remove_file(self.path(url)).is_ok()
            && let Some(used) = lock(&self.used).as_mut()
        {
            *used = used.saturating_sub(meta.len());
        }
    }

    /// Store bytes (through a temp file renamed into place), then trim to the budget.
    pub fn put(&self, url: &str, bytes: &[u8]) -> anyhow::Result<()> {
        crate::atomic::write(&self.path(url), bytes)?;
        let mut used = lock(&self.used);
        let total = match *used {
            Some(n) => n + bytes.len() as u64,
            None => crate::atomic::total(&self.dir, 0),
        };
        *used = Some(if total > self.budget { crate::atomic::trim(&self.dir, 0, self.budget * 9 / 10) } else { total });
        Ok(())
    }
}

/// 128-bit FNV-1a: stable across Rust versions, unlike `DefaultHasher`.
fn fnv1a128(data: &[u8]) -> u128 {
    let mut h: u128 = 0x6c62272e07bb014262b821756295c58d;
    for &b in data {
        h ^= b as u128;
        h = h.wrapping_mul(0x0000000001000000000000000000013B);
    }
    h
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn roundtrip_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path().join("thumbs"), 1000);
        assert!(!c.contains("u") && c.get("u").is_none());
        c.put("u", b"abc").unwrap();
        assert!(c.contains("u"));
        assert_eq!(c.get("u").unwrap(), b"abc");
        assert!(!c.contains("v"));
        c.remove("u");
        assert!(c.get("u").is_none());
        // No temp files left behind.
        assert_eq!(fs::read_dir(dir.path().join("thumbs")).unwrap().count(), 0);
    }

    #[test]
    fn evicts_least_recently_used() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path().to_path_buf(), 250);
        let old = SystemTime::now() - Duration::from_secs(3600);
        for (i, url) in ["a", "b", "c"].iter().enumerate() {
            c.put(url, &[0; 100][..80]).unwrap();
            let f = fs::File::options().write(true).open(c.path(url)).unwrap();
            f.set_modified(old + Duration::from_secs(i as u64 * 60)).unwrap();
        }
        // Reading "a" makes it the most recently used.
        c.get("a").unwrap();
        // 4 x 80 = 320 > 250: trimmed to at most 225, oldest ("b") first.
        c.put("d", &[0; 80]).unwrap();
        assert!(!c.contains("b"));
        assert!(c.contains("a") && c.contains("d"));
        assert!(crate::atomic::total(dir.path(), 0) <= 225);
    }

    #[test]
    fn stable_names() {
        assert_eq!(fnv1a128(b""), 0x6c62272e07bb014262b821756295c58d);
        assert_ne!(fnv1a128(b"https://x/1s.jpg"), fnv1a128(b"https://x/2s.jpg"));
    }

    #[test]
    fn eviction_spares_another_writers_temp_file() {
        // Another image worker is midway through a put: its temp file is beside the
        // entries, under the name ck gives one now or gave one before. Trimming must leave it be.
        for tag in [String::new(), format!(".ck-{}-0", std::process::id())] {
            let dir = tempfile::tempdir().unwrap();
            let c = DiskCache::new(dir.path().to_path_buf(), 250);
            let tmp = dir.path().join(format!("{:032x}{tag}.tmp", fnv1a128(b"busy")));
            fs::write(&tmp, [0; 200]).unwrap();
            let f = fs::File::options().write(true).open(&tmp).unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
            c.put("a", &[0; 80]).unwrap();
            assert!(tmp.exists(), "an in-flight temp file was evicted");
            assert!(c.contains("a"));
        }
    }
}
