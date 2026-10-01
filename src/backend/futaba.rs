//! 4chan's JSON API, and the vichan family that clones it.

use anyhow::{Result, bail};
use serde_json::Value;

use super::Backend;
use crate::http::{as_bool, as_i64, as_str, as_u64, encode_segment as enc, get_json};
use crate::markup;
use crate::model::{Attachment, Board, Post};

pub struct Futaba {
    api: String,
    web: String,
    /// `{media}/{board}/{tim}{ext}` for 4chan, `{media}/{board}/src/{tim}{ext}` for vichan.
    media: String,
    is_4chan: bool,
    boards: Option<Vec<Board>>,
}

impl Futaba {
    pub fn fourchan(boards: Option<Vec<Board>>) -> Self {
        Self {
            api: "https://a.4cdn.org".into(),
            web: "https://boards.4chan.org".into(),
            media: "https://i.4cdn.org".into(),
            is_4chan: true,
            boards,
        }
    }

    pub fn vichan(base: String, boards: Option<Vec<Board>>) -> Self {
        Self { api: base.clone(), web: base.clone(), media: base, is_4chan: false, boards }
    }

    fn file_url(&self, board: &str, tim: &str, ext: &str) -> String {
        if self.is_4chan {
            format!("{}/{}/{tim}{ext}", self.media, enc(board))
        } else {
            format!("{}/{}/src/{tim}{ext}", self.media, enc(board))
        }
    }

    fn attachment(&self, board: &str, v: &Value) -> Option<Attachment> {
        let tim = as_str(&v["tim"])?;
        let ext = as_str(&v["ext"]).unwrap_or_default();
        // vichan uses this marker for deleted files.
        if ext == "deleted" || as_bool(&v["filedeleted"]) {
            return None;
        }
        let filename = as_str(&v["filename"]).map(|f| markup::decode(&f)).unwrap_or_else(|| tim.clone());
        Some(Attachment {
            filename: format!("{filename}{ext}"),
            url: self.file_url(board, &tim, &ext),
            width: as_u64(&v["w"]).map(|n| n as u32),
            height: as_u64(&v["h"]).map(|n| n as u32),
            size: as_u64(&v["fsize"]),
        })
    }

    fn post(&self, board: &str, v: &Value) -> Post {
        let parsed = markup::parse_html(v["com"].as_str().unwrap_or(""));
        let mut name = as_str(&v["name"]).map(|n| markup::decode(&n)).unwrap_or_else(|| "Anonymous".into());
        if let Some(trip) = as_str(&v["trip"]) {
            name.push(' ');
            name.push_str(&trip);
        }
        if let Some(cap) = as_str(&v["capcode"]) {
            name.push_str(&format!(" ## {cap}"));
        }
        let mut files: Vec<_> = self.attachment(board, v).into_iter().collect();
        if let Some(extra) = v["extra_files"].as_array() {
            files.extend(extra.iter().filter_map(|f| self.attachment(board, f)));
        }
        Post {
            no: as_u64(&v["no"]).unwrap_or(0),
            name,
            subject: as_str(&v["sub"]).map(|s| markup::decode(&s)),
            time: as_i64(&v["time"]).unwrap_or(0),
            body: parsed.lines,
            quotes: parsed.quotes,
            files,
            replies: as_u64(&v["replies"]).map(|n| n as u32),
            images: as_u64(&v["images"]).map(|n| n as u32),
            sticky: as_bool(&v["sticky"]),
            locked: as_bool(&v["closed"]) || as_bool(&v["locked"]),
        }
    }
}

impl Backend for Futaba {
    fn boards(&self) -> Result<Vec<Board>> {
        if let Some(b) = &self.boards {
            return Ok(b.clone());
        }
        // 4chan has boards.json; a few vichan installs do too.
        let v = get_json(&format!("{}/boards.json", self.api));
        let v = match v {
            Ok(v) => v,
            Err(e) if self.is_4chan => return Err(e),
            Err(_) => bail!("this site has no board list API; add `boards = [...]` to its config"),
        };
        let list = v["boards"].as_array().cloned().unwrap_or_default();
        Ok(list
            .iter()
            .filter_map(|b| {
                Some(Board {
                    uri: as_str(&b["board"]).or_else(|| as_str(&b["uri"]))?,
                    title: as_str(&b["title"]).map(|t| markup::decode(&t)).unwrap_or_default(),
                    nsfw: b.get("ws_board").map(|w| !as_bool(w)),
                })
            })
            .collect())
    }

    fn catalog(&self, board: &str) -> Result<Vec<Post>> {
        let v = get_json(&format!("{}/{}/catalog.json", self.api, enc(board)))?;
        let pages = v.as_array().cloned().unwrap_or_default();
        Ok(pages
            .iter()
            .flat_map(|p| p["threads"].as_array().cloned().unwrap_or_default())
            .map(|t| self.post(board, &t))
            .collect())
    }

    fn thread(&self, board: &str, no: u64) -> Result<Vec<Post>> {
        let path = if self.is_4chan { "thread" } else { "res" };
        let v = get_json(&format!("{}/{}/{path}/{no}.json", self.api, enc(board)))?;
        let posts = v["posts"].as_array().cloned().unwrap_or_default();
        Ok(posts.iter().map(|p| self.post(board, p)).collect())
    }

    fn board_url(&self, board: &str) -> String {
        format!("{}/{}/", self.web, enc(board))
    }

    fn thread_url(&self, board: &str, no: u64) -> String {
        if self.is_4chan {
            format!("{}/{}/thread/{no}", self.web, enc(board))
        } else {
            format!("{}/{}/res/{no}.html", self.web, enc(board))
        }
    }
}
