//! Saving a post's or a thread's files to disk.

use std::path::{Path, PathBuf};

use crate::model::Post;

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

/// The directory for a thread's files, from the `download_dir` template.
pub fn dir(template: Option<&str>, site: &str, board: &str, thread: u64) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let downloads = downloads();
    let t = template.unwrap_or(DEFAULT_DIR);
    let t = match t.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home.display()),
        None => t.to_string(),
    };
    let path = t
        .replace("{downloads}", &downloads.display().to_string())
        .replace("{site}", &sanitize(site))
        .replace("{board}", &sanitize(board))
        .replace("{thread}", &thread.to_string());
    PathBuf::from(path)
}

/// `(url, destination)` for every file of `posts`. Names are the post number and the
/// original file name, plus the file's position when a post has two files with one name,
/// so they're unique within a thread and the same on every run (existing files get skipped).
pub fn jobs(posts: &[&Post], dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for p in posts {
        let names: Vec<String> = p.files.iter().map(|f| sanitize(&f.filename)).collect();
        for (i, (f, name)) in p.files.iter().zip(&names).enumerate() {
            let dup = names.iter().filter(|n| *n == name).count() > 1;
            let file = if dup { format!("{}_{}_{name}", p.no, i + 1) } else { format!("{}_{name}", p.no) };
            out.push((f.url.clone(), dir.join(file)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Attachment;

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
        let file = |name: &str| Attachment { filename: name.into(), url: format!("https://x/{name}"), ..Default::default() };
        let a = Post { no: 1, files: vec![file("a.png"), file("a.png"), file("b.png")], ..Default::default() };
        let b = Post { no: 2, files: vec![file("a.png")], ..Default::default() };
        let names: Vec<_> = jobs(&[&a, &b], Path::new("/d"))
            .into_iter()
            .map(|(_, p)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["1_1_a.png", "1_2_a.png", "1_b.png", "2_a.png"]);
    }

    #[test]
    fn dir_template() {
        let d = dir(Some("/tmp/x/{site}/{board}/{thread}"), "4chan", "g/../", 5);
        assert_eq!(d, PathBuf::from("/tmp/x/4chan/g_.._/5"));
        assert!(dir(None, "s", "b", 1).ends_with("ck/s/b/1"));
    }
}
