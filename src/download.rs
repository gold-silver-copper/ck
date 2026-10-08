//! Saving a post's or a thread's files to disk.

use std::path::{Path, PathBuf};

use crate::model::{Attachment, Post};
use crate::store::ThreadKey;

/// Default for `download_dir`. `{downloads}` is the system's Downloads folder.
const DEFAULT_DIR: &str = "{downloads}/ck/{site}/{board}/{thread}";

/// Make a file name from a server safe to use: no path separators, control characters, or
/// characters Windows rejects; no leading dots (no hidden files, no `..`); not too long.
pub fn sanitize(name: &str) -> String {
    let bad = |c: char| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|');
    let s: String = name.chars().map(|c| if bad(c) { '_' } else { c }).collect();
    // Dots and spaces both, so ". .." can't come out as "..".
    let s = s.trim_start_matches(|c: char| c == '.' || c.is_whitespace()).trim_end();
    let mut out = String::new();
    for c in s.chars() {
        if out.len() + c.len_utf8() > 180 {
            break;
        }
        out.push(c);
    }
    if out.is_empty() { "file".into() } else { out }
}

/// The system's Downloads folder.
fn downloads() -> PathBuf {
    dirs::download_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Downloads"))
}

/// Where files go unless `download_dir` says otherwise (in a folder per site, board and
/// thread).
pub fn default_root() -> PathBuf {
    downloads().join("ck")
}

/// The directory for thread `key`'s files, from the `download_dir` template.
pub fn dir(template: Option<&str>, key: &ThreadKey) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let downloads = downloads();
    let t = template.unwrap_or(DEFAULT_DIR);
    let t = match t.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home.display()),
        None => t.to_string(),
    };
    let path = t
        .replace("{downloads}", &downloads.display().to_string())
        .replace("{site}", &sanitize(&key.site))
        .replace("{board}", &sanitize(&key.board))
        .replace("{thread}", &key.no.to_string());
    PathBuf::from(path)
}

/// Every file of a post with where it's saved. Names are the post number and the original
/// file name, plus the file's position when a post has two files with one name, so they're
/// unique within a thread and the same on every run (existing files get skipped).
pub fn paths<'a>(p: &'a Post, dir: &Path) -> Vec<(&'a Attachment, PathBuf)> {
    let names: Vec<String> = p.files.iter().map(|f| sanitize(&f.filename)).collect();
    let mut out = Vec::new();
    for (i, (f, name)) in p.files.iter().zip(&names).enumerate() {
        let dup = names.iter().filter(|n| *n == name).count() > 1;
        let file = if dup { format!("{}_{}_{name}", p.no, i + 1) } else { format!("{}_{name}", p.no) };
        out.push((f, dir.join(file)));
    }
    out
}

/// `(url, destination)` for every file of `posts` the site has. A file the archive kept
/// only the thumbnail of isn't saved.
pub fn jobs(posts: &[&Post], dir: &Path) -> Vec<(String, PathBuf)> {
    posts.iter().flat_map(|p| paths(p, dir)).filter_map(|(f, path)| Some((f.url.clone()?, path))).collect()
}

/// What saving one of a post's files fetches, and where to: nothing for a file the site
/// has only the thumbnail of.
pub fn job(p: &Post, file: &Attachment, dir: &Path) -> Vec<(String, PathBuf)> {
    paths(p, dir).into_iter().filter_map(|(f, path)| Some((f.url.clone().filter(|_| f.url == file.url)?, path))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizing() {
        assert_eq!(sanitize("cat.jpg"), "cat.jpg");
        assert_eq!(sanitize("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize(".hidden"), "hidden");
        assert_eq!(sanitize("a\\b:c\n.png"), "a_b_c_.png");
        assert_eq!(sanitize(".."), "file");
        // Dots behind spaces are leading dots too.
        assert_eq!(sanitize(". .."), "file");
        assert_eq!(sanitize(" . . .x"), "x");
        assert_eq!(sanitize("  "), "file");
        assert!(sanitize(&"é".repeat(200)).len() <= 180);
    }

    #[test]
    fn unique_names() {
        let file = |name: &str| Attachment { filename: name.into(), ..Attachment::at(format!("https://x/{name}")) };
        let a = Post { no: 1, files: vec![file("a.png"), file("a.png"), file("b.png")], ..Default::default() };
        let b = Post { no: 2, files: vec![file("a.png")], ..Default::default() };
        let names: Vec<_> = jobs(&[&a, &b], Path::new("/d"))
            .into_iter()
            .map(|(_, p)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["1_1_a.png", "1_2_a.png", "1_b.png", "2_a.png"]);
        // One file's job is found by its address: a refresh that changed its size still
        // finds it, and a post with the same file twice saves both.
        let refreshed = Attachment { size: Some(1), ..file("a.png") };
        let names: Vec<_> = job(&a, &refreshed, Path::new("/d")).into_iter().map(|(_, p)| p).collect();
        assert_eq!(names, [Path::new("/d/1_1_a.png"), Path::new("/d/1_2_a.png")]);
        assert!(job(&a, &Attachment { url: None, ..file("a.png") }, Path::new("/d")).is_empty());
    }

    #[test]
    fn dir_template() {
        let d = dir(Some("/tmp/x/{site}/{board}/{thread}"), &ThreadKey { site: "4chan".into(), board: "g/../".into(), no: 5 });
        assert_eq!(d, PathBuf::from("/tmp/x/4chan/g_.._/5"));
        assert!(dir(None, &ThreadKey { site: "s".into(), board: "b".into(), no: 1 }).ends_with("ck/s/b/1"));
    }
}
