//! Saved threads: the last good copy of each watched (or exported) thread, kept in the data
//! directory under `threads/<site>/<board>/<no>.json`, so it can be read after the thread is
//! gone. Posts keep their look: each line is runs of text with the markup's marker style.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::{Deserialize, Serialize};

use crate::model::{Anchor, Attachment, Link, Post};
use crate::store::{ThreadKey, write_atomic};
use crate::theme::mark;

/// The format of a saved thread's file.
pub const VERSION: u32 = 1;

/// What the Saved view lists about a saved thread (kept together in `saved.json`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SavedMeta {
    #[serde(flatten)]
    pub key: ThreadKey,
    pub subject: String,
    /// When it was last saved (Unix seconds).
    pub saved: i64,
    /// The thread 404'd after it was saved.
    #[serde(default)]
    pub dead: bool,
    /// Size of its file.
    pub bytes: u64,
    /// How many posts, and the newest one, when saved: a thread without new posts isn't
    /// written again.
    pub posts: usize,
    pub newest: u64,
    /// A hash of the posts as saved.
    #[serde(default)]
    pub hash: u64,
}

/// A saved thread's file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedThread {
    pub version: u32,
    pub site: String,
    pub board: String,
    pub no: u64,
    pub subject: String,
    pub saved: i64,
    #[serde(default)]
    pub dead: bool,
    pub url: String,
    pub posts: Vec<SavedPost>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedPost {
    pub no: u64,
    pub name: String,
    pub subject: Option<String>,
    pub time: i64,
    pub body: Vec<SavedLine>,
    pub quotes: Vec<u64>,
    pub links: Vec<Link>,
    pub urls: Vec<String>,
    pub anchors: Vec<Anchor>,
    pub files: Vec<Attachment>,
    pub replies: Option<u32>,
    pub images: Option<u32>,
    pub sticky: bool,
    pub locked: bool,
    pub bumplimit: bool,
    pub board: Option<String>,
}

/// A body line: a code block's line or not, and its runs of text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedLine {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub code: bool,
    pub runs: Vec<Run>,
}

/// Text with its look: the markup's marker (by name; plain when left out), the marker under
/// it (a spoiler's), and its modifiers (`bold`, `italic`, `underlined`, `crossed_out`...).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Run {
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bg: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
}

/// The markup's marker colors (see `theme::mark`), by the name a saved run gives them.
const MARKS: [(&str, Color); 8] = [
    ("greentext", mark::GREENTEXT),
    ("pinktext", mark::PINKTEXT),
    ("quotelink", mark::QUOTELINK),
    ("heading", mark::HEADING),
    ("code", mark::CODE),
    ("spoiler", mark::SPOILER),
    ("revealed", mark::REVEALED),
    ("link", mark::LINK),
];

fn mark_name(c: Option<Color>) -> String {
    MARKS.iter().find(|(_, m)| Some(*m) == c).map_or("", |(n, _)| n).to_string()
}

/// A marker by name; an unknown one (from a newer ck) is plain.
fn mark_color(name: &str) -> Option<Color> {
    MARKS.iter().find(|(n, _)| *n == name).map(|(_, m)| *m)
}

fn flag_names(m: Modifier) -> Vec<String> {
    m.iter_names().map(|(n, _)| n.to_ascii_lowercase()).collect()
}

fn flags(names: &[String]) -> Modifier {
    names.iter().filter_map(|n| Modifier::from_name(&n.to_ascii_uppercase())).fold(Modifier::empty(), |a, b| a | b)
}

impl From<&Span<'static>> for Run {
    fn from(s: &Span<'static>) -> Self {
        Run { text: s.content.to_string(), kind: mark_name(s.style.fg), bg: mark_name(s.style.bg), flags: flag_names(s.style.add_modifier) }
    }
}

impl From<Run> for Span<'static> {
    fn from(r: Run) -> Self {
        let style = Style { fg: mark_color(&r.kind), bg: mark_color(&r.bg), add_modifier: flags(&r.flags), ..Style::new() };
        Span::styled(r.text, style)
    }
}

impl From<&Post> for SavedPost {
    fn from(p: &Post) -> Self {
        let body = p
            .body
            .iter()
            .map(|l| SavedLine {
                code: l.style == crate::markup::CODE_LINE,
                runs: l.spans.iter().map(Run::from).collect(),
            })
            .collect();
        SavedPost {
            no: p.no,
            name: p.name.clone(),
            subject: p.subject.clone(),
            time: p.time,
            body,
            quotes: p.quotes.clone(),
            links: p.links.clone(),
            urls: p.urls.clone(),
            anchors: p.anchors.clone(),
            files: p.files.clone(),
            replies: p.replies,
            images: p.images,
            sticky: p.sticky,
            locked: p.locked,
            bumplimit: p.bumplimit,
            board: p.board.clone(),
        }
    }
}

impl From<SavedPost> for Post {
    fn from(p: SavedPost) -> Self {
        let body = p
            .body
            .into_iter()
            .map(|l| {
                let line = Line::from(l.runs.into_iter().map(Span::from).collect::<Vec<_>>());
                if l.code { line.style(crate::markup::CODE_LINE) } else { line }
            })
            .collect();
        Post {
            no: p.no,
            name: p.name,
            subject: p.subject,
            time: p.time,
            body,
            quotes: p.quotes,
            links: p.links,
            urls: p.urls,
            anchors: p.anchors,
            files: p.files,
            replies: p.replies,
            images: p.images,
            sticky: p.sticky,
            locked: p.locked,
            bumplimit: p.bumplimit,
            board: p.board,
            ..Default::default()
        }
    }
}

/// Where a thread's copy is kept.
pub fn path(dir: &Path, key: &ThreadKey) -> PathBuf {
    dir.join("threads").join(component(&key.site)).join(component(&key.board)).join(format!("{}.json", key.no))
}

/// A site or board name as a folder name every filesystem takes (APFS refuses unassigned
/// characters, for one): letters, digits, `-`, `_` and `.`, the rest `_`, and then a hash of
/// the name, so names that differ only there stay apart.
pub(crate) fn component(name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let kept: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    let clean = crate::download::sanitize(&kept);
    if clean == name {
        return clean;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    format!("{clean}-{:08x}", h.finish() as u32)
}

/// Write a thread's copy; its size.
pub fn write(dir: &Path, t: &SavedThread) -> Result<u64> {
    let bytes = serde_json::to_vec(t)?;
    let key = ThreadKey { site: t.site.clone(), board: t.board.clone(), no: t.no };
    write_atomic(&path(dir, &key), &bytes)?;
    Ok(bytes.len() as u64)
}

/// Read a thread's copy. One that won't load is moved aside (like the other data files)
/// rather than lost.
pub fn read(dir: &Path, key: &ThreadKey) -> Result<SavedThread> {
    let file = path(dir, key);
    let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
    match serde_json::from_slice::<SavedThread>(&bytes) {
        Ok(t) => Ok(t),
        Err(e) => {
            let _ = std::fs::rename(&file, file.with_extension("json.corrupt"));
            Err(e).with_context(|| format!("{} was corrupt; moved it aside", file.display()))
        }
    }
}

/// The list of saved threads, from their files (when `saved.json` is missing or broken).
pub fn scan(dir: &Path) -> Vec<SavedMeta> {
    let mut out = Vec::new();
    let read_dir = |p: &Path| std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).collect::<Vec<_>>();
    for site in read_dir(&dir.join("threads")) {
        for board in read_dir(&site) {
            for file in read_dir(&board).into_iter().filter(|f| f.extension().is_some_and(|e| e == "json")) {
                let Ok(bytes) = std::fs::read(&file) else { continue };
                let Ok(t) = serde_json::from_slice::<SavedThread>(&bytes) else { continue };
                out.push(SavedMeta {
                    key: ThreadKey { site: t.site, board: t.board, no: t.no },
                    subject: t.subject,
                    saved: t.saved,
                    dead: t.dead,
                    bytes: bytes.len() as u64,
                    posts: t.posts.len(),
                    newest: t.posts.iter().map(|p| p.no).max().unwrap_or(0),
                    hash: 0,
                });
            }
        }
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.saved));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{Flavor, parse_html};

    fn same(a: &Post, b: &Post) {
        assert_eq!(a.body, b.body, "post {}", a.no);
        assert_eq!((a.no, &a.name, &a.subject, a.time), (b.no, &b.name, &b.subject, b.time));
        assert_eq!((&a.quotes, &a.links, &a.urls, &a.anchors, &a.files), (&b.quotes, &b.links, &b.urls, &b.anchors, &b.files));
        assert_eq!((a.replies, a.images, a.sticky, a.locked, a.bumplimit, &a.board), (b.replies, b.images, b.sticky, b.locked, b.bumplimit, &b.board));
    }

    fn round_trip(p: &Post) -> Post {
        let json = serde_json::to_string(&SavedPost::from(p)).unwrap();
        serde_json::from_str::<SavedPost>(&json).unwrap().into()
    }

    #[test]
    fn posts_keep_their_look() {
        for (site, posts) in crate::backend::fixture_threads() {
            assert!(!posts.is_empty(), "{site}");
            for p in &posts {
                same(p, &round_trip(p));
            }
        }
        // Code blocks, spoilers, greentext, quote links and headings, from every engine.
        let samples = crate::backend::fixture("markup_samples.json");
        for (name, html) in samples.as_object().unwrap() {
            let flavor = match name.split('_').next().unwrap() {
                "4chan" => Flavor::Fourchan,
                "vichan" => Flavor::Vichan,
                _ => Flavor::Lynxchan,
            };
            let parsed = parse_html(html.as_str().unwrap(), flavor);
            let p = Post { no: 1, body: parsed.lines, anchors: parsed.anchors, ..Default::default() };
            assert!(!p.body.is_empty(), "{name}");
            same(&p, &round_trip(&p));
        }
    }

    #[test]
    fn files_by_thread_and_a_corrupt_one_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let posts: Vec<SavedPost> = crate::backend::fixture_threads().remove(0).1.iter().map(SavedPost::from).collect();
        let t = SavedThread { version: VERSION, site: "4chan".into(), board: "g".into(), no: 1, subject: "s".into(), saved: 5, dead: false, url: "u".into(), posts };
        let size = write(dir.path(), &t).unwrap();
        let key = ThreadKey { site: "4chan".into(), board: "g".into(), no: 1 };
        assert_eq!(path(dir.path(), &key), dir.path().join("threads/4chan/g/1.json"));
        assert_eq!(read(dir.path(), &key).unwrap().posts, t.posts);
        let listed = scan(dir.path());
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].bytes, listed[0].posts, &listed[0].key), (size, t.posts.len(), &key));
        // A board name can't climb out of the directory, and only plain characters reach the
        // filesystem (an unassigned one made APFS refuse the folder; found by fuzzing).
        let odd = ThreadKey { site: "../x".into(), board: "../../y".into(), no: 2 };
        assert!(path(dir.path(), &odd).starts_with(dir.path().join("threads")));
        assert!(!path(dir.path(), &odd).components().any(|c| c == std::path::Component::ParentDir));
        let odd = ThreadKey { site: "4chan".into(), board: "AZf9b\u{af4}\u{594}fy".into(), no: 3 };
        let p = path(dir.path(), &odd);
        assert!(p.to_str().unwrap().is_ascii(), "{p:?}");
        let t = SavedThread { site: odd.site.clone(), board: odd.board.clone(), no: 3, ..t.clone() };
        write(dir.path(), &t).unwrap();
        assert_eq!(read(dir.path(), &odd).unwrap().board, odd.board);
        // Unicode boards stay readable; different odd names stay apart.
        assert!(path(dir.path(), &ThreadKey { site: "lainchan".into(), board: "λ".into(), no: 1 }).ends_with("lainchan/λ/1.json"));
        let a = ThreadKey { board: "a?b".into(), ..odd.clone() };
        let b = ThreadKey { board: "a*b".into(), ..odd.clone() };
        assert_ne!(path(dir.path(), &a), path(dir.path(), &b));

        std::fs::write(path(dir.path(), &key), b"{ not json").unwrap();
        assert!(read(dir.path(), &key).is_err());
        assert!(dir.path().join("threads/4chan/g/1.json.corrupt").exists());
        assert_eq!(scan(dir.path()).iter().map(|m| m.key.no).collect::<Vec<_>>(), [3]);
    }
}
