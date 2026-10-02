//! FoolFuuka 4chan archives (desuarchive, b4k, ...): the `/_/api/chan/` JSON API.

use anyhow::Result;
use serde_json::Value;

use super::{Backend, Partial, SearchPage};
use crate::http::{as_bool, as_i64, as_str, as_u64, encode_segment as enc, get_json, items, register_media_host};
use crate::markup::{self, Flavor};
use crate::model::{Attachment, Board, Post};

/// Index pages fetched for the "catalog" (each is one rate-limited request).
const INDEX_PAGES: u32 = 3;

pub struct Foolfuuka {
    base: String,
    boards: Option<Vec<Board>>,
}

impl Foolfuuka {
    pub fn new(base: String, boards: Option<Vec<Board>>) -> Self {
        Self { base, boards }
    }

    fn api(&self, query: &str) -> Result<Value> {
        get_json(&format!("{}/_/api/chan/{query}", self.base))
    }
}

/// Archived 4chan boards from `/_/api/chan/archives/`.
pub fn parse_archives(v: &Value) -> Vec<Board> {
    let mut boards: Vec<Board> = v["archives"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .filter_map(|b| {
            Some(Board {
                uri: as_str(&b["shortname"])?,
                title: as_str(&b["name"]).map(|t| markup::decode(&t)).unwrap_or_default(),
                nsfw: None,
            })
        })
        .collect();
    boards.sort_by(|a, b| a.uri.cmp(&b.uri));
    boards
}

/// One index page: thread OPs with reply counts. The JSON object is in bump order, but
/// serde_json sorts keys, so order by the newest post shown instead.
pub fn parse_index(v: &Value) -> Vec<Post> {
    let mut threads: Vec<(i64, Post)> = v
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .filter_map(|t| {
            let mut op = post(&t["op"])?;
            let last = t["posts"].as_array().map_or(&[][..], Vec::as_slice);
            let bumped = last.iter().filter_map(|p| as_i64(&p["timestamp"])).max().unwrap_or(op.time);
            let shown_images = last.iter().filter(|p| p["media"].is_object()).count() as u64;
            op.replies = Some(as_u64(&t["omitted"]).unwrap_or(0).saturating_add(last.len() as u64) as u32);
            op.images = Some(as_u64(&t["images_omitted"]).unwrap_or(0).saturating_add(shown_images) as u32);
            Some((bumped, op))
        })
        .collect();
    threads.sort_by_key(|(bumped, _)| std::cmp::Reverse(*bumped));
    threads.into_iter().map(|(_, p)| p).collect()
}

/// A thread from `/_/api/chan/thread/`: `{ "<no>": { "op": {...}, "posts": { "<no>": {...} } } }`.
pub fn parse_thread(v: &Value) -> Vec<Post> {
    let Some(t) = v.as_object().and_then(|m| m.values().next()) else { return Vec::new() };
    let mut posts: Vec<Post> = post(&t["op"]).into_iter().collect();
    let mut replies: Vec<Post> = t["posts"].as_object().into_iter().flat_map(|m| m.values()).filter_map(post).collect();
    replies.sort_by_key(|p| p.no);
    posts.extend(replies);
    posts
}

/// Search results: `{"0": {"posts": [...]}, "meta": {"total_found": N}}`, or
/// `{"error": "..."}` (no results, searching too often, search turned off).
pub fn parse_search(v: &Value) -> Result<SearchPage> {
    if let Some(e) = as_str(&v["error"]) {
        if e.starts_with("No results") {
            return Ok(SearchPage { hits: Vec::new(), total: Some(0) });
        }
        anyhow::bail!("{}", markup::decode(&e));
    }
    let hits = items(&v["0"]["posts"])
        .filter_map(|p| Some((as_u64(&p["thread_num"])?, post(p)?)))
        .collect();
    Ok(SearchPage { hits, total: as_u64(&v["meta"]["total_found"]) })
}

fn post(v: &Value) -> Option<Post> {
    // Ghost posts (made on the archive after the thread died) have a subnum; skip them.
    if as_u64(&v["subnum"]).unwrap_or(0) != 0 {
        return None;
    }
    let parsed = markup::parse_html(v["comment_processed"].as_str().unwrap_or(""), Flavor::Vichan);
    let mut name = as_str(&v["name_processed"]).or_else(|| as_str(&v["name"])).map(|n| markup::decode(&n)).unwrap_or_else(|| "Anonymous".into());
    if let Some(trip) = as_str(&v["trip"]) {
        name.push(' ');
        name.push_str(&trip);
    }
    let cap = match as_str(&v["capcode"]).as_deref() {
        Some("M") => Some("Mod"),
        Some("A") => Some("Admin"),
        Some("D") => Some("Developer"),
        Some("V") => Some("Verified"),
        _ => None,
    };
    if let Some(cap) = cap {
        name.push_str(&format!(" ## {cap}"));
    }
    Some(Post {
        no: as_u64(&v["num"])?,
        name,
        subject: as_str(&v["title_processed"]).or_else(|| as_str(&v["title"])).map(|s| markup::decode(&s)),
        time: as_i64(&v["timestamp"]).unwrap_or(0),
        files: attachment(&v["media"]).into_iter().collect(),
        sticky: as_bool(&v["sticky"]),
        board: as_str(&v["board"]["shortname"]),
        locked: as_bool(&v["locked"]),
        ..parsed.into()
    })
}

fn attachment(m: &Value) -> Option<Attachment> {
    if !m.is_object() || as_str(&m["media_status"]).is_some_and(|s| s == "banned") {
        return None;
    }
    let thumb = as_str(&m["thumb_link"]);
    // The full file isn't always archived; fall back to the thumbnail.
    let url = as_str(&m["media_link"]).or_else(|| as_str(&m["remote_media_link"])).or_else(|| thumb.clone())?;
    if let Some(t) = &thumb {
        register_media_host(t);
    }
    register_media_host(&url);
    let spoiler = as_bool(&m["spoiler"]);
    Some(Attachment {
        filename: as_str(&m["media_filename_processed"]).or_else(|| as_str(&m["media_filename"])).map(|f| markup::decode(&f)).unwrap_or_default(),
        url,
        thumb: if spoiler { None } else { thumb },
        spoiler,
        width: as_u64(&m["media_w"]).map(|n| n as u32),
        height: as_u64(&m["media_h"]).map(|n| n as u32),
        size: as_u64(&m["media_size"]),
        md5: as_str(&m["media_hash"]),
    })
}

impl Backend for Foolfuuka {
    fn boards(&self, _partial: Partial<Board>) -> Result<Vec<Board>> {
        if let Some(b) = &self.boards {
            return Ok(b.clone());
        }
        Ok(parse_archives(&self.api("archives/")?))
    }

    fn catalog(&self, board: &str, partial: Partial<Post>) -> Result<Vec<Post>> {
        let mut out: Vec<Post> = Vec::new();
        for page in 1..=INDEX_PAGES {
            if page > 1 {
                partial(&out);
            }
            let v = self.api(&format!("index/?board={}&page={page}", enc(board)));
            // A later page failing (or running out) still leaves the earlier ones.
            let v = match v {
                Ok(v) => v,
                Err(e) if page == 1 => return Err(e),
                Err(_) => break,
            };
            let threads = parse_index(&v);
            if threads.is_empty() {
                break;
            }
            for t in threads {
                if !out.iter().any(|p| p.no == t.no) {
                    out.push(t);
                }
            }
        }
        Ok(out)
    }

    fn search(&self, board: &str, query: &str, page: u32) -> Result<SearchPage> {
        parse_search(&self.api(&format!("search/?boards={}&text={}&page={page}", enc(board), enc(query)))?)
    }

    fn thread(&self, board: &str, no: u64) -> Result<Vec<Post>> {
        Ok(parse_thread(&self.api(&format!("thread/?board={}&num={no}", enc(board)))?))
    }

    fn find_thread(&self, board: &str, post: u64) -> Result<Option<u64>> {
        let v = self.api(&format!("post/?board={}&num={post}", enc(board)))?;
        Ok(as_u64(&v["thread_num"]))
    }

    fn board_url(&self, board: &str) -> String {
        format!("{}/{}/", self.base, enc(board))
    }

    fn thread_url(&self, board: &str, no: u64) -> String {
        format!("{}/{}/thread/{no}/", self.base, enc(board))
    }
}

#[cfg(test)]
mod tests {
    use crate::backend::fixture;

    #[test]
    fn archives() {
        let boards = super::parse_archives(&fixture("foolfuuka_archives.json"));
        assert!(!boards.is_empty());
        assert!(boards.iter().any(|b| b.uri == "a" && b.title == "Anime & Manga"));
    }

    #[test]
    fn index() {
        let threads = super::parse_index(&fixture("foolfuuka_index.json"));
        assert_eq!(threads.len(), 4);
        let op = threads.iter().find(|t| t.no == 109960193).unwrap();
        assert_eq!(op.subject.as_deref(), Some("I feel myself becoming dumber when I use LLMs"));
        let f = &op.files[0];
        assert_eq!(f.url, "https://desu-usergeneratedcontent.xyz/g/image/1790/89/1790897522450.jpg");
        assert_eq!(f.thumb.as_deref(), Some("https://desu-usergeneratedcontent.xyz/g/thumb/1790/89/1790897522450s.jpg"));
        assert_eq!((f.width, f.height, f.size), (Some(1536), Some(2048), Some(288112)));
        assert!(!op.sticky && op.replies.is_some());
    }

    #[test]
    fn search_results_and_errors() {
        let page = super::parse_search(&fixture("foolfuuka_search.json")).unwrap();
        assert_eq!(page.total, Some(4290));
        assert_eq!(page.hits.len(), 4);
        let (thread, p) = &page.hits[0];
        assert_eq!((*thread, p.no, p.board.as_deref()), (109914360, 109920091, Some("g")));
        assert!(p.plain_text().contains("borrow checking"));
        let none = super::parse_search(&serde_json::json!({"error": "No results found."})).unwrap();
        assert_eq!((none.hits.len(), none.total), (0, Some(0)));
        let err = super::parse_search(&serde_json::json!({"error": "You&#039;re searching too fast."})).err().unwrap();
        assert_eq!(err.to_string(), "You're searching too fast.");
    }

    #[test]
    fn thread_and_post() {
        let posts = super::parse_thread(&fixture("foolfuuka_thread.json"));
        assert_eq!(posts[0].no, 109959723);
        assert!(posts.windows(2).all(|w| w[0].no < w[1].no));
        // Quotes of the OP carry the thread in their href.
        let reply = posts.iter().find(|p| p.quotes.contains(&109959723)).unwrap();
        assert!(reply.links.iter().any(|l| l.thread == Some(109959723)));
        // Backlinks to the archive's own pages are quote links, not web links.
        assert!(posts.iter().flat_map(|p| &p.urls).all(|u| !u.contains("desuarchive.org/g/thread/109959723")));
        assert_eq!(crate::http::as_u64(&fixture("foolfuuka_post.json")["thread_num"]), Some(109959723));
    }
}
