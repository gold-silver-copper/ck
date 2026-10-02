//! Makaba, 2ch.hk's engine: `/{board}/catalog.json`, `/{board}/res/{no}.json`, and the
//! mobile API for the board list and post lookups.

use anyhow::Result;
use serde_json::Value;

use super::{Backend, Partial};
use crate::http::{as_bool, as_i64, as_str, as_u64, encode_segment as enc, get_json, register_media_host};
use crate::markup::{self, Flavor};
use crate::model::{Attachment, Board, Post};

/// The board category for adult boards.
const ADULT_CATEGORY: &str = "Взрослым";

pub struct Makaba {
    base: String,
    /// Where files are served from (2ch.hk redirects them to another host).
    media: String,
    boards: Option<Vec<Board>>,
}

impl Makaba {
    pub fn new(base: String, media: Option<String>, boards: Option<Vec<Board>>) -> Self {
        let media = media.map(|m| m.trim_end_matches('/').to_string()).unwrap_or_else(|| base.clone());
        if media != base {
            register_media_host(&media);
        }
        Self { base, media, boards }
    }

    /// Thread OPs from `catalog.json`.
    pub fn parse_catalog(&self, v: &Value) -> Vec<Post> {
        v["threads"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| {
                let mut p = self.post(t);
                // posts_count includes the OP.
                p.replies = as_u64(&t["posts_count"]).map(|n| n.saturating_sub(1) as u32);
                p.images = as_u64(&t["files_count"]).map(|n| n as u32);
                p
            })
            .collect()
    }

    /// Posts from `res/{no}.json`: `{ "threads": [{ "posts": [...] }] }`, OP first.
    pub fn parse_thread(&self, v: &Value) -> Vec<Post> {
        v["threads"][0]["posts"].as_array().into_iter().flatten().map(|p| self.post(p)).collect()
    }

    fn post(&self, v: &Value) -> Post {
        let parsed = markup::parse_html(v["comment"].as_str().unwrap_or(""), Flavor::Makaba);
        let mut name = as_str(&v["name"]).map(|n| strip_tags(&n)).unwrap_or_else(|| "Аноним".into());
        if let Some(trip) = as_str(&v["trip"]) {
            name.push(' ');
            name.push_str(&strip_tags(&trip));
        }
        Post {
            no: as_u64(&v["num"]).unwrap_or(0),
            name,
            subject: as_str(&v["subject"]).map(|s| strip_tags(&s)).filter(|s| !s.is_empty()),
            time: as_i64(&v["timestamp"]).unwrap_or(0),
            body: parsed.lines,
            quotes: parsed.quotes,
            links: parsed.links,
            urls: parsed.urls,
            files: v["files"].as_array().into_iter().flatten().filter_map(|f| self.attachment(f)).collect(),
            sticky: as_bool(&v["sticky"]),
            board: as_str(&v["board"]),
            locked: as_bool(&v["closed"]),
            ..Default::default()
        }
    }

    /// Files give paths relative to the site; sizes are in KB.
    fn attachment(&self, f: &Value) -> Option<Attachment> {
        let path = as_str(&f["path"])?;
        Some(Attachment {
            filename: as_str(&f["fullname"]).or_else(|| as_str(&f["name"])).map(|n| markup::decode(&n)).unwrap_or_default(),
            url: format!("{}{path}", self.media),
            thumb: as_str(&f["thumbnail"]).map(|t| format!("{}{t}", self.media)),
            spoiler: false,
            width: as_u64(&f["width"]).map(|n| n as u32),
            height: as_u64(&f["height"]).map(|n| n as u32),
            size: as_u64(&f["size"]).map(|kb| kb * 1024),
        })
    }
}

/// Boards from `/api/mobile/v2/boards`.
pub fn parse_boards(v: &Value) -> Vec<Board> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| {
            Some(Board {
                uri: as_str(&b["id"])?,
                title: as_str(&b["name"]).unwrap_or_default(),
                nsfw: Some(b["category"].as_str() == Some(ADULT_CATEGORY)),
            })
        })
        .collect()
}

/// Names and subjects can carry markup (e.g. coloured trips).
fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    markup::decode(out.trim())
}

impl Backend for Makaba {
    fn boards(&self, _partial: Partial<Board>) -> Result<Vec<Board>> {
        if let Some(b) = &self.boards {
            return Ok(b.clone());
        }
        Ok(parse_boards(&get_json(&format!("{}/api/mobile/v2/boards", self.base))?))
    }

    fn catalog(&self, board: &str, _partial: Partial<Post>) -> Result<Vec<Post>> {
        Ok(self.parse_catalog(&get_json(&format!("{}/{}/catalog.json", self.base, enc(board)))?))
    }

    fn thread(&self, board: &str, no: u64) -> Result<Vec<Post>> {
        Ok(self.parse_thread(&get_json(&format!("{}/{}/res/{no}.json", self.base, enc(board)))?))
    }

    /// `/api/mobile/v2/post/{board}/{no}` gives the post with its thread as `parent` (0 for OPs).
    fn find_thread(&self, board: &str, post: u64) -> Result<Option<u64>> {
        let v = get_json(&format!("{}/api/mobile/v2/post/{}/{post}", self.base, enc(board)))?;
        Ok(as_u64(&v["post"]["parent"]).map(|p| if p == 0 { post } else { p }))
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

    use super::Makaba;
    use crate::markup::{is_quote_link, is_spoiler};

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn dvach() -> Makaba {
        Makaba::new("https://2ch.hk".into(), Some("https://2ch.su".into()), None)
    }

    #[test]
    fn boards() {
        let boards = super::parse_boards(&fixture("makaba_boards.json"));
        let b = boards.iter().find(|b| b.uri == "b").unwrap();
        assert_eq!((b.title.as_str(), b.nsfw), ("Бред", Some(false)));
        assert_eq!(boards.iter().find(|b| b.uri == "h").unwrap().nsfw, Some(true));
    }

    #[test]
    fn catalog() {
        let cat = dvach().parse_catalog(&fixture("makaba_catalog.json"));
        assert_eq!(cat.len(), 6);
        assert!(cat[0].sticky && cat[0].locked);
        assert_eq!(cat[0].replies, Some(0));
        let f = &cat[0].files[0];
        assert_eq!(f.url, "https://2ch.su/b/src/328868282/17686951847150.jpg");
        assert_eq!(f.thumb.as_deref(), Some("https://2ch.su/b/thumb/328868282/17686951847150s.jpg"));
        assert_eq!(f.size, Some(31 * 1024));
    }

    #[test]
    fn thread_markup() {
        let posts = dvach().parse_thread(&fixture("makaba_thread.json"));
        let op = posts[0].no;
        assert_eq!(op, 337030181);
        // Replies quote the OP; 2ch's own " (OP)" is dropped (ck adds its own).
        let reply = posts.iter().find(|p| p.quotes.contains(&op)).unwrap();
        let quote = reply.body.iter().flat_map(|l| &l.spans).find(|s| is_quote_link(s.style)).unwrap();
        assert_eq!(quote.content, format!(">>{op}"));
        assert!(reply.links.iter().any(|l| l.thread == Some(op) && l.board.as_deref() == Some("b")));
        let all: Vec<_> = posts.iter().flat_map(|p| &p.body).flat_map(|l| &l.spans).collect();
        assert!(all.iter().any(|s| s.content.starts_with('>') && s.style == crate::markup::greentext()));
        assert!(all.iter().any(|s| is_spoiler(s.style)));
        assert!(!all.iter().any(|s| s.content.contains("(OP)")));
    }

    #[test]
    fn post_lookup_gives_the_thread() {
        let v = fixture("makaba_post.json");
        assert_eq!(crate::http::as_u64(&v["post"]["parent"]), Some(337030181));
    }
}
