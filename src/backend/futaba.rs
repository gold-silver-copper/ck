//! 4chan's JSON API, and the vichan family that clones it.
#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]

use std::fmt::Write as _;

use anyhow::{Result, bail};
use serde_json::Value;

use super::{Backend, Partial, as_u32};
use crate::http::{as_bool, as_i64, as_str, as_u64, encode_segment as enc, get_json, items};
use crate::markup;
use crate::model::{Attachment, Board, Flag, Post};

pub struct Futaba {
    api: String,
    web: String,
    /// `{media}/{board}/{tim}{ext}` for 4chan, `{media}/{board}/src/{tim}{ext}` for vichan, or
    /// `{media}/file_store/{tim}{ext}` for posts marked `fpath: 1` (8kun).
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

    /// `media` is where files are served from, if not the site itself (8kun).
    pub fn vichan(base: String, thumb_ext: Option<String>, media: Option<String>, boards: Option<Vec<Board>>) -> Self {
        let thumb_ext = thumb_ext.map(|e| e.trim_start_matches('.').to_string());
        let media = media.map(|m| m.trim_end_matches('/').to_string()).unwrap_or_else(|| base.clone());
        if media != base {
            crate::http::register_media_host(&media);
        }
        Self { api: base.clone(), web: base, media, is_4chan: false, thumb_ext, boards }
    }

    /// `flat`: the post's files are in one `file_store` directory, not per board.
    fn file_url(&self, board: &str, tim: &str, ext: &str, flat: bool) -> String {
        if self.is_4chan {
            format!("{}/{}/{tim}{ext}", self.media, enc(board))
        } else if flat {
            format!("{}/file_store/{tim}{ext}", self.media)
        } else {
            format!("{}/{}/src/{tim}{ext}", self.media, enc(board))
        }
    }

    /// 4chan: `{tim}s.jpg` on the media host. vichan: `/{board}/thumb/{tim}.{ext}`, where ext is
    /// the site's `thumb_ext`, else the file's own extension, for images, and always `jpg` for
    /// videos (checked on wizchan and tvch, whose thumb_ext is png).
    fn thumb_url(&self, board: &str, tim: &str, ext: &str, flat: bool) -> Option<String> {
        if self.is_4chan {
            return Some(format!("{}/{}/{tim}s.jpg", self.media, enc(board)));
        }
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        let thumb_ext = match ext.as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" => self.thumb_ext.clone().unwrap_or(ext),
            "webm" | "mp4" => "jpg".into(),
            _ => return None, // generic file icon
        };
        if flat {
            return Some(format!("{}/file_store/thumb/{tim}.{thumb_ext}", self.media));
        }
        Some(format!("{}/{}/thumb/{tim}.{thumb_ext}", self.media, enc(board)))
    }

    /// A `files` entry with explicit `file_path` and `thumb_path`.
    fn path_attachment(&self, f: &Value) -> Option<Attachment> {
        let path = as_str(&f["file_path"])?;
        let name = as_str(&f["filename"]).map(|n| markup::decode(&n)).unwrap_or_default();
        let spoiler = as_bool(&f["spoiler"]);
        Some(Attachment {
            filename: format!("{name}{}", as_str(&f["ext"]).unwrap_or_default()),
            url: format!("{}{path}", self.media),
            thumb: as_str(&f["thumb_path"]).filter(|_| !spoiler).map(|t| format!("{}{t}", self.media)),
            spoiler,
            width: as_u32(&f["w"]),
            height: as_u32(&f["h"]),
            size: as_u64(&f["fsize"]),
            md5: as_str(&f["md5"]),
        })
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
        let flat = as_u64(&v["fpath"]) == Some(1);
        Some(Attachment {
            filename: format!("{filename}{ext}"),
            url: self.file_url(board, &tim, &ext, flat),
            thumb: if spoiler { None } else { self.thumb_url(board, &tim, &ext, flat) },
            spoiler,
            width: as_u32(&v["w"]),
            height: as_u32(&v["h"]),
            size: as_u64(&v["fsize"]),
            md5: as_str(&v["md5"]),
        })
    }

    fn post(&self, board: &str, v: &Value) -> Post {
        let flavor = if self.is_4chan { markup::Flavor::Fourchan } else { markup::Flavor::Vichan };
        let parsed = markup::parse_html(v["com"].as_str().unwrap_or(""), flavor);
        let mut name = as_str(&v["name"]).map(|n| markup::decode(&n)).unwrap_or_else(|| "Anonymous".into());
        let (trip, capcode) = (as_str(&v["trip"]), as_str(&v["capcode"]));
        if let Some(trip) = &trip {
            name.push(' ');
            name.push_str(trip);
        }
        if let Some(cap) = &capcode {
            let _ = write!(name, " ## {cap}");
        }
        // A country's flag, else a board's own (4chan's /pol/); vichan forks use `country`
        // for custom flags too.
        let text = |k: &str| as_str(&v[k]).map(|s| markup::decode(&s));
        let flag = Flag::new(text("country"), text("country_name")).or_else(|| Flag::new(text("board_flag"), text("flag_name")));
        // On an overboard, files live under the thread's own board.
        let own_board = as_str(&v["board"]);
        let board = own_board.as_deref().unwrap_or(board);
        let mut files: Vec<_> = self.attachment(board, v).into_iter().collect();
        files.extend(items(&v["extra_files"]).filter_map(|f| self.attachment(board, f)));
        // Newer vichan forks (leftypol) list files with their paths instead.
        files.extend(items(&v["files"]).filter_map(|f| self.path_attachment(f)));
        Post {
            no: as_u64(&v["no"]).unwrap_or(0),
            name,
            subject: as_str(&v["sub"]).map(|s| markup::decode(&s)),
            time: as_i64(&v["time"]).unwrap_or(0),
            files,
            id: text("id"),
            flag,
            trip,
            capcode,
            replies: as_u32(&v["replies"]),
            images: as_u32(&v["images"]),
            sticky: as_bool(&v["sticky"]),
            board: own_board,
            locked: as_bool(&v["closed"]) || as_bool(&v["locked"]),
            bumplimit: as_bool(&v["bumplimit"]),
            ..parsed.into()
        }
    }

    /// Thread OPs from `catalog.json`: an array of pages with `threads`.
    pub fn parse_catalog(&self, board: &str, v: &Value) -> Vec<Post> {
        items(v).flat_map(|p| items(&p["threads"])).map(|t| self.post(board, t)).collect()
    }

    /// Posts from a thread's JSON: `{ "posts": [...] }`, OP first.
    pub fn parse_thread(&self, board: &str, v: &Value) -> Vec<Post> {
        items(&v["posts"]).map(|p| self.post(board, p)).collect()
    }
}

/// Boards from `boards.json`: `{ "boards": [...] }` (4chan), or a bare list (8kun).
pub fn parse_boards(v: &Value) -> Vec<Board> {
    let list = v["boards"].as_array().or(v.as_array()).into_iter().flatten();
    list.filter_map(|b| {
        Some(Board {
            uri: as_str(&b["board"]).or_else(|| as_str(&b["uri"]))?,
            title: as_str(&b["title"]).map(|t| markup::decode(t.trim())).unwrap_or_default(),
            nsfw: b.get("ws_board").or_else(|| b.get("sfw")).map(|w| !as_bool(w)),
        })
    })
    .collect()
}

impl Backend for Futaba {
    fn boards(&self, _partial: Partial<Board>) -> Result<Vec<Board>> {
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

    fn catalog(&self, board: &str, _partial: Partial<Post>) -> Result<Vec<Post>> {
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

    fn post_url(&self, board: &str, thread: u64, post: u64) -> String {
        let anchor = if self.is_4chan { "p" } else { "" };
        format!("{}#{anchor}{post}", self.thread_url(board, thread))
    }
}

#[cfg(test)]
mod tests {
    use crate::backend::fixture;
    use serde_json::Value;

    use super::Futaba;

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
        let b = Futaba::vichan("https://lainchan.org".into(), Some("png".into()), None, None);
        let cat = b.parse_catalog("λ", &fixture("vichan_catalog.json"));
        assert_eq!(cat[0].subject.as_deref(), Some("Programming Employment"));
        let posts = b.parse_thread("λ", &fixture("vichan_thread.json"));
        assert_eq!(posts[0].no, 30364);
        // vichan marks deleted files with ext "deleted"; they're dropped.
        assert!(posts.iter().flat_map(|p| &p.files).all(|f| !f.url.ends_with("deleted")));
        assert!(posts.iter().any(|p| p.links.iter().any(|l| l.thread == Some(30364))));
        assert!(posts.iter().any(|p| p.urls == ["https://youtu.be/nUsDk8wjRPs"]));
    }

    #[test]
    fn files_with_paths() {
        // leftypol's vichan fork: a `files` list with explicit paths and webp thumbnails.
        let b = Futaba::vichan("https://leftypol.org".into(), None, None, None);
        let posts = b.parse_thread("leftypol", &fixture("leftypol_thread.json"));
        assert_eq!(posts[0].no, 2923329);
        let files: Vec<_> = posts.iter().flat_map(|p| &p.files).collect();
        assert_eq!(files.len(), 7);
        let f = files.iter().find(|f| f.url.ends_with("1790754948757-9.jpg")).unwrap();
        assert_eq!(f.url, "https://leftypol.org/leftypol/src/1790754948757-9.jpg");
        assert_eq!(f.thumb.as_deref(), Some("https://leftypol.org/leftypol/thumb/1790754948757-9.webp"));
        assert_eq!((f.filename.as_str(), f.width, f.size), ("842251815915.jpg", Some(1080), Some(178528)));
        let cat = b.parse_catalog("leftypol", &fixture("leftypol_catalog.json"));
        assert!(cat.iter().any(|p| !p.files.is_empty()));
        let urls: Vec<&String> = posts.iter().flat_map(|p| &p.urls).collect();
        assert!(urls.iter().any(|u| *u == "https://jacobin.com/2026/09/economic-democracy-is-at-the-heart-of-socialism"));
    }

    #[test]
    fn overboard_threads_keep_their_board() {
        let b = Futaba::vichan("https://leftypol.org".into(), None, None, None);
        let cat = b.parse_catalog("overboard", &fixture("leftypol_overboard.json"));
        let boards: Vec<_> = cat.iter().map(|p| p.board.as_deref().unwrap()).collect();
        assert_eq!(boards, ["leftypol", "latam", "siberia", "tech", "games"]);
        // Files are under the thread's own board, not the overboard.
        assert!(cat.iter().flat_map(|p| &p.files).all(|f| !f.url.contains("/overboard/")));
    }

    #[test]
    fn eightkun() {
        let boards = super::parse_boards(&fixture("8kun_boards.json"));
        assert!(boards.iter().any(|b| b.uri == "qresearch" && b.title == "Q Research"));
        let b = Futaba::vichan("https://8kun.top".into(), None, Some("https://nerv.8kun.top".into()), None);
        let cat = b.parse_catalog("v", &fixture("8kun_catalog.json"));
        let f = |ext: &str| cat.iter().flat_map(|p| &p.files).find(|f| f.filename.ends_with(ext)).unwrap().clone();
        let jpg = f(".jpg");
        let tim = "888d8dada6c13986ebca008cfdf8eb67fbe39f21346ca1e5c6321171239238a9";
        assert_eq!(jpg.url, format!("https://nerv.8kun.top/file_store/{tim}.jpg"));
        assert_eq!(jpg.thumb, Some(format!("https://nerv.8kun.top/file_store/thumb/{tim}.jpg")));
        // Videos get jpg thumbnails, images keep their extension (checked live).
        assert!(f(".mp4").thumb.unwrap().ends_with(".jpg"));
        assert!(f(".webp").thumb.unwrap().ends_with(".webp"));
    }

    #[test]
    fn poster_ids_flags_trips_and_capcodes() {
        use crate::model::Flag;
        let flag = |code: &str, name: &str| Some(Flag { code: code.into(), name: name.into() });
        let posts = Futaba::fourchan(None).parse_thread("pol", &fixture("4chan_pol_thread.json"));
        let by = |no: u64| posts.iter().find(|p| p.no == no).unwrap();
        let op = by(487211034);
        assert_eq!((op.id.as_deref(), &op.flag), (Some("Ab3dEf+g"), &flag("US", "United States")));
        assert_eq!(posts.iter().filter(|p| p.id.as_deref() == Some("Ab3dEf+g")).count(), 2);
        // A board flag where there's no country.
        assert_eq!(by(487211201).flag, flag("AC", "Anarcho-Capitalist"));
        // The tripcode and capcode stay in the name, and are kept apart too.
        let named = by(487211260);
        assert_eq!((named.name.as_str(), named.trip.as_deref(), named.capcode.as_deref()), ("Kot !!Fz3mQwerty", Some("!!Fz3mQwerty"), None));
        let modpost = by(487211333);
        assert_eq!((modpost.name.as_str(), modpost.capcode.as_deref(), modpost.flag.as_ref()), ("Anonymous ## mod", Some("mod"), None));
        // /g/ has none of them.
        let g = Futaba::fourchan(None).parse_thread("g", &fixture("4chan_thread.json"));
        assert!(g.iter().all(|p| p.id.is_none() && p.flag.is_none() && p.trip.is_none()));

        // vichan: 8kun's IDs, and leftypol's custom flags in `country` (its files' `id`s
        // aren't posters').
        let cat = Futaba::vichan("https://8kun.top".into(), None, None, None).parse_catalog("v", &fixture("8kun_catalog.json"));
        assert_eq!(cat[0].id.as_deref(), Some("f8502e"));
        let posts = Futaba::vichan("https://leftypol.org".into(), None, None, None).parse_thread("leftypol", &fixture("leftypol_thread.json"));
        assert!(posts.iter().all(|p| p.id.is_none()));
        let p = posts.iter().find(|p| p.no == 2923530).unwrap();
        assert_eq!(p.flag, flag("naxalite", "Naxalite"));
        assert_eq!(p.flag.as_ref().unwrap().short(), "Naxalite");
        assert_eq!(by(487211102).flag.as_ref().unwrap().short(), "GB");
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
        let lain = Futaba::vichan("https://lainchan.org".into(), Some("png".into()), None, None);
        let p = lain.post("λ", &catalog_thread(&fixture("vichan_catalog.json"), 42742));
        assert_eq!(p.files[0].thumb.as_deref(), Some("https://lainchan.org/%CE%BB/thumb/1754702648060-0.png"));

        // wizchan keeps the file's extension, and uses jpg for videos.
        let wiz = Futaba::vichan("https://wizchan.org".into(), None, None, None);
        let cat = fixture("wizchan_catalog.json");
        let thumb = |no| wiz.post("wiz", &catalog_thread(&cat, no)).files.first().and_then(|f| f.thumb.clone());
        assert_eq!(thumb(230021).as_deref(), Some("https://wizchan.org/wiz/thumb/1790081744989.jpg"));
        assert_eq!(thumb(211629).as_deref(), Some("https://wizchan.org/wiz/thumb/1696663189546.jpg"));
        // Deleted files are dropped.
        assert_eq!(thumb(229036), None);
    }
}
