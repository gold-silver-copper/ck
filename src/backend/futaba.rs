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
    /// vichan's `thumb_ext` setting: fixed thumbnail extension, or `None` for "same as the file".
    thumb_ext: Option<String>,
    boards: Option<Vec<Board>>,
}

impl Futaba {
    pub fn fourchan(boards: Option<Vec<Board>>) -> Self {
        crate::http::register_media_host("https://i.4cdn.org");
        Self {
            api: "https://a.4cdn.org".into(),
            web: "https://boards.4chan.org".into(),
            media: "https://i.4cdn.org".into(),
            is_4chan: true,
            thumb_ext: None,
            boards,
        }
    }

    pub fn vichan(base: String, thumb_ext: Option<String>, boards: Option<Vec<Board>>) -> Self {
        let thumb_ext = thumb_ext.map(|e| e.trim_start_matches('.').to_string());
        Self { api: base.clone(), web: base.clone(), media: base, is_4chan: false, thumb_ext, boards }
    }

    fn file_url(&self, board: &str, tim: &str, ext: &str) -> String {
        if self.is_4chan {
            format!("{}/{}/{tim}{ext}", self.media, enc(board))
        } else {
            format!("{}/{}/src/{tim}{ext}", self.media, enc(board))
        }
    }

    /// 4chan: `{tim}s.jpg` on the media host. vichan: `/{board}/thumb/{tim}.{ext}`, where ext is
    /// the site's `thumb_ext`, else the file's own extension for images and `jpg` for videos.
    fn thumb_url(&self, board: &str, tim: &str, ext: &str) -> Option<String> {
        if self.is_4chan {
            return Some(format!("{}/{}/{tim}s.jpg", self.media, enc(board)));
        }
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        let thumb_ext = match ext.as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" => self.thumb_ext.clone().unwrap_or(ext),
            "webm" | "mp4" => self.thumb_ext.clone().unwrap_or_else(|| "jpg".into()),
            _ => return None, // generic file icon
        };
        Some(format!("{}/{}/thumb/{tim}.{thumb_ext}", self.media, enc(board)))
    }

    fn attachment(&self, board: &str, v: &Value) -> Option<Attachment> {
        let tim = as_str(&v["tim"])?;
        let ext = as_str(&v["ext"]).unwrap_or_default();
        // vichan uses this marker for deleted files.
        if ext == "deleted" || as_bool(&v["filedeleted"]) {
            return None;
        }
        let filename = as_str(&v["filename"]).map(|f| markup::decode(&f)).unwrap_or_else(|| tim.clone());
        let spoiler = as_bool(&v["spoiler"]);
        Some(Attachment {
            filename: format!("{filename}{ext}"),
            url: self.file_url(board, &tim, &ext),
            thumb: if spoiler { None } else { self.thumb_url(board, &tim, &ext) },
            spoiler,
            width: as_u64(&v["w"]).map(|n| n as u32),
            height: as_u64(&v["h"]).map(|n| n as u32),
            size: as_u64(&v["fsize"]),
        })
    }

    fn post(&self, board: &str, v: &Value) -> Post {
        let flavor = if self.is_4chan { markup::Flavor::Fourchan } else { markup::Flavor::Vichan };
        let parsed = markup::parse_html(v["com"].as_str().unwrap_or(""), flavor);
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
            links: parsed.links,
            files,
            replies: as_u64(&v["replies"]).map(|n| n as u32),
            images: as_u64(&v["images"]).map(|n| n as u32),
            sticky: as_bool(&v["sticky"]),
            locked: as_bool(&v["closed"]) || as_bool(&v["locked"]),
            ..Default::default()
        }
    }
}

/// Boards from `boards.json`.
pub fn parse_boards(v: &Value) -> Vec<Board> {
    let list = v["boards"].as_array().cloned().unwrap_or_default();
    list.iter()
        .filter_map(|b| {
            Some(Board {
                uri: as_str(&b["board"]).or_else(|| as_str(&b["uri"]))?,
                title: as_str(&b["title"]).map(|t| markup::decode(&t)).unwrap_or_default(),
                nsfw: b.get("ws_board").map(|w| !as_bool(w)),
            })
        })
        .collect()
}

impl Futaba {
    /// Thread OPs from `catalog.json`: an array of pages with `threads`.
    pub fn parse_catalog(&self, board: &str, v: &Value) -> Vec<Post> {
        let pages = v.as_array().cloned().unwrap_or_default();
        pages
            .iter()
            .flat_map(|p| p["threads"].as_array().cloned().unwrap_or_default())
            .map(|t| self.post(board, &t))
            .collect()
    }

    /// Posts from a thread's JSON: `{ "posts": [...] }`, OP first.
    pub fn parse_thread(&self, board: &str, v: &Value) -> Vec<Post> {
        let posts = v["posts"].as_array().cloned().unwrap_or_default();
        posts.iter().map(|p| self.post(board, p)).collect()
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
        Ok(parse_boards(&v))
    }

    fn catalog(&self, board: &str) -> Result<Vec<Post>> {
        let v = get_json(&format!("{}/{}/catalog.json", self.api, enc(board)))?;
        Ok(self.parse_catalog(board, &v))
    }

    fn thread(&self, board: &str, no: u64) -> Result<Vec<Post>> {
        let path = if self.is_4chan { "thread" } else { "res" };
        let v = get_json(&format!("{}/{}/{path}/{no}.json", self.api, enc(board)))?;
        Ok(self.parse_thread(board, &v))
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

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::Futaba;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn catalog_thread(v: &Value, no: u64) -> Value {
        let threads = v[0]["threads"].as_array().unwrap();
        threads.iter().find(|t| t["no"].as_u64() == Some(no)).unwrap().clone()
    }

    #[test]
    fn fourchan_boards_catalog_thread() {
        let boards = super::parse_boards(&fixture("4chan_boards.json"));
        let g = boards.iter().find(|b| b.uri == "g").unwrap();
        assert_eq!((g.title.as_str(), g.nsfw), ("Technology", Some(false)));
        assert_eq!(boards.iter().find(|b| b.uri == "b").unwrap().nsfw, Some(true));

        let b = Futaba::fourchan(None);
        let cat = b.parse_catalog("g", &fixture("4chan_catalog.json"));
        assert_eq!(cat.len(), 6);
        assert!(cat[0].sticky && cat[0].locked);
        assert!(cat.iter().all(|p| p.replies.is_some()));

        let posts = b.parse_thread("g", &fixture("4chan_thread.json"));
        assert_eq!(posts[0].no, 109949798);
        assert!(posts.len() > 5);
        // Replies quote the OP, and the OP collects them as backlinks later.
        assert!(posts.iter().skip(1).any(|p| p.quotes.contains(&109949798)));
    }

    #[test]
    fn vichan_catalog_thread() {
        let b = Futaba::vichan("https://lainchan.org".into(), Some("png".into()), None);
        let cat = b.parse_catalog("λ", &fixture("vichan_catalog.json"));
        assert_eq!(cat[0].subject.as_deref(), Some("Programming Employment"));
        let posts = b.parse_thread("λ", &fixture("vichan_thread.json"));
        assert_eq!(posts[0].no, 30364);
        // vichan marks deleted files with ext "deleted"; they're dropped.
        assert!(posts.iter().flat_map(|p| &p.files).all(|f| !f.url.ends_with("deleted")));
        assert!(posts.iter().any(|p| p.links.iter().any(|l| l.thread == Some(30364))));
    }

    #[test]
    fn fourchan_thumbnails() {
        let b = Futaba::fourchan(None);
        let p = b.post("g", &catalog_thread(&fixture("4chan_catalog.json"), 109949798));
        let f = &p.files[0];
        assert_eq!(f.url, "https://i.4cdn.org/g/1790800293810251.mp4");
        assert_eq!(f.thumb.as_deref(), Some("https://i.4cdn.org/g/1790800293810251s.jpg"));
        assert!(f.is_video() && !f.spoiler);

        let p = b.post("a", &fixture("4chan_spoiler_post.json"));
        assert!(p.files[0].spoiler);
        assert_eq!(p.files[0].thumb, None);
    }

    #[test]
    fn vichan_thumbnails() {
        // lainchan renders every thumbnail as png.
        let lain = Futaba::vichan("https://lainchan.org".into(), Some("png".into()), None);
        let p = lain.post("λ", &catalog_thread(&fixture("vichan_catalog.json"), 42742));
        assert_eq!(p.files[0].thumb.as_deref(), Some("https://lainchan.org/%CE%BB/thumb/1754702648060-0.png"));

        // wizchan keeps the file's extension, and uses jpg for videos.
        let wiz = Futaba::vichan("https://wizchan.org".into(), None, None);
        let cat = fixture("wizchan_catalog.json");
        let thumb = |no| wiz.post("wiz", &catalog_thread(&cat, no)).files.first().and_then(|f| f.thumb.clone());
        assert_eq!(thumb(230021).as_deref(), Some("https://wizchan.org/wiz/thumb/1790081744989.jpg"));
        assert_eq!(thumb(211629).as_deref(), Some("https://wizchan.org/wiz/thumb/1696663189546.jpg"));
        // Deleted files are dropped.
        assert_eq!(thumb(229036), None);
    }
}
