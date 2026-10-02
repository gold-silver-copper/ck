//! jschan engine JSON API (zzzchan, ...).

use anyhow::Result;
use serde_json::Value;

use super::{Backend, Partial};
use crate::http::{as_bool, as_i64, as_str, as_u64, encode_segment as enc, get_json};
use crate::markup::{self, Flavor};
use crate::model::{Attachment, Board, Post};

/// The board list is paginated, local boards first, then webring boards from other sites.
const MAX_BOARD_PAGES: u64 = 5;

pub struct Jschan {
    base: String,
    boards: Option<Vec<Board>>,
}

impl Jschan {
    pub fn new(base: String, boards: Option<Vec<Board>>) -> Self {
        Self { base, boards }
    }
}

/// Local boards from one page of `/boards.json?local_first=true`, and whether the page
/// already reached the webring entries (so later pages have no local boards).
pub fn parse_boards(v: &Value) -> (Vec<Board>, bool) {
    let list = v["boards"].as_array().cloned().unwrap_or_default();
    let reached_webring = list.iter().any(|b| as_bool(&b["webring"]));
    let boards = list
        .iter()
        .filter(|b| !as_bool(&b["webring"]))
        .filter_map(|b| {
            Some(Board {
                uri: as_str(&b["_id"])?,
                title: as_str(&b["settings"]["name"]).unwrap_or_default(),
                nsfw: b["settings"]["sfw"].as_bool().map(|sfw| !sfw),
            })
        })
        .collect();
    (boards, reached_webring)
}

/// A thread from `/{board}/thread/{no}.json`: the OP with its `replies`.
pub fn parse_thread(base: &str, v: &Value) -> Vec<Post> {
    let mut posts = vec![post(base, v)];
    posts.extend(v["replies"].as_array().into_iter().flatten().map(|p| post(base, p)));
    posts
}

pub fn post(base: &str, v: &Value) -> Post {
    let parsed = markup::parse_html(v["message"].as_str().unwrap_or(""), Flavor::Jschan);
    let mut name = as_str(&v["name"]).unwrap_or_else(|| "Anonymous".into());
    if let Some(trip) = as_str(&v["tripcode"]) {
        name.push(' ');
        name.push_str(&trip);
    }
    if let Some(cap) = as_str(&v["capcode"]) {
        name.push_str(&format!(" {}", cap.trim()));
    }
    let time = as_i64(&v["u"])
        .map(|ms| ms / 1000)
        .or_else(|| v["date"].as_str().and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok()).map(|t| t.timestamp()))
        .unwrap_or(0);
    let files = v["files"].as_array().into_iter().flatten().filter_map(|f| attachment(base, f)).collect();
    Post {
        no: as_u64(&v["postId"]).unwrap_or(0),
        name,
        subject: as_str(&v["subject"]),
        time,
        body: parsed.lines,
        quotes: parsed.quotes,
        links: parsed.links,
        files,
        replies: as_u64(&v["replyposts"]).map(|n| n as u32),
        images: as_u64(&v["replyfiles"]).map(|n| n as u32),
        sticky: as_bool(&v["sticky"]),
        locked: as_bool(&v["locked"]),
        ..Default::default()
    }
}

/// Files live at `/file/{filename}`, thumbnails at `/file/thumb/{hash}{thumbextension}`.
fn attachment(base: &str, f: &Value) -> Option<Attachment> {
    let filename = as_str(&f["filename"])?;
    let spoiler = as_bool(&f["spoiler"]);
    let mime = as_str(&f["mimetype"]).unwrap_or_default();
    let has_thumb = mime.starts_with("image/") || mime.starts_with("video/");
    let thumb = match (as_str(&f["hash"]), as_str(&f["thumbextension"])) {
        (Some(hash), Some(ext)) if has_thumb && !spoiler => Some(format!("{base}/file/thumb/{hash}{ext}")),
        _ => None,
    };
    Some(Attachment {
        filename: as_str(&f["originalFilename"]).unwrap_or_else(|| filename.clone()),
        url: format!("{base}/file/{filename}"),
        thumb,
        spoiler,
        width: as_u64(&f["geometry"]["width"]).map(|n| n as u32),
        height: as_u64(&f["geometry"]["height"]).map(|n| n as u32),
        size: as_u64(&f["size"]),
    })
}

impl Backend for Jschan {
    fn boards(&self, partial: Partial<Board>) -> Result<Vec<Board>> {
        if let Some(b) = &self.boards {
            return Ok(b.clone());
        }
        let mut out = Vec::new();
        for page in 1..=MAX_BOARD_PAGES {
            let v = get_json(&format!("{}/boards.json?local_first=true&page={page}", self.base))?;
            let (boards, done) = parse_boards(&v);
            out.extend(boards);
            if done || page >= as_u64(&v["maxPage"]).unwrap_or(1) {
                break;
            }
            partial(&out);
        }
        Ok(out)
    }

    fn catalog(&self, board: &str, _partial: Partial<Post>) -> Result<Vec<Post>> {
        let v = get_json(&format!("{}/{}/catalog.json", self.base, enc(board)))?;
        Ok(v.as_array().into_iter().flatten().map(|t| post(&self.base, t)).collect())
    }

    fn thread(&self, board: &str, no: u64) -> Result<Vec<Post>> {
        let v = get_json(&format!("{}/{}/thread/{no}.json", self.base, enc(board)))?;
        Ok(parse_thread(&self.base, &v))
    }

    fn board_url(&self, board: &str) -> String {
        format!("{}/{}/", self.base, enc(board))
    }

    fn thread_url(&self, board: &str, no: u64) -> String {
        format!("{}/{}/thread/{no}.html", self.base, enc(board))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    const BASE: &str = "https://zzzchan.xyz";

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn boards_skip_webring() {
        let (boards, done) = super::parse_boards(&fixture("jschan_boards.json"));
        assert!(done);
        assert_eq!(boards.len(), 3);
        assert_eq!((boards[0].uri.as_str(), boards[0].title.as_str()), ("v", "Video Games"));
    }

    #[test]
    fn catalog_files() {
        let cat = fixture("jschan_catalog.json");
        let posts: Vec<_> = cat.as_array().unwrap().iter().map(|t| super::post(BASE, t)).collect();
        let sticky = &posts[0];
        assert!(sticky.sticky && sticky.replies == Some(215));
        let f = &sticky.files[0];
        let hash = "7330ef9178e4de4b06d0f9daec58c109b4d7e61ad7300ddf9417336d40e7a742";
        assert_eq!(f.url, format!("{BASE}/file/{hash}.png"));
        assert_eq!(f.thumb, Some(format!("{BASE}/file/thumb/{hash}.png")));
        let spoiler = posts.iter().flat_map(|p| &p.files).find(|f| f.spoiler).unwrap();
        assert!(spoiler.thumb.is_none());
        let video = posts.iter().flat_map(|p| &p.files).find(|f| f.is_video()).unwrap();
        assert!(video.thumb.as_deref().unwrap().ends_with(".jpg"));
    }

    #[test]
    fn thread_quotes() {
        let posts = super::parse_thread(BASE, &fixture("jschan_thread.json"));
        assert_eq!(posts[0].no, 321302);
        let reply = posts.iter().find(|p| p.quotes.contains(&321302)).unwrap();
        // jschan's own "(OP)" annotation is dropped (ck adds its own).
        let text: String = reply.body[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.trim_end(), ">>321302");
    }
}
