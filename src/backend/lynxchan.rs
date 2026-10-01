//! LynxChan engine JSON API (endchan, kohlchan, ...).

use anyhow::Result;
use serde_json::Value;

use super::Backend;
use crate::http::{as_bool, as_str, as_u64, encode_segment as enc, get_json};
use crate::markup;
use crate::model::{Attachment, Board, Post};

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

    fn get(&self, path: &str) -> Result<Value> {
        Ok(unwrap(get_json(&format!("{}{path}", self.base))?))
    }

    /// Thread OPs from `/{board}/catalog.json`.
    pub fn parse_catalog(&self, v: &Value) -> Vec<Post> {
        v.as_array().into_iter().flatten().map(|t| self.post(t, "threadId")).collect()
    }

    /// A thread from `/{board}/res/{no}.json`: the OP's fields plus `posts`.
    pub fn parse_thread(&self, v: &Value) -> Vec<Post> {
        let mut posts = vec![self.post(v, "threadId")];
        posts.extend(v["posts"].as_array().into_iter().flatten().map(|p| self.post(p, "postId")));
        posts
    }

    fn post(&self, v: &Value, no_key: &str) -> Post {
        // `markdown` is the rendered HTML, with link targets; `message` is the raw text.
        let parsed = match v["markdown"].as_str().filter(|m| !m.is_empty()) {
            Some(md) => markup::parse_html(md, markup::Flavor::Lynxchan),
            None => markup::parse_plain(v["message"].as_str().unwrap_or("")),
        };
        let mut name = as_str(&v["name"]).unwrap_or_else(|| "Anonymous".into());
        if let Some(role) = as_str(&v["signedRole"]) {
            name.push_str(&format!(" ## {role}"));
        }
        let mut files: Vec<Attachment> = v["files"]
            .as_array()
            .map(|fs| {
                fs.iter()
                    .filter_map(|f| {
                        let path = as_str(&f["path"])?;
                        let (thumb, spoiler) = thumb(&f["thumb"]);
                        Some(Attachment {
                            filename: as_str(&f["originalName"])
                                .unwrap_or_else(|| path.rsplit('/').next().unwrap_or("").into()),
                            url: format!("{}{path}", self.base),
                            thumb: thumb.map(|t| format!("{}{t}", self.base)),
                            spoiler,
                            width: as_u64(&f["width"]).map(|n| n as u32),
                            height: as_u64(&f["height"]).map(|n| n as u32),
                            size: as_u64(&f["size"]),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Some catalogs (endchan) only give the OP's thumbnail, not its files.
        if files.is_empty()
            && let Some(path) = as_str(&v["thumb"])
        {
            let (thumb, spoiler) = thumb(&v["thumb"]);
            files.push(Attachment {
                filename: "catalog thumbnail (open the thread for the file)".into(),
                url: format!("{}{path}", self.base),
                thumb: thumb.map(|t| format!("{}{t}", self.base)),
                spoiler,
                width: None,
                height: None,
                size: None,
            });
        }
        Post {
            no: as_u64(&v[no_key]).unwrap_or(0),
            name,
            subject: as_str(&v["subject"]),
            time: parse_time(&v["creation"]).or_else(|| parse_time(&v["lastBump"])).unwrap_or(0),
            body: parsed.lines,
            quotes: parsed.quotes,
            links: parsed.links,
            files,
            // postCount excludes the OP, like 4chan's `replies`.
            replies: as_u64(&v["postCount"]).map(|n| n as u32),
            images: as_u64(&v["fileCount"]).map(|n| n as u32),
            sticky: as_bool(&v["pinned"]),
            locked: as_bool(&v["locked"]),
            ..Default::default()
        }
    }
}

/// Some responses (kohlchan's board list) are wrapped as `{"status": "ok", "data": ...}`.
pub fn unwrap(mut v: Value) -> Value {
    if v.get("status").is_some() && v.get("data").is_some() {
        v = v["data"].take();
    }
    v
}

/// One page of `/boards.js?json=1` (already unwrapped): boards and the page count.
pub fn parse_boards(v: &Value) -> (Vec<Board>, u64) {
    let boards = v["boards"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| {
            let uri = as_str(&b["boardUri"])?;
            let nsfw = b["specialSettings"].as_array().map(|s| !s.iter().any(|x| x.as_str() == Some("sfw")));
            Some(Board { uri, title: as_str(&b["boardName"]).unwrap_or_default(), nsfw })
        })
        .collect();
    (boards, as_u64(&v["pageCount"]).unwrap_or(1))
}

/// A file's thumbnail path and whether it's a spoiler. Spoilers and non-images point at
/// shared placeholder images, which aren't worth showing.
fn thumb(v: &Value) -> (Option<String>, bool) {
    let thumb = as_str(v);
    let spoiler = thumb.as_deref() == Some("/spoiler.png");
    (thumb.filter(|t| !spoiler && t != "/genericThumb.png"), spoiler)
}

fn parse_time(v: &Value) -> Option<i64> {
    let s = v.as_str()?;
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| t.timestamp())
}

impl Backend for Lynxchan {
    fn boards(&self) -> Result<Vec<Board>> {
        if let Some(b) = &self.boards {
            return Ok(b.clone());
        }
        let mut out = Vec::new();
        let mut page = 1;
        loop {
            let (boards, pages) = parse_boards(&self.get(&format!("/boards.js?json=1&page={page}"))?);
            out.extend(boards);
            if page >= pages || page >= MAX_BOARD_PAGES {
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    fn catalog(&self, board: &str) -> Result<Vec<Post>> {
        Ok(self.parse_catalog(&self.get(&format!("/{}/catalog.json", enc(board)))?))
    }

    fn thread(&self, board: &str, no: u64) -> Result<Vec<Post>> {
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
    use serde_json::Value;

    use super::Lynxchan;

    fn json(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn fixture(name: &str) -> Vec<Value> {
        serde_json::from_value(json(name)).unwrap()
    }

    #[test]
    fn boards_plain_and_wrapped() {
        // endchan: plain JSON.
        let (boards, pages) = super::parse_boards(&super::unwrap(json("lynxchan_boards.json")));
        assert_eq!(boards.len(), 4);
        assert_eq!(boards[0].uri, "polru");
        assert!(pages > 1);
        // kohlchan: {"status": "ok", "data": {...}}.
        let (boards, pages) = super::parse_boards(&super::unwrap(json("lynxchan_boards_wrapped.json")));
        assert_eq!((boards[0].uri.as_str(), boards[0].title.as_str()), ("int", "International"));
        assert_eq!(pages, 1);
    }

    #[test]
    fn catalog_and_thread() {
        let end = Lynxchan::new("https://endchan.net".into(), None);
        let cat = end.parse_catalog(&json("lynxchan_catalog.json"));
        assert_eq!(cat[0].no, 908495);
        assert!(cat[0].sticky);
        let posts = end.parse_thread(&json("lynxchan_thread.json"));
        assert_eq!(posts[0].no, 867082);
        // The OP links the previous thread through its rendered markdown.
        assert!(posts[0].links.iter().any(|l| l.thread == Some(782482)));
        let kohl = Lynxchan::new("https://kohlchan.net".into(), None);
        let posts = kohl.parse_thread(&json("kohlchan_thread.json"));
        assert!(posts.len() > 1 && posts.iter().all(|p| p.no > 0));
    }

    fn find(v: &[Value], no: u64) -> &Value {
        v.iter().find(|t| t["threadId"].as_u64() == Some(no)).unwrap()
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
        assert!(p.files[0].url.ends_with(".jpg") || p.files[0].url.contains("/.media/"));
        let p = kohl.post(find(&cat, 28874644), "threadId");
        assert!(p.files[0].spoiler && p.files[0].thumb.is_none());
    }
}
