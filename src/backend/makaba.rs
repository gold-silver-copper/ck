//! Makaba, 2ch.hk's engine: `/{board}/catalog.json`, `/{board}/res/{no}.json`, and the
//! mobile API for the board list and post lookups.
#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]

use anyhow::Result;
use serde_json::Value;

use super::{Backend, Partial, as_u32, saturate};
use crate::http::{as_bool, as_i64, as_str, as_u64, encode_segment as enc, get_json, items, register_media_host};
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
        items(&v["threads"])
            .map(|t| {
                let mut p = self.post(t);
                // posts_count includes the OP.
                p.replies = as_u64(&t["posts_count"]).map(|n| saturate(n.saturating_sub(1)));
                p.images = as_u32(&t["files_count"]);
                p
            })
            .collect()
    }

    /// Posts from `res/{no}.json`: `{ "threads": [{ "posts": [...] }] }`, OP first.
    pub fn parse_thread(&self, v: &Value) -> Vec<Post> {
        v.get("threads").and_then(|t| t.get(0)).into_iter().flat_map(|t| items(&t["posts"])).map(|p| self.post(p)).collect()
    }

    fn post(&self, v: &Value) -> Post {
        let parsed = markup::parse_html(v["comment"].as_str().unwrap_or(""), Flavor::Makaba);
        let mut name = as_str(&v["name"]).map(|n| strip_tags(&n)).unwrap_or_else(|| "Аноним".into());
        let trip = as_str(&v["trip"]).map(|t| strip_tags(&t)).filter(|t| !t.is_empty());
        if let Some(trip) = &trip {
            name.push(' ');
            name.push_str(trip);
        }
        Post {
            no: as_u64(&v["num"]).unwrap_or(0),
            name,
            trip,
            subject: as_str(&v["subject"]).map(|s| strip_tags(&s)).filter(|s| !s.is_empty()),
            time: as_i64(&v["timestamp"]).unwrap_or(0),
            files: items(&v["files"]).filter_map(|f| self.attachment(f)).collect(),
            sticky: as_bool(&v["sticky"]),
            board: as_str(&v["board"]),
            locked: as_bool(&v["closed"]),
            ..parsed.into()
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
            width: as_u32(&f["width"]),
            height: as_u32(&f["height"]),
            size: as_u64(&f["size"]).map(|kb| kb.saturating_mul(1024)),
            md5: as_str(&f["md5"]).and_then(|h| hex_to_base64(&h)),
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
        Ok(v.get("post").and_then(|p| as_u64(&p["parent"])).map(|p| if p == 0 { post } else { p }))
    }

    fn board_url(&self, board: &str) -> String {
        format!("{}/{}/", self.base, enc(board))
    }

    fn thread_url(&self, board: &str, no: u64) -> String {
        format!("{}/{}/res/{no}.html", self.base, enc(board))
    }
}

/// makaba gives MD5s in hex; filters match them in base64, like every other engine's.
fn hex_to_base64(hex: &str) -> Option<String> {
    use base64::Engine;
    let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i.saturating_add(2))?, 16).ok()).collect::<Option<_>>()?;
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use crate::backend::fixture;

    #[test]
    fn md5_in_base64() {
        assert_eq!(super::hex_to_base64("c578d37450280436da83c3cf3b022fd5").as_deref(), Some("xXjTdFAoBDbag8PPOwIv1Q=="));
        assert_eq!(super::hex_to_base64("666f6f").as_deref(), Some("Zm9v"));
        assert_eq!(super::hex_to_base64("6f").as_deref(), Some("bw=="));
        assert_eq!(super::hex_to_base64("zz"), None);
    }

    use super::Makaba;
    use crate::markup::{is_quote_link, is_spoiler};

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
        assert!(all.iter().any(|s| s.content.starts_with('>') && s.style == crate::markup::GREENTEXT));
        assert!(all.iter().any(|s| is_spoiler(s.style)));
        assert!(!all.iter().any(|s| s.content.contains("(OP)")));
    }

    #[test]
    fn post_lookup_gives_the_thread() {
        let v = fixture("makaba_post.json");
        assert_eq!(crate::http::as_u64(&v["post"]["parent"]), Some(337030181));
    }
}
