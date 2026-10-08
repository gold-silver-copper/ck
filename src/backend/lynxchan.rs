//! LynxChan engine JSON API (endchan, kohlchan, ...).
#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]

use anyhow::Result;
use serde_json::Value;

use super::{Backend, Partial, as_u32, saturate, site_error};
use crate::http::{as_bool, as_str, as_u64, encode_segment as enc, get_json, items, is_not_found};
use crate::markup;
use crate::model::{Attachment, Board, FileKind, Flag, Post, Poster};

/// Board list is paginated; don't hammer huge sites at startup.
const MAX_BOARD_PAGES: u64 = 5;

pub struct Lynxchan {
    base: String,
    boards: Option<Vec<Board>>,
}

impl Lynxchan {
    pub fn new(base: String, boards: Option<Vec<Board>>) -> Self {
        Self { base, boards }
    }

    /// A wrapped answer is unwrapped; one whose status isn't "ok" is an error with its text.
    fn get(&self, path: &str) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        let v = get_json(&url)?;
        let text = || v.get("data").map(|d| as_str(d).unwrap_or_else(|| d.to_string())).unwrap_or_default();
        match v.get("status").and_then(Value::as_str) {
            None | Some("ok") => Ok(unwrap(v)),
            Some("error") => Err(site_error(&url, &text())),
            Some(status) => anyhow::bail!("{status}: {}", text()),
        }
    }

    /// Thread OPs from `/{board}/catalog.json`.
    pub fn parse_catalog(&self, v: &Value) -> Vec<Post> {
        items(v).map(|t| self.post(t, "threadId")).collect()
    }

    /// Page 1 of a board's index (`/{board}/1.json`), which is how overboards are served:
    /// threads from many boards, each with its `boardUri` and its last few replies.
    pub fn parse_index(&self, v: &Value) -> Vec<Post> {
        items(&v["threads"])
            .map(|t| {
                let mut p = self.post(t, "threadId");
                let shown = t["posts"].as_array().map_or(0, |a| a.len()) as u64;
                let shown_files: u64 = items(&t["posts"]).map(|r| r["files"].as_array().map_or(0, |f| f.len()) as u64).sum();
                p.replies = Some(saturate(as_u64(&t["omittedPosts"]).unwrap_or(0).saturating_add(shown)));
                p.images = Some(saturate(as_u64(&t["omittedFiles"]).unwrap_or(0).saturating_add(shown_files)));
                p
            })
            .collect()
    }

    /// A thread from `/{board}/res/{no}.json`: the OP's fields plus `posts`.
    pub fn parse_thread(&self, v: &Value) -> Vec<Post> {
        let mut posts = vec![self.post(v, "threadId")];
        posts.extend(items(&v["posts"]).map(|p| self.post(p, "postId")));
        posts
    }

    fn post(&self, v: &Value, no_key: &str) -> Post {
        // `markdown` is the rendered HTML, with link targets; `message` is the raw text.
        let parsed = match v["markdown"].as_str().filter(|m| !m.is_empty()) {
            Some(md) => markup::parse_html(md, markup::Flavor::Lynxchan),
            None => markup::parse_plain(v["message"].as_str().unwrap_or("")),
        };
        let (name, trip) = name_and_trip(as_str(&v["name"]));
        // `flagCode` is `-us` for a country; a board's custom flag has only its name.
        let code = as_str(&v["flagCode"]).map(|c| c.trim_start_matches('-').to_string());
        let flag = Flag::new(code, as_str(&v["flagName"]));
        let mut files: Vec<Attachment> = items(&v["files"])
            .filter_map(|f| {
                let path = as_str(&f["path"])?;
                let (thumb, spoiler) = thumb(&f["thumb"]);
                let filename = as_str(&f["originalName"]).unwrap_or_else(|| path.rsplit('/').next().unwrap_or("").into());
                let url = format!("{}{path}", self.base);
                Some(Attachment {
                    kind: FileKind::of(as_str(&f["mime"]).as_deref(), Some(&url), &filename),
                    filename,
                    url: Some(url),
                    thumb: thumb.map(|t| format!("{}{t}", self.base)),
                    spoiler,
                    width: as_u32(&f["width"]),
                    height: as_u32(&f["height"]),
                    size: as_u64(&f["size"]),
                    md5: None,
                })
            })
            .collect();
        // Some catalogs (endchan) only give the OP's thumbnail, not its files.
        if files.is_empty() && as_str(&v["thumb"]).is_some() {
            let (thumb, spoiler) = thumb(&v["thumb"]);
            let thumb = thumb.map(|t| format!("{}{t}", self.base));
            files.push(Attachment { filename: String::new(), url: None, kind: FileKind::Other, thumb, spoiler, width: None, height: None, size: None, md5: None });
        }
        Post {
            no: as_u64(&v[no_key]).unwrap_or(0),
            poster: Poster::new(name, "Anonymous", trip, as_str(&v["signedRole"])),
            id: as_str(&v["id"]),
            flag,
            subject: as_str(&v["subject"]),
            time: parse_time(&v["creation"]).or_else(|| parse_time(&v["lastBump"])).unwrap_or(0),
            files,
            // postCount excludes the OP, like 4chan's `replies`.
            replies: as_u32(&v["postCount"]),
            images: as_u32(&v["fileCount"]),
            sticky: as_bool(&v["pinned"]),
            board: as_str(&v["boardUri"]),
            locked: as_bool(&v["locked"]),
            ..parsed.into()
        }
    }
}

/// Some responses (kohlchan's board list) are wrapped as `{"status": "ok", "data": ...}`.
pub fn unwrap(mut v: Value) -> Value {
    if v.get("status").is_some()
        && let Some(data) = v.get_mut("data")
    {
        v = data.take();
    }
    v
}

/// One page of `/boards.js?json=1` (already unwrapped): boards and the page count.
pub fn parse_boards(v: &Value) -> (Vec<Board>, u64) {
    let boards = items(&v["boards"])
        .filter_map(|b| {
            let uri = as_str(&b["boardUri"])?;
            let nsfw = b["specialSettings"].as_array().map(|s| !s.iter().any(|x| x.as_str() == Some("sfw")));
            Some(Board { uri, title: as_str(&b["boardName"]).unwrap_or_default(), nsfw })
        })
        .collect();
    (boards, as_u64(&v["pageCount"]).unwrap_or(1))
}

/// The overboards a board list names (`overboard`, `sfwOverboard`), to list as boards.
pub fn parse_overboards(v: &Value) -> Vec<Board> {
    [("overboard", "Overboard (all boards)"), ("sfwOverboard", "SFW overboard")]
        .iter()
        .filter_map(|(key, title)| Some(Board { uri: as_str(&v[*key])?, title: title.to_string(), nsfw: None }))
        .collect()
}

/// A file's thumbnail path and whether it's a spoiler. Spoilers and non-images point at
/// shared placeholder images, which aren't worth showing.
fn thumb(v: &Value) -> (Option<String>, bool) {
    let thumb = as_str(v);
    let spoiler = thumb.as_deref() == Some("/spoiler.png");
    (thumb.filter(|t| !spoiler && t != "/genericThumb.png"), spoiler)
}

/// LynxChan sends the tripcode inside the name (`Bernd!!Fz3mQwerty`): it starts at the
/// last run of `!` followed by nothing but a trip's characters, so a `!` earlier in the
/// name stays in the name.
fn name_and_trip(raw: Option<String>) -> (Option<String>, Option<String>) {
    let Some(r) = raw.as_deref() else { return (None, None) };
    let hash = r.trim_end_matches(|c: char| c.is_ascii_alphanumeric() || "./+".contains(c));
    let name = hash.trim_end_matches('!');
    match r.strip_prefix(name) {
        Some(trip) if hash.len() < r.len() && name.len() < hash.len() => (Some(name.into()), Some(trip.into())),
        _ => (raw, None),
    }
}

fn parse_time(v: &Value) -> Option<i64> {
    let s = v.as_str()?;
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| t.timestamp())
}

impl Backend for Lynxchan {
    fn boards(&self, partial: Partial<Board>) -> Result<Vec<Board>> {
        if let Some(b) = &self.boards {
            return Ok(b.clone());
        }
        let mut out = Vec::new();
        let mut page = 1;
        loop {
            let v = self.get(&format!("/boards.js?json=1&page={page}"))?;
            if page == 1 {
                out.extend(parse_overboards(&v));
            }
            let (boards, pages) = parse_boards(&v);
            out.extend(boards);
            if page >= pages || page >= MAX_BOARD_PAGES {
                break;
            }
            partial(&out);
            page = page.saturating_add(1);
        }
        Ok(out)
    }

    /// Overboards have no catalog.json; their first index page stands in for one.
    fn catalog(&self, board: &str, _partial: Partial<Post>) -> Result<Vec<Post>> {
        match self.get(&format!("/{}/catalog.json", enc(board))) {
            Err(e) if is_not_found(&e) => Ok(self.parse_index(&self.get(&format!("/{}/1.json", enc(board)))?)),
            res => Ok(self.parse_catalog(&res?)),
        }
    }

    fn thread_unchecked(&self, board: &str, no: u64) -> Result<Vec<Post>> {
        Ok(self.parse_thread(&self.get(&format!("/{}/res/{no}.json", enc(board)))?))
    }

    fn board_url(&self, board: &str) -> String {
        format!("{}/{}/", self.base, enc(board))
    }

    fn thread_url(&self, board: &str, no: u64) -> String {
        format!("{}/{}/res/{no}.html", self.base, enc(board))
    }
}

#[cfg(test)]
mod tests {
    use crate::backend::fixture;
    use serde_json::Value;

    use super::Lynxchan;

    #[test]
    fn boards_plain_and_wrapped() {
        // endchan: plain JSON.
        let (boards, pages) = super::parse_boards(&super::unwrap(fixture("lynxchan_boards.json")));
        assert_eq!(boards.len(), 4);
        assert_eq!(boards[0].uri, "polru");
        assert!(pages > 1);
        // kohlchan: {"status": "ok", "data": {...}}.
        let (boards, pages) = super::parse_boards(&super::unwrap(fixture("lynxchan_boards_wrapped.json")));
        assert_eq!((boards[0].uri.as_str(), boards[0].title.as_str()), ("int", "International"));
        assert_eq!(pages, 1);
    }

    #[test]
    fn overboards() {
        // Named in the board list...
        let names = |f| super::parse_overboards(&super::unwrap(fixture(f))).into_iter().map(|b| b.uri).collect::<Vec<_>>();
        assert_eq!(names("lynxchan_boards.json"), ["overboard", "overboard_sfw"]);
        assert_eq!(names("lynxchan_boards_wrapped.json"), ["alle", "nvip"]);
        // ...and served as index pages, threads from many boards.
        let end = Lynxchan::new("https://endchan.net".into(), None);
        let posts = end.parse_index(&fixture("lynxchan_overboard.json"));
        let boards: Vec<_> = posts.iter().map(|p| p.board.as_deref().unwrap()).collect();
        assert_eq!(boards, ["terrachan", "derman", "polru", "dota"]);
        let kohl = Lynxchan::new("https://kohlchan.net".into(), None);
        let posts = kohl.parse_index(&fixture("kohlchan_overboard.json"));
        assert_eq!(posts[0].board.as_deref(), Some("int"));
        assert_eq!(posts[0].replies, Some(47 + 2));
    }

    #[test]
    fn catalog_and_thread() {
        let end = Lynxchan::new("https://endchan.net".into(), None);
        let cat = end.parse_catalog(&fixture("lynxchan_catalog.json"));
        assert_eq!(cat[0].no, 908495);
        assert!(cat[0].sticky);
        let posts = end.parse_thread(&fixture("lynxchan_thread.json"));
        assert_eq!(posts[0].no, 867082);
        // The OP links the previous thread through its rendered markdown.
        assert!(posts[0].links.iter().any(|l| l.thread == Some(782482)));
        // Web links are collected as the site linked them (LynxChan keeps a trailing paren).
        let urls: Vec<&str> = posts.iter().flat_map(|p| &p.urls).map(String::as_str).collect();
        assert!(urls.contains(&"https://Paha-Ne-Vydast.me/astrapress/89410)"), "{urls:?}");
        let kohl = Lynxchan::new("https://kohlchan.net".into(), None);
        let posts = kohl.parse_thread(&fixture("kohlchan_thread.json"));
        assert!(posts.len() > 1 && posts.iter().all(|p| p.no > 0));
    }

    #[test]
    fn ids_and_flags() {
        use crate::model::Flag;
        let posts = Lynxchan::new("https://endchan.net".into(), None).parse_thread(&fixture("lynxchan_thread.json"));
        assert_eq!(posts[0].id.as_deref(), Some("443169"));
        // A board's own flag has a name and no code.
        assert_eq!(posts[0].flag, Some(Flag { code: String::new(), name: "Tatarstan".into() }));
        assert_eq!(posts[0].flag.as_ref().unwrap().short(), "Tatarstan");
        let p = posts.iter().find(|p| p.no == 867094).unwrap();
        assert_eq!((p.id.as_deref(), p.flag.as_ref().map(Flag::short).as_deref()), (Some("dc6304"), Some("RO")));
        assert!(posts.iter().find(|p| p.no == 867169).is_some_and(|p| p.id.is_none() && p.flag.is_none()));
        // kohlchan: flags without IDs; `-br` is Brazil's.
        let posts = Lynxchan::new("https://kohlchan.net".into(), None).parse_thread(&fixture("kohlchan_thread.json"));
        assert!(posts.iter().all(|p| p.id.is_none()));
        let p = posts.iter().find(|p| p.no == 28883633).unwrap();
        assert_eq!(p.flag, Some(Flag { code: "br".into(), name: "Brasil".into() }));
    }

    fn find(v: &Value, no: u64) -> &Value {
        v.as_array().unwrap().iter().find(|t| t["threadId"].as_u64() == Some(no)).unwrap()
    }

    #[test]
    fn catalog_thumbnails() {
        // endchan's catalog has only a thumbnail per thread.
        let end = Lynxchan::new("https://endchan.net".into(), None);
        let cat = fixture("lynxchan_catalog.json");
        let p = end.post(find(&cat, 867082), "threadId");
        let thumb = "https://endchan.net/.media/t_53781bea8476800b093c909d2f0902d2-imagejpeg";
        assert_eq!(p.files[0].thumb.as_deref(), Some(thumb));
        let p = end.post(find(&cat, 128014), "threadId");
        assert!(p.files[0].spoiler && p.files[0].thumb.is_none());
        let p = end.post(find(&cat, 837516), "threadId");
        assert!(!p.files[0].spoiler && p.files[0].thumb.is_none());

        // kohlchan's lists the files.
        let kohl = Lynxchan::new("https://kohlchan.net".into(), None);
        let cat = fixture("kohlchan_catalog.json");
        let p = kohl.post(find(&cat, 28870642), "threadId");
        assert_eq!(p.files.len(), 4);
        assert!(p.files[0].url.as_ref().is_some_and(|u| u.ends_with(".jpg") || u.contains("/.media/")));
        let p = kohl.post(find(&cat, 28874644), "threadId");
        assert!(p.files[0].spoiler && p.files[0].thumb.is_none());
    }

    #[test]
    fn catalog_thumbnail_is_not_a_file() {
        // endchan's catalog gives only a thumbnail: no download saves it as the thread's
        // file, and a filename filter doesn't match the placeholder name ck gives it.
        let end = Lynxchan::new("https://endchan.net".into(), None);
        let p = end.post(find(&fixture("lynxchan_catalog.json"), 867082), "threadId");
        let thumb = "https://endchan.net/.media/t_53781bea8476800b093c909d2f0902d2-imagejpeg";
        let jobs = crate::download::jobs(&[&p], std::path::Path::new("/d"));
        assert!(jobs.iter().all(|(url, _)| url != thumb), "the catalog thumbnail is downloaded as a file: {jobs:?}");
        let f = crate::filter::tests::filters("[[filter]]\npattern = \"(?i)thumbnail\"\nfield = \"filename\"\n").unwrap();
        assert!(f.check("endchan", "b", &p, true).hidden.is_none(), "a filename filter matched ck's placeholder text");
    }

    #[test]
    fn a_tripcode_in_the_name_is_the_posts_tripcode() {
        // LynxChan has no tripcode field: it appends the trip to `name` ("Bernd!!Fz3mQwerty").
        let end = Lynxchan::new("https://endchan.net".into(), None);
        let p = end.post(&serde_json::json!({"threadId": 1, "name": "Bernd!!Fz3mQwerty"}), "threadId");
        assert_eq!(p.poster.trip(), Some("!!Fz3mQwerty"), "name {:?}", p.poster.name());
        let f = crate::filter::tests::filters("[[filter]]\npattern = \"^!!Fz3m\"\nfield = \"tripcode\"\n").unwrap();
        assert!(f.check("endchan", "b", &p, false).hidden.is_some(), "a tripcode filter misses the post");
        let p = end.post(&serde_json::json!({"threadId": 1, "name": "Hi!there!!Fz3mQwerty"}), "threadId");
        assert_eq!((p.poster.name(), p.poster.trip()), ("Hi!there !!Fz3mQwerty", Some("!!Fz3mQwerty")));
    }

    #[test]
    fn file_kind_comes_from_the_mime() {
        // LynxChan says what each file is; a path without an extension is still a video.
        let end = Lynxchan::new("https://endchan.net".into(), None);
        let v = serde_json::json!({"threadId": 1, "files": [
            {"path": "/.media/7cb1d1b2e5fffe9cfe1adbfe0a72ff3c", "mime": "video/mp4", "originalName": "clip", "thumb": "/.media/t_7cb1d1b2e5fffe9cfe1adbfe0a72ff3c"},
            {"path": "/.media/9cbd28a1c035034c6379843b6ee0ddbf", "mime": "image/jpeg", "originalName": "pic", "thumb": "/.media/t_9cbd28a1c035034c6379843b6ee0ddbf"},
        ]});
        let p = end.post(&v, "threadId");
        assert!(p.files[0].is_video(), "mime video/mp4 ignored");
        assert!(p.files[1].is_image(), "mime image/jpeg ignored");
    }
}
