//! FoolFuuka 4chan archives (desuarchive, b4k, ...): the `/_/api/chan/` JSON API.
#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]

use anyhow::Result;
use serde_json::Value;

use super::{Backend, Partial, SearchPage, as_u32, saturate, site_error};
use crate::http::{as_bool, as_i64, as_str, as_u64, encode_segment as enc, get_json, items, register_media_host};
use crate::markup::{self, Flavor};
use crate::model::{Attachment, Board, FileKind, Flag, Post, Poster};

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

    /// `api`, with an `{"error": ...}` answer an error (not found, if it says so).
    fn checked(&self, query: &str) -> Result<Value> {
        let v = self.api(query)?;
        v.get("error").and_then(as_str).map_or(Ok(v), |msg| Err(site_error(&format!("{}/_/api/chan/{query}", self.base), &msg)))
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
            // Ghost posts aren't counted: post() leaves them out of the thread too.
            let last: Vec<Post> = t["posts"].as_array().into_iter().flatten().filter_map(post).collect();
            let bumped = last.iter().map(|p| p.time).max().unwrap_or(op.time);
            let shown_images = last.iter().filter(|p| !p.files.is_empty()).count() as u64;
            op.replies = Some(saturate(as_u64(&t["omitted"]).unwrap_or(0).saturating_add(last.len() as u64)));
            op.images = Some(saturate(as_u64(&t["images_omitted"]).unwrap_or(0).saturating_add(shown_images)));
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
    let hits = v.get("0").into_iter().flat_map(|r| items(&r["posts"]))
        .filter_map(|p| Some((as_u64(&p["thread_num"])?, post(p)?)))
        .collect();
    Ok(SearchPage { hits, total: v.get("meta").and_then(|m| as_u64(&m["total_found"])) })
}

fn post(v: &Value) -> Option<Post> {
    // Ghost posts (made on the archive after the thread died) have a subnum; skip them.
    if as_u64(&v["subnum"]).unwrap_or(0) != 0 {
        return None;
    }
    let parsed = markup::parse_html(v["comment_processed"].as_str().unwrap_or(""), Flavor::Vichan);
    let name = as_str(&v["name_processed"]).or_else(|| as_str(&v["name"])).map(|n| markup::decode(&n));
    // A letter for the capcode 4chan spells out; `N` is none.
    let letter = |c: &str| match c { "M" => "mod", "A" => "admin", "D" => "developer", "V" => "verified", "F" => "founder", "G" => "manager", c => c }.to_string();
    let capcode = as_str(&v["capcode"]).filter(|c| c != "N").map(|c| letter(&c));
    let text = |k: &str| as_str(&v[k]).map(|s| markup::decode(&s));
    Some(Post {
        no: as_u64(&v["num"])?,
        poster: Poster::new(name, "Anonymous", as_str(&v["trip"]), capcode),
        id: text("poster_hash"),
        flag: Flag::new(text("poster_country"), text("poster_country_name")),
        subject: as_str(&v["title_processed"]).or_else(|| as_str(&v["title"])).map(|s| markup::decode(&s)),
        time: as_i64(&v["timestamp"]).unwrap_or(0),
        files: attachment(&v["media"]).into_iter().collect(),
        sticky: as_bool(&v["sticky"]),
        board: v.get("board").and_then(|b| as_str(&b["shortname"])),
        locked: as_bool(&v["locked"]),
        ..parsed.into()
    })
}

fn attachment(m: &Value) -> Option<Attachment> {
    if !m.is_object() || as_str(&m["media_status"]).is_some_and(|s| s == "banned") {
        return None;
    }
    let thumb = as_str(&m["thumb_link"]);
    // The full file isn't always archived; then there's only the thumbnail (or nothing).
    let url = as_str(&m["media_link"]).or_else(|| as_str(&m["remote_media_link"]));
    url.as_ref().or(thumb.as_ref())?;
    url.iter().chain(&thumb).for_each(|u| register_media_host(u));
    let spoiler = as_bool(&m["spoiler"]);
    let filename = as_str(&m["media_filename_processed"]).or_else(|| as_str(&m["media_filename"])).map(|f| markup::decode(&f)).unwrap_or_default();
    Some(Attachment {
        kind: FileKind::of(None, url.as_deref(), &filename),
        filename,
        url,
        thumb: if spoiler { None } else { thumb },
        spoiler,
        width: as_u32(&m["media_w"]),
        height: as_u32(&m["media_h"]),
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

    fn thread_unchecked(&self, board: &str, no: u64) -> Result<Vec<Post>> {
        Ok(parse_thread(&self.checked(&format!("thread/?board={}&num={no}", enc(board)))?))
    }

    fn find_thread(&self, board: &str, post: u64) -> Result<Option<u64>> {
        let v = self.checked(&format!("post/?board={}&num={post}", enc(board)))?;
        Ok(v.get("thread_num").and_then(as_u64))
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
        assert_eq!(f.url.as_deref(), Some("https://desu-usergeneratedcontent.xyz/g/image/1790/89/1790897522450.jpg"));
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
    fn ids_and_flags() {
        // The fixture's /g/ has none; an archived /pol/ post has them all.
        let v = fixture("foolfuuka_post.json");
        let p = super::post(&v).unwrap();
        assert!(p.id.is_none() && p.flag.is_none() && p.poster.trip().is_none() && p.poster.capcode().is_none());
        let mut v = v;
        v["poster_hash"] = "Ab3dEf+g".into();
        v["poster_country"] = "FI".into();
        v["poster_country_name"] = "Finland".into();
        v["trip"] = "!!Fz3mQwerty".into();
        v["capcode"] = "M".into();
        let p = super::post(&v).unwrap();
        assert_eq!((p.id.as_deref(), p.flag.as_ref().map(|f| f.short())), (Some("Ab3dEf+g"), Some("FI".into())));
        assert_eq!((p.poster.trip(), p.poster.capcode()), (Some("!!Fz3mQwerty"), Some("mod")));
    }

    #[test]
    fn founder_and_manager_posts_keep_their_capcode() {
        // FoolFuuka stores 4chan's founder and manager capcodes as "F" and "G".
        for letter in ["F", "G"] {
            let mut v = fixture("foolfuuka_post.json");
            v["capcode"] = letter.into();
            let p = super::post(&v).unwrap();
            assert!(p.poster.capcode().is_some(), "capcode {letter:?} is dropped");
            assert!(p.poster.name().contains(" ## "), "capcode {letter:?} is missing from the name {:?}", p.poster.name());
        }
    }

    #[test]
    fn index_reply_counts_leave_out_ghost_posts() {
        // A ghost reply (made on the archive after the thread died) isn't in the thread,
        // so the index's reply count doesn't count it either.
        let mut v = fixture("foolfuuka_index.json");
        let mut ghost = v["109960109"]["posts"][0].clone();
        ghost["subnum"] = "1".into();
        v["109960109"]["posts"].as_array_mut().unwrap().push(ghost);
        let op = super::parse_index(&v).into_iter().find(|t| t.no == 109960109).unwrap();
        assert_eq!(op.replies, Some(2), "the ghost reply is counted");
    }

    #[test]
    fn unarchived_file_is_not_its_thumbnail() {
        // A webm the archive kept only the thumbnail of: the file is still a video, and
        // a download must not save the JPEG thumbnail as `<no>_clip.webm`.
        let mut v = fixture("foolfuuka_post.json");
        let thumb = "https://desu-usergeneratedcontent.xyz/g/thumb/1790/89/1790897522450s.jpg";
        v["media"] = serde_json::json!({
            "media_status": "normal", "media_link": null, "remote_media_link": null, "thumb_link": thumb,
            "media_filename": "clip.webm", "media_w": 1280, "media_h": 720, "media_size": 3_000_000, "spoiler": "0",
        });
        let p = super::post(&v).unwrap();
        let f = &p.files[0];
        assert!(f.is_video(), "a .webm post's file is a video; ext() read the thumbnail's {:?}", f.ext());
        assert!(!f.is_image(), "the viewer would show the thumbnail as the full file");
        let jobs = crate::download::jobs(&[&p], std::path::Path::new("/d"));
        let bad: Vec<_> = jobs.iter().filter(|(url, path)| url == thumb && path.to_string_lossy().ends_with("clip.webm")).collect();
        assert!(bad.is_empty(), "the thumbnail is saved under the original's name: {bad:?}");
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
        // Lines are `<br />\n`: the newline doesn't start the next line with a space, and
        // a blank line is empty.
        let p = posts.iter().find(|p| p.no == 109959765).unwrap();
        let lines: Vec<String> = p.body.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert_eq!(lines[1..4], ["yeah it depends on how much tabs you keep open. ", "", "But beyond that, most of us have 16:9 screens or wider. for some productivity tasks, it is useful to have menus on the side."]);
        assert_eq!(lines.last().unwrap(), "I use both");
    }
}
