//! Site-agnostic data model. Every backend converts its own JSON into these.

use std::sync::OnceLock;

use ratatui::text::Line;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Board {
    /// URI segment, e.g. `g` or `λ`.
    pub uri: String,
    pub title: String,
    /// `None` when the site doesn't say.
    pub nsfw: Option<bool>,
}

/// Where a quote link points, as far as the markup tells.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Link {
    /// `None`: the current board.
    pub board: Option<String>,
    /// `None`: unknown, or the current thread for a bare `>>123`.
    pub thread: Option<u64>,
    /// `None` for a board link like `>>>/g/`.
    pub post: Option<u64>,
}

/// Where a link in a post's text goes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Target {
    Quote(Link),
    Url(String),
}

/// A link in a post's text: the body line it's on, the byte range of its text in that
/// line, and where it goes. In the order they appear.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Anchor {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub to: Target,
}

/// What a file is, as the site says or its name tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(test, derive(Default))]
pub enum FileKind { Image, Video, #[cfg_attr(test, default)] Other }

impl FileKind {
    /// What a file is: the engine's mime when it says video or an image ck draws, else the
    /// file URL's extension, else the original name's. The only place a kind is decided.
    pub fn of(mime: Option<&str>, url: Option<&str>, filename: &str) -> Self {
        match mime.and_then(|m| m.split_once('/')) {
            Some(("video", _)) => Self::Video,
            Some(("image", sub)) if Self::from_ext(sub) == Self::Image => Self::Image,
            _ => url.and_then(url_ext).or_else(|| ext_of(filename)).map_or(Self::Other, |e| Self::from_ext(&e)),
        }
    }

    /// The one list of extensions ck knows.
    pub fn from_ext(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" => Self::Image,
            "webm" | "mp4" | "mov" | "m4v" | "mkv" => Self::Video,
            _ => Self::Other,
        }
    }
}

/// Lowercase extension of a name's last path segment, without the dot; none when what
/// follows the last dot isn't one (`build v1.2 final`).
fn ext_of(name: &str) -> Option<String> {
    let (_, e) = name.rsplit('/').next().unwrap_or(name).rsplit_once('.')?;
    (!e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric())).then(|| e.to_ascii_lowercase())
}

/// A URL's extension, its query and fragment left out (a file's name may have `?` or `#`).
fn url_ext(url: &str) -> Option<String> {
    ext_of(url.split(['?', '#']).next().unwrap_or(url))
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(Default))]
#[serde(from = "StoredAttachment")]
pub struct Attachment {
    pub filename: String,
    /// The full file; `None` when the site or archive has only the thumbnail. Never the thumbnail.
    /// Saved as `""` when there's none, which older versions of ck still read.
    #[serde(serialize_with = "url_or_empty")]
    pub url: Option<String>,
    /// What the original is, as the site says or its name tells ([`FileKind::of`]).
    pub kind: FileKind,
    /// Thumbnail image, if the site makes one we can show (not for spoilers or generic icons).
    pub thumb: Option<String>,
    pub spoiler: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size: Option<u64>,
    /// The file's MD5, base64-encoded as 4chan gives it, where the site says.
    pub md5: Option<String>,
}

fn url_or_empty<S: serde::Serializer>(url: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(url.as_deref().unwrap_or_default())
}

/// An attachment as saved copies have it: no `kind` from older versions of ck, `""` for no
/// file, and a FoolFuuka file the archive didn't keep stored with its thumbnail as `url`.
#[derive(serde::Deserialize)]
struct StoredAttachment {
    filename: String, url: Option<String>, kind: Option<FileKind>, thumb: Option<String>, spoiler: bool,
    width: Option<u32>, height: Option<u32>, size: Option<u64>, md5: Option<String>,
}

impl From<StoredAttachment> for Attachment {
    fn from(s: StoredAttachment) -> Self {
        // Old copies only: a thumbnail that stood in for its file. LynxChan (its files are
        // under `/.media/`) gives a small image as its own thumbnail; that one is the file.
        // A spoilered file kept no thumbnail of its own, so there it's told by FoolFuuka's
        // thumbnail name, `<tim>s.jpg`.
        let tim_thumb = |u: &str| u.rsplit('/').next().and_then(|n| n.strip_suffix("s.jpg")).is_some_and(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()));
        let stand_in = |u: &String| {
            s.kind.is_none()
                && match &s.thumb {
                    Some(t) => t == u && !u.contains("/.media/"),
                    None => s.spoiler && tim_thumb(u),
                }
        };
        let url = s.url.filter(|u| !u.is_empty() && !stand_in(u));
        let kind = s.kind.unwrap_or_else(|| FileKind::of(None, url.as_deref(), &s.filename));
        let StoredAttachment { filename, thumb, spoiler, width, height, size, md5, .. } = s;
        Self { filename, url, kind, thumb, spoiler, width, height, size, md5 }
    }
}

/// What [`Attachment::link`] calls a thumbnail standing in for its file.
pub const THUMBNAIL_URL: &str = "thumbnail URL";

impl Attachment {
    /// Lowercase extension for labels, read as [`FileKind::of`] reads it: the file URL's,
    /// else the original name's.
    pub fn ext(&self) -> String {
        self.url.as_deref().and_then(url_ext).or_else(|| ext_of(&self.filename)).unwrap_or_default()
    }

    pub fn is_video(&self) -> bool {
        self.kind == FileKind::Video
    }

    /// Whether the original is something we can decode and show in the terminal.
    pub fn is_image(&self) -> bool {
        self.kind == FileKind::Image
    }

    /// The full file, when ck can draw it itself.
    pub fn image(&self) -> Option<&str> {
        self.url.as_deref().filter(|_| self.is_image())
    }

    /// What opening, copying or showing a link acts on, and what to call it: the file,
    /// else its thumbnail.
    pub fn link(&self) -> Option<(&'static str, &str)> {
        self.url.as_deref().map(|u| ("file URL", u)).or_else(|| self.thumb.as_deref().map(|t| (THUMBNAIL_URL, t)))
    }
}

#[cfg(test)]
impl Attachment {
    /// A file at `url`, of the kind its extension says.
    pub fn at(url: impl Into<String>) -> Self {
        let url = url.into();
        Self { kind: FileKind::of(None, Some(&url), ""), url: Some(url), ..Default::default() }
    }
}

/// A poster's flag: their country's, or one the board lets them pick.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Flag {
    /// As the site gives it: `US`, a board flag's `AC`, a custom flag's own name; may be empty.
    pub code: String,
    /// `United States`, `Anarcho-Capitalist`; may be empty.
    pub name: String,
}

impl Flag {
    /// A flag from what the site says, if it says anything.
    pub fn new(code: Option<String>, name: Option<String>) -> Option<Self> {
        let (code, name) = (clean(code).unwrap_or_default(), clean(name).unwrap_or_default());
        (!code.is_empty() || !name.is_empty()).then_some(Self { code, name })
    }

    /// What a post's header shows: a two-letter code as it is (`US`), else the name.
    pub fn short(&self) -> String {
        let two = self.code.len() == 2 && self.code.chars().all(|c| c.is_ascii_alphabetic());
        if two || self.name.is_empty() { self.code.to_ascii_uppercase() } else { self.name.clone() }
    }
}

/// Trimmed, and none when nothing is left.
fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// A capcode spelled 4chan's way whatever the engine sends: FoolFuuka's letters (`M`, and
/// `N` for none) and capitalised words (`Mod`) become `mod`. Other roles stay as sent.
fn role(c: &str) -> Option<String> {
    let word = match c.to_ascii_lowercase().as_str() {
        "" | "n" => return None,
        "m" | "mod" | "moderator" => "mod",
        "a" | "admin" | "administrator" => "admin",
        "d" | "developer" => "developer",
        "v" | "verified" => "verified",
        "f" | "founder" => "founder",
        "g" | "manager" => "manager",
        _ => c,
    };
    Some(word.into())
}

/// Who posted: the name as shown, and its tripcode and capcode apart for filters. Every
/// engine builds it through [`Poster::new`], so each part is spelled one way.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Poster {
    shown: String,
    trip: Option<String>,
    capcode: Option<String>,
}

impl Poster {
    /// The name (else `default`), tripcode and capcode as the site sends them, cleaned and
    /// shown as `name !trip ## capcode`.
    pub fn new(name: Option<String>, default: &str, trip: Option<String>, capcode: Option<String>) -> Self {
        let mut p = Self::stored(clean(name).unwrap_or_else(|| default.into()), trip, capcode);
        let cap = p.capcode.as_ref().map(|c| format!("## {c}"));
        for part in p.trip.iter().chain(&cap) {
            p.shown.push(' ');
            p.shown.push_str(part);
        }
        p
    }

    /// Saved copies only: the name as it was shown then, its parts cleaned and spelled as now.
    pub fn stored(shown: String, trip: Option<String>, capcode: Option<String>) -> Self {
        Self { shown, trip: clean(trip), capcode: clean(capcode).and_then(|c| role(c.trim_start_matches(['#', ' ']))) }
    }

    /// The name as shown, tripcode and capcode included.
    pub fn name(&self) -> &str {
        &self.shown
    }

    pub fn trip(&self) -> Option<&str> {
        self.trip.as_deref()
    }

    /// `mod`, `admin`, `Board Owner`: as the site spells it, without a leading `##`.
    pub fn capcode(&self) -> Option<&str> {
        self.capcode.as_deref()
    }
}

#[cfg(test)]
impl From<&str> for Poster {
    fn from(name: &str) -> Self {
        Self { shown: name.into(), ..Self::default() }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Post {
    pub no: u64,
    pub poster: Poster,
    pub subject: Option<String>,
    /// Unix timestamp (seconds).
    pub time: i64,
    /// Comment already parsed into styled, unwrapped lines.
    pub body: Vec<Line<'static>>,
    /// Post numbers this post quotes (`>>123`).
    pub quotes: Vec<u64>,
    /// All quote links, including ones to other threads and boards.
    pub links: Vec<Link>,
    /// Web links in the comment.
    pub urls: Vec<String>,
    /// Every link in the text, where it is.
    pub anchors: Vec<Anchor>,
    pub files: Vec<Attachment>,
    /// The poster's ID in this thread, on boards that give them (4chan's /pol/, /b/).
    pub id: Option<String>,
    pub flag: Option<Flag>,
    // Catalog-only fields.
    pub replies: Option<u32>,
    pub images: Option<u32>,
    pub sticky: bool,
    pub locked: bool,
    /// The thread has reached its bump limit (4chan says so on the OP).
    pub bumplimit: bool,
    /// The board the thread is on, when the site says. Overboards mix threads from many
    /// boards, so it can differ from the board being browsed.
    pub board: Option<String>,
    /// `plain_text` and `search_text`, computed once.
    pub text: OnceLock<(String, String)>,
}

/// The newest post's number (the highest), or 0 with no posts: what's been seen is
/// counted by it.
pub fn max_no(posts: &[Post]) -> u64 {
    posts.iter().map(|p| p.no).max().unwrap_or(0)
}

/// An answer with fewer than 1/`SHRUNK` of the posts last known of a thread is more likely
/// cut short (or a page the site sent while in trouble) than moderators deleting most of it.
pub const SHRUNK: usize = 2;

/// Whether `got` posts are fewer than half (`SHRUNK`) of the `had` last known.
pub fn shrank(had: usize, got: usize) -> bool {
    got.saturating_mul(SHRUNK) < had
}

/// A thread as a site answered for it: OP first, the OP being the thread asked for.
#[derive(Debug, Clone)]
pub struct Thread {
    no: u64,
    posts: Vec<Post>,
}

impl Thread {
    /// The site's answer for thread `no`: an error if it has no posts, or is another thread.
    pub fn answer(no: u64, posts: Vec<Post>) -> anyhow::Result<Thread> {
        match posts.first().map(|p| p.no) {
            None => anyhow::bail!("The site sent thread {no} without any posts"),
            Some(op) if op != no => anyhow::bail!("The site answered thread {no} with thread {op}"),
            Some(_) => Ok(Thread { no, posts }),
        }
    }

    /// A copy on disk: any with posts, under its OP's number (older copies may be of the
    /// thread a site answered with).
    pub fn saved(copy: crate::saved::SavedThread) -> Option<Thread> {
        let posts: Vec<Post> = copy.posts.into_iter().map(Post::from).collect();
        Some(Thread { no: posts.first()?.no, posts })
    }

    pub fn no(&self) -> u64 {
        self.no
    }

    pub fn posts(&self) -> &[Post] {
        &self.posts
    }

    pub fn into_posts(self) -> Vec<Post> {
        self.posts
    }
}

/// What searching inside a thread looks through, lowercased: the name, subject, file names
/// and text (`plain`, hidden spoilers left out). Searching saved threads uses it too.
pub fn search_haystack<'a>(name: &str, subject: Option<&str>, files: impl IntoIterator<Item = &'a str>, plain: &str) -> String {
    let mut s = format!("{name} {} ", subject.unwrap_or(""));
    for f in files {
        s.push_str(f);
        s.push(' ');
    }
    s.push_str(plain);
    s.to_lowercase()
}

impl Post {
    /// Body flattened to a single line of plain text, for previews and filtering. Spoilers
    /// are left out. Computed once.
    pub fn plain_text(&self) -> &str {
        &self.texts().0
    }

    /// Lowercase number, subject and plain text, for case-insensitive filtering.
    pub fn search_text(&self) -> &str {
        &self.texts().1
    }

    fn texts(&self) -> &(String, String) {
        self.text.get_or_init(|| {
            let mut plain = String::new();
            for line in &self.body {
                if !plain.is_empty() {
                    plain.push(' ');
                }
                for span in &line.spans {
                    if crate::markup::is_spoiler(span.style) {
                        plain.push_str("[spoiler]");
                    } else {
                        plain.push_str(&span.content);
                    }
                }
            }
            let search = format!("{} {} {plain}", self.no, self.subject.as_deref().unwrap_or("")).to_lowercase();
            (plain, search)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::markup::{Flavor, parse_html};

    #[test]
    fn a_poster_is_cleaned_and_joined_once() {
        use super::Poster;
        let p = Poster::new(Some("  Kot ".into()), "Anonymous", Some("!!Fz3m".into()), Some(" ##Board Owner".into()));
        assert_eq!((p.name(), p.trip(), p.capcode()), ("Kot !!Fz3m ## Board Owner", Some("!!Fz3m"), Some("Board Owner")));
        // Blank parts are none, and a blank name is the site's default.
        let p = Poster::new(Some(" ".into()), "Anonymous", Some("  ".into()), Some("##".into()));
        assert_eq!((p.name(), p.trip(), p.capcode()), ("Anonymous", None, None));
        // FoolFuuka's letters, and the words saved copies of its threads kept, are 4chan's.
        assert_eq!(Poster::new(None, "Anonymous", None, Some("M".into())).name(), "Anonymous ## mod");
        assert_eq!(Poster::stored("Anonymous ## Mod".into(), None, Some("Mod".into())).capcode(), Some("mod"));
    }

    #[test]
    fn file_kind_from_mime_then_url_then_name() {
        use super::FileKind::{self, *};
        // The mime wins; an image the terminal can't draw is a plain file.
        assert_eq!(FileKind::of(Some("video/mp4"), Some("https://x/a.jpg"), "a.jpg"), Video);
        assert_eq!(FileKind::of(Some("image/jpeg"), Some("https://x/a"), ""), Image);
        assert_eq!(FileKind::of(Some("image/svg+xml"), None, "a.svg"), Other);
        // An image mime ck has no name for leaves it to the extension.
        assert_eq!(FileKind::of(Some("image/apng"), Some("https://x/a.png"), "a.png"), Image);
        // A mime that says neither (or nothing) leaves it to the file's URL, without its
        // query, then the original name.
        assert_eq!(FileKind::of(Some("application/octet-stream"), Some("https://x/a.webm"), "a"), Video);
        assert_eq!(FileKind::of(Some(""), Some("https://x/a.WEBM?x=1.png"), "a.png"), Video);
        assert_eq!(FileKind::of(None, Some("https://x.y/a"), "clip.webm"), Video);
        assert_eq!(FileKind::of(None, None, "noext"), Other);
        // An original name is only a name: `?` and `#` are part of it.
        assert_eq!(FileKind::of(None, None, "what?.webm"), Video);
        assert_eq!(FileKind::of(None, None, "a#b.png"), Image);
        assert_eq!(FileKind::of(None, None, "build v1.2 final"), Other);
    }

    #[test]
    fn plain_text_hides_spoilers() {
        let parsed = parse_html(r#"With a <span class="spoiler">SaaS</span> of $5"#, Flavor::Vichan);
        let p = super::Post { body: parsed.lines, ..Default::default() };
        assert_eq!(p.plain_text(), "With a [spoiler] of $5");
        assert_eq!(p.search_text(), "0  with a [spoiler] of $5");
    }

    fn posts(ns: &[u64]) -> Vec<super::Post> {
        ns.iter().map(|&no| super::Post { no, ..Default::default() }).collect()
    }

    #[test]
    fn an_answer_is_the_thread_asked_for() {
        use super::Thread;
        assert!(Thread::answer(5, Vec::new()).is_err());
        let e = Thread::answer(5, posts(&[7, 8])).unwrap_err();
        assert_eq!(e.to_string(), "The site answered thread 5 with thread 7");
        let t = Thread::answer(5, posts(&[5, 6])).unwrap();
        assert_eq!((t.no(), t.posts().len()), (5, 2));
    }

    #[test]
    fn fewer_than_half_the_posts_known_is_cut_short() {
        // Exactly half isn't; one fewer is.
        assert!(!super::shrank(10, 5));
        assert!(super::shrank(10, 4));
        assert!(!super::shrank(0, 1));
        assert!(super::shrank(3, 1));
    }
}
