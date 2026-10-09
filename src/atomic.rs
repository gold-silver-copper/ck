//! The one way ck lands a file (whole or not at all, through a temp file of its own), reads
//! a data file (and may then write it), and lists and trims a cache folder (never counting
//! another writer's temp).
#![allow(clippy::disallowed_methods)] // this module is the owner the lint points to

use anyhow::{Context as _, Result};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// A temp file this old was left by a crash or kill; no live write takes this long.
const STALE: Duration = Duration::from_secs(24 * 3600);

/// Write `data` to `path` whole or not at all. A symlink is written through to its target
/// (even one that isn't there yet), so a linked config or data file stays linked.
pub fn write(path: &Path, data: &[u8]) -> Result<()> {
    write_from(&link_target(path), data)
}

/// Write what `body` reads into `path` through a temp file of this write's own beside it,
/// renamed into place, and removed on any error or panic. A symlink at `path` is replaced,
/// not written through (a download never lands outside its folder). The folder is created
/// if missing, and an existing file keeps its permissions. The first write into a folder in
/// a run removes the stale temps a crash left there, so they can't pile up.
pub fn write_from(path: &Path, mut body: impl std::io::Read) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    static SWEPT: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        if crate::http::lock(&SWEPT).insert(dir.to_path_buf()) {
            listing(dir, 0).1.into_iter().filter(|t| stale(t.1) && ours(&t.0)).for_each(|t| drop(std::fs::remove_file(t.0)));
        }
    }
    let mode = std::fs::symlink_metadata(path).ok().filter(std::fs::Metadata::is_file).map(|m| m.permissions());
    let landed = (|| -> std::io::Result<()> {
        let (mut tmp, mut file) = create_temp(path, &NEXT)?;
        // Before the content goes in, so a private file is never readable by others.
        if let Some(mode) = mode {
            file.set_permissions(mode)?;
        }
        std::io::copy(&mut body, &mut file)?;
        drop(file);
        std::fs::rename(&tmp.0, path)?;
        tmp.0 = PathBuf::new(); // landed: nothing for the drop to remove
        Ok(())
    })();
    landed.with_context(|| format!("writing {}", path.display()))
}

/// Unpack the tar archive `archive` into the folder `dir`, whole or not at all: into a folder
/// of its own beside it, renamed into place (replacing what was there), and removed on any
/// error.
pub fn unpack(archive: impl std::io::Read, dir: &Path) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = dir.parent().context("no folder to unpack into")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut tmp = TempDir(parent.join(format!("{name}.ck-{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))));
    let landed = (|| -> std::io::Result<()> {
        tar::Archive::new(archive).unpack(&tmp.0)?;
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        std::fs::rename(&tmp.0, dir)?;
        tmp.0 = PathBuf::new();
        Ok(())
    })();
    landed.with_context(|| format!("unpacking into {}", dir.display()))
}

/// Remove the folders in `dir` but `keep` (what older versions unpacked there).
pub fn remove_other_dirs(dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path != keep && e.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// A folder being unpacked, removed when dropped unless it was renamed into place.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// A temp file, removed when dropped (by an error's early return or a panic's unwinding)
/// unless it was renamed into place.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

/// A fresh `<path>.ck-<pid>-<n>.tmp`, created with create_new: no other write can share it. On
/// a clash (a leftover from a dead process with the same pid) the next n is taken.
fn create_temp(path: &Path, next: &AtomicU64) -> std::io::Result<(Temp, std::fs::File)> {
    let mut clash = std::io::Error::from(std::io::ErrorKind::AlreadyExists);
    for _ in 0..1000 {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(format!(".ck-{}-{}.tmp", std::process::id(), next.fetch_add(1, Ordering::Relaxed)));
        match std::fs::File::options().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((Temp(tmp.into()), file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => clash = e,
            Err(e) => return Err(e),
        }
    }
    Err(clash)
}

/// Where a write to `path` should go: the file a symlink points to (even one that isn't
/// there yet), or `path` itself.
fn link_target(path: &Path) -> PathBuf {
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return path.to_path_buf();
    }
    std::fs::canonicalize(path)
        .or_else(|_| std::fs::read_link(path).map(|to| path.parent().map_or_else(|| to.clone(), |dir| dir.join(&to))))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// What reading a JSON data file found, with the permission to write it (`Owned`) whenever
/// what's on disk is known: not when it couldn't be read.
pub enum Read<T> {
    Missing(Owned),
    Loaded(T, Owned),
    /// It wouldn't load (not UTF-8 included), and was moved aside to `<x>.json.corrupt`.
    Corrupt(String, Owned),
    /// It couldn't be read (permissions, a folder in its place), or moved aside.
    Unreadable(String),
}

pub fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Read<T> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        // Under a file instead of a folder, it can't be there either.
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => return Read::Missing(Owned::new(path)),
        Err(e) => return Read::Unreadable(format!("Couldn't read {} ({e})", path.display())),
    };
    let e = match serde_json::from_slice(&bytes) {
        Ok(v) => return Read::Loaded(v, Owned::new(path)),
        Err(e) => e,
    };
    let aside = path.with_extension("json.corrupt");
    match std::fs::rename(path, &aside) {
        Ok(()) => Read::Corrupt(format!("{} was corrupt ({e}); moved it to {}", path.display(), aside.display()), Owned::new(path)),
        Err(r) => Read::Unreadable(format!("Couldn't read {}: corrupt ({e}), and couldn't be moved aside ({r})", path.display())),
    }
}

/// The permission to write one data file, and a hash of what was last read or written there.
pub struct Owned {
    path: PathBuf,
    last: Cell<Option<u64>>,
}

impl Owned {
    fn new(path: &Path) -> Self {
        Owned { path: path.to_path_buf(), last: Cell::new(None) }
    }

    /// Write `bytes` to the file unless they're what's there.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        if self.last.get() != Some(hash(bytes)) {
            write(&self.path, bytes)?;
            self.mark(bytes);
        }
        Ok(())
    }

    /// Count `bytes` (what was just read, as ck would write it) as what's there.
    pub fn mark(&self, bytes: &[u8]) {
        self.last.set(Some(hash(bytes)));
    }
}

fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Whether `path` is a temp file: ours, an older version's `<x>.tmp`, or an old download's `.part`.
fn is_temp(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "tmp" || e == "part")
}

/// Whether `path` is named the way ck names a temp file, `<x>.ck-<pid>-<n>.tmp`: outside
/// the caches, a user's own `notes.tmp` (or a downloaded `clip.2024-05.tmp`) is left alone.
fn ours(path: &Path) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let tag = path.file_stem().map(Path::new).and_then(Path::extension).and_then(|e| e.to_str());
    let tag = tag.and_then(|t| t.strip_prefix("ck-")).and_then(|t| t.split_once('-'));
    path.extension().is_some_and(|e| e == "tmp") && tag.is_some_and(|(pid, n)| digits(pid) && digits(n))
}

/// Whether a temp file last modified then is a crash's leftover. One whose time can't be
/// read is taken for a live one.
fn stale(modified: Option<SystemTime>) -> bool {
    modified.and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|age| age > STALE)
}

/// The regular files under `dir`, down to `depth` folders below it (path, size, last
/// modified), apart from them the temp files (path, last modified), and what couldn't be
/// listed.
type Listing = (Vec<(PathBuf, u64, SystemTime)>, Vec<(PathBuf, Option<SystemTime>)>, Vec<String>);

fn listing(dir: &Path, depth: u8) -> Listing {
    let mut listing = Listing::default();
    walk(dir, depth, &mut listing);
    listing
}

fn walk(dir: &Path, depth: u8, into: &mut Listing) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // Not made yet, removed since it was listed, or a file in its place: nothing in it.
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => return,
        Err(e) => return into.2.push(format!("Couldn't list {} ({e})", dir.display())),
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                into.2.push(format!("Couldn't look at {} ({e})", path.display()));
                continue;
            }
        };
        if meta.is_dir() && depth > 0 {
            walk(&path, depth - 1, into);
        } else if meta.is_file() && is_temp(&path) {
            into.1.push((path, meta.modified().ok()));
        } else if meta.is_file() {
            into.0.push((path, meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
        }
    }
}

/// The files under `dir`, down to `depth` folders below it, temp files left out, and what
/// under it couldn't be listed. (A symlinked folder is followed; `depth` keeps a loop of
/// them finite.)
pub fn files(dir: &Path, depth: u8) -> (Vec<PathBuf>, Vec<String>) {
    let (files, _, unlisted) = listing(dir, depth);
    (files.into_iter().map(|f| f.0).collect(), unlisted)
}

/// The bytes the files under `dir` take, down to `depth` folders below it, temp files left out.
pub fn total(dir: &Path, depth: u8) -> u64 {
    listing(dir, depth).0.iter().fold(0, |sum, f| sum.saturating_add(f.1))
}

/// For a cache folder, down to `depth` folders below it: remove temp files older than a
/// day, then the least recently modified files until at most `target` bytes remain; the
/// bytes left. A younger temp file is another write in flight: neither counted nor removed.
pub fn trim(dir: &Path, depth: u8, target: u64) -> u64 {
    let (mut files, temps, _) = listing(dir, depth);
    temps.into_iter().filter(|t| stale(t.1)).for_each(|t| drop(std::fs::remove_file(t.0)));
    files.sort_by_key(|f| f.2);
    let mut left = files.iter().fold(0u64, |sum, f| sum.saturating_add(f.1));
    for (path, len, _) in files {
        if left <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            left = left.saturating_sub(len);
        }
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(dir: &Path) -> Vec<std::ffi::OsString> {
        let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        names
    }

    fn age(path: &Path, by: Duration) {
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - by).unwrap();
    }

    #[test]
    fn unpacks_a_folder_whole_or_not_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "bin/ck-web", &b"hi"[..]).unwrap();
        let archive = tar.into_inner().unwrap();
        let to = dir.path().join("web").join("1");
        std::fs::create_dir_all(to.join("old")).unwrap();
        unpack(&archive[..], &to).unwrap();
        assert_eq!(std::fs::read(to.join("bin/ck-web")).unwrap(), b"hi");
        assert!(!to.join("old").exists());
        // A broken archive leaves what was there, and no temp.
        assert!(unpack(&archive[..100], &to).is_err());
        assert!(to.join("bin/ck-web").exists());
        assert_eq!(names(&dir.path().join("web")), ["1"]);
    }

    #[test]
    fn creates_the_folder_and_keeps_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c.json");
        write(&path, b"one").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            write(&path, b"two").unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"two");
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(names(&dir.path().join("a/b")), vec![std::ffi::OsString::from("c.json")]);
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.toml");
        let link = dir.path().join("link.toml");
        std::os::unix::fs::symlink("real.toml", &link).unwrap();
        // Even when what it points to isn't there yet.
        write(&link, b"one").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&real).unwrap(), b"one");
        write(&link, b"two").unwrap();
        assert_eq!(std::fs::read(&real).unwrap(), b"two");
    }

    #[test]
    fn two_writes_of_one_file_at_once_both_land_whole() {
        // Pages::write runs on request threads: two tabs on one thread write its page at
        // once. Each write must land whole, and neither may fail for the other's sake.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.json");
        let payloads: Vec<Vec<u8>> = (0..4u8).map(|b| vec![b'a' + b; 256 * 1024]).collect();
        let results: Vec<Vec<String>> = std::thread::scope(|s| {
            let writers: Vec<_> = payloads
                .iter()
                .map(|data| {
                    let (path, payloads) = (&path, &payloads);
                    s.spawn(move || {
                        let mut errors = Vec::new();
                        for _ in 0..50 {
                            if let Err(e) = write(path, data) {
                                errors.push(format!("{e:#}"));
                            }
                            let now = std::fs::read(path).unwrap_or_default();
                            if !payloads.contains(&now) {
                                errors.push(format!("torn file: {} bytes", now.len()));
                            }
                        }
                        errors
                    })
                })
                .collect();
            writers.into_iter().map(|w| w.join().unwrap()).collect()
        });
        let errors: Vec<&String> = results.iter().flatten().collect();
        assert!(errors.is_empty(), "{} failed writes, e.g. {:?}", errors.len(), errors.iter().take(3).collect::<Vec<_>>());
    }

    #[test]
    fn a_failed_write_leaves_no_temp_file() {
        // The rename fails (a folder is in the way): nothing but the folder stays.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("watched.json");
        std::fs::create_dir_all(path.join("in-the-way")).unwrap();
        assert!(write(&path, b"[]").is_err());
        assert_eq!(names(dir.path()), vec![std::ffi::OsString::from("watched.json")]);
    }

    #[test]
    fn overlapping_downloads_of_a_file_both_land() {
        /// Reads `data`, but first lets another download of the same file run start to end.
        struct Overlapped<'a>(&'a Path, Option<&'static [u8]>, &'static [u8]);
        impl std::io::Read for Overlapped<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if let Some(other) = self.1.take() {
                    write_from(self.0, other).unwrap();
                }
                std::io::Read::read(&mut self.2, buf)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.png");
        write_from(&path, Overlapped(&path, Some(b"first"), b"second")).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        // No temp file is left behind.
        assert_eq!(names(dir.path()), vec![std::ffi::OsString::from("1.png")]);
    }

    #[test]
    fn a_leftover_temp_of_the_same_name_is_stepped_over_and_kept() {
        // A dead process with this pid (or another pid namespace) left temps under the
        // names the next writes would take: they're neither written into nor truncated.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.json");
        let leftovers: Vec<PathBuf> = (0..200).map(|n| dir.path().join(format!("1.json.ck-{}-{n}.tmp", std::process::id()))).collect();
        for l in &leftovers {
            std::fs::write(l, b"leftover").unwrap();
        }
        let next = AtomicU64::new(0);
        let (tmp, _) = create_temp(&path, &next).unwrap();
        assert_eq!(tmp.0, dir.path().join(format!("1.json.ck-{}-200.tmp", std::process::id())));
        for l in &leftovers {
            assert_eq!(std::fs::read(l).unwrap(), b"leftover");
        }
        drop(tmp);
        assert_eq!(names(dir.path()).len(), 200);
    }

    #[test]
    fn a_panic_midway_leaves_no_temp_file() {
        struct Panics;
        impl std::io::Read for Panics {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("the body reader broke");
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.png");
        assert!(crate::guard::result(|| write_from(&path, Panics)).is_err());
        assert!(names(dir.path()).is_empty());
    }

    #[test]
    fn the_first_write_into_a_folder_removes_a_crashs_leftovers_there() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("history.json.ck-4242-7.tmp");
        let live = dir.path().join("history.json.ck-4243-0.tmp");
        let mine = dir.path().join("notes.tmp");
        for p in [&old, &live, &mine] {
            std::fs::write(p, b"x").unwrap();
        }
        age(&old, STALE * 2);
        age(&mine, STALE * 2);
        age(&live, Duration::from_secs(60));
        write(&dir.path().join("history.json"), b"{}").unwrap();
        // A stale temp of ck's own naming goes; a live one, and a user's own file, stay.
        assert!(!old.exists() && live.exists() && mine.exists());
        // Only once per folder per run.
        std::fs::write(&old, b"x").unwrap();
        age(&old, STALE * 2);
        write(&dir.path().join("history.json"), b"{}").unwrap();
        assert!(old.exists());
    }

    #[test]
    fn ours_knows_cks_temp_names() {
        assert!(ours(Path::new("a/1.json.ck-12-0.tmp")));
        assert!(!ours(Path::new("1_cat.png.12-3.part")));
        assert!(!ours(Path::new("123_clip.2024-05.tmp")));
        assert!(!ours(Path::new("history.json.tmp")));
        assert!(!ours(Path::new("notes.tmp")));
        assert!(!ours(Path::new("a.b-c.tmp")));
        assert!(!ours(Path::new("1.json.ck-12-0.json")));
    }

    #[cfg(unix)]
    #[test]
    fn listings_follow_a_linked_folder_and_keep_to_regular_files() {
        let (dir, elsewhere) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::create_dir_all(elsewhere.path().join("g")).unwrap();
        std::fs::write(elsewhere.path().join("g/1.json"), b"{}").unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), dir.path().join("4chan")).unwrap();
        // A link to a file it doesn't own, and one to nothing, aren't files of the folder.
        std::os::unix::fs::symlink("/dev/null", dir.path().join("null")).unwrap();
        std::os::unix::fs::symlink("nowhere", dir.path().join("dangling")).unwrap();
        // A loop of links ends.
        std::os::unix::fs::symlink(dir.path(), elsewhere.path().join("g/loop")).unwrap();
        assert_eq!(files(dir.path(), 2).0, vec![dir.path().join("4chan/g/1.json")]);
    }

    #[cfg(unix)]
    #[test]
    fn a_flat_trim_never_reaches_into_a_linked_folder() {
        // The thumbnail cache is flat: a folder linked into it isn't its to empty.
        let (dir, elsewhere) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(elsewhere.path().join("keep.png"), [0; 100]).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), dir.path().join("linked")).unwrap();
        std::fs::write(dir.path().join("thumb"), [0; 100]).unwrap();
        assert_eq!(total(dir.path(), 0), 100);
        assert_eq!(trim(dir.path(), 0, 0), 0);
        assert!(elsewhere.path().join("keep.png").exists() && !dir.path().join("thumb").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_from_replaces_a_symlink_instead_of_writing_through_it() {
        // A download onto a name some link already holds lands in the folder, not out of it.
        let (dir, elsewhere) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let outside = elsewhere.path().join("x.png");
        let path = dir.path().join("1_x.png");
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        write_from(&path, &b"img"[..]).unwrap();
        assert!(!outside.exists());
        assert!(std::fs::symlink_metadata(&path).unwrap().is_file());
        assert_eq!(std::fs::read(&path).unwrap(), b"img");
    }

    #[test]
    fn trim_sweeps_stale_temps_and_spares_fresh_ones() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join(format!("a.json.ck-{}-0.tmp", std::process::id()));
        let stale = dir.path().join("b/c.json.tmp");
        let part = dir.path().join("d.png.1-2.part");
        std::fs::write(&fresh, [0; 100]).unwrap();
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(&stale, [0; 100]).unwrap();
        std::fs::write(&part, [0; 100]).unwrap();
        std::fs::write(dir.path().join("e.json"), [0; 10]).unwrap();
        age(&fresh, Duration::from_secs(3600));
        age(&stale, STALE + Duration::from_secs(60));
        age(&part, STALE * 3);
        // Temps aren't counted or listed.
        assert_eq!(total(dir.path(), 2), 10);
        assert_eq!(files(dir.path(), 2).0, vec![dir.path().join("e.json")]);
        // Even with nothing allowed, a write in flight stays; a crash's leftovers go.
        assert_eq!(trim(dir.path(), 2, 0), 0);
        assert!(fresh.exists());
        assert!(!stale.exists() && !part.exists());
        assert!(!dir.path().join("e.json").exists());
    }

    #[test]
    fn trim_removes_the_least_recently_modified_first() {
        let dir = tempfile::tempdir().unwrap();
        for (i, name) in ["old", "mid", "new"].iter().enumerate() {
            let p = dir.path().join("x").join(name);
            write(&p, &[0; 100]).unwrap();
            age(&p, Duration::from_secs(600 - i as u64 * 60));
        }
        assert_eq!(trim(dir.path(), 1, 250), 200);
        assert_eq!(names(&dir.path().join("x")), vec![std::ffi::OsString::from("mid"), std::ffi::OsString::from("new")]);
    }

    #[test]
    fn a_file_that_wont_load_is_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        std::fs::write(&path, b"{").unwrap();
        assert!(matches!(read::<Vec<u64>>(&path), Read::Corrupt(..)));
        let aside = dir.path().join("history.json.corrupt");
        assert_eq!(std::fs::read(&aside).unwrap(), b"{");
        assert!(matches!(read::<Vec<u64>>(&path), Read::Missing(_)));
        // Where it can't be moved, it can't be written over either.
        std::fs::write(&path, b"{").unwrap();
        std::fs::remove_file(&aside).unwrap();
        std::fs::create_dir_all(aside.join("in-the-way")).unwrap();
        assert!(matches!(read::<Vec<u64>>(&path), Read::Unreadable(_)));
        assert_eq!(std::fs::read(&path).unwrap(), b"{");
    }
}
