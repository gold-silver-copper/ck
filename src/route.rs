//! Where a URL or a short form like `4chan/g/123#456` leads: a site, and a board, thread
//! and post on it.

use anyhow::{Result, bail};

use crate::config::{SiteConfig, SiteKind};
use crate::http;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub site: usize,
    /// `None`: the site's board list.
    pub board: Option<String>,
    pub thread: Option<u64>,
    pub post: Option<u64>,
}

/// A site's name and the hosts its pages are on.
pub struct SiteInfo {
    pub name: String,
    pub hosts: Vec<String>,
}

impl SiteInfo {
    /// `board_url` is any page URL of the site (its hosts are derived from it).
    pub fn new(cfg: &SiteConfig, board_url: &str) -> Self {
        let mut hosts = vec![bare(http::host(board_url)).to_string()];
        let extra: &[&str] = match cfg.kind {
            SiteKind::Fourchan => &["boards.4chan.org", "boards.4channel.org", "4chan.org", "4channel.org"],
            SiteKind::Makaba => &["2ch.hk", "2ch.su", "2ch.life"],
            _ => &[],
        };
        hosts.extend(extra.iter().map(|h| h.to_string()));
        if let Some(m) = &cfg.media_url {
            hosts.push(bare(http::host(m)).to_string());
        }
        Self { name: cfg.name.clone(), hosts }
    }
}

fn bare(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

/// The forms `resolve` accepts, for error messages.
pub const FORMS: &str = "a URL, g, /g/, 123, 4chan/g, 4chan/g/123, lainchan/λ/42#43 or >>>/g/123";

/// Resolve what was typed (or pasted) after `:`. `here` is the current site and board,
/// for short forms without them.
pub fn resolve(input: &str, sites: &[SiteInfo], here: (usize, Option<&str>)) -> Result<Target> {
    let input = input.trim();
    if input.is_empty() {
        bail!("Type {FORMS}");
    }
    if let Some(rest) = input.split_once("://").map(|(_, r)| r).or_else(|| input.strip_prefix("//")).or_else(|| {
        // `boards.4chan.org/g/` without a scheme: the first part looks like a host.
        let first = input.split('/').next()?;
        (first.contains('.') && input.contains('/') && sites.iter().any(|s| s.hosts.iter().any(|h| *h == bare(first)))).then_some(input)
    }) {
        let host = bare(rest.split(['/', '?', '#']).next().unwrap_or(rest));
        let Some(site) = sites.iter().position(|s| s.hosts.iter().any(|h| h == host)) else {
            bail!("No site in the config is on {host}");
        };
        let after = &rest[rest.find('/').unwrap_or(rest.len())..];
        let (path, fragment) = after.split_once('#').unwrap_or((after, ""));
        let path = path.split('?').next().unwrap_or(path);
        let (board, thread, post) = parse_path(path, fragment);
        return Ok(Target { site, board, thread, post });
    }
    // Short forms. `>>>/g/123` is a cross-board quote.
    let input = input.strip_prefix(">>>").unwrap_or(input);
    let (path, fragment) = input.split_once('#').unwrap_or((input, ""));
    let post = number(fragment.trim_start_matches(['p', 'q']));
    let mut parts: Vec<String> = path.split('/').filter(|p| !p.is_empty()).map(decode).collect();
    let mut site = here.0;
    if let Some(i) = parts.first().and_then(|first| sites.iter().position(|s| s.name.eq_ignore_ascii_case(first))) {
        site = i;
        parts.remove(0);
    } else if parts.len() == 1 && let Some(n) = number(&parts[0]) {
        // A bare number: a thread on the current board.
        let Some(board) = here.1 else { bail!("Open a board first, or type BOARD/{n}") };
        return Ok(Target { site, board: Some(board.to_string()), thread: Some(n), post });
    }
    let target = match parts.as_slice() {
        [] => Target { site, board: None, thread: None, post: None },
        [board] => Target { site, board: Some(board.clone()), thread: None, post: None },
        [board, n] => match number(n) {
            Some(n) => Target { site, board: Some(board.clone()), thread: Some(n), post },
            None => bail!("`{n}` isn't a thread number; type {FORMS}"),
        },
        _ => bail!("Couldn't read `{input}`; type {FORMS}"),
    };
    Ok(target)
}

/// Board, thread and post from a page path and fragment, in the URL styles of every engine:
/// `/g/thread/123/slug#p456` (4chan), `/g/res/123.html#456` and `/g/res/123+50.html`
/// (vichan, LynxChan, makaba), `/g/thread/123.html#456` (jschan), `/g/thread/123/#456` and
/// `/g/post/456/` (FoolFuuka), `/g/`, `/g/catalog#s=lmg`.
pub fn parse_path(path: &str, fragment: &str) -> (Option<String>, Option<u64>, Option<u64>) {
    let parts: Vec<String> = path.split('/').filter(|p| !p.is_empty()).map(decode).collect();
    let board = parts.first().filter(|b| *b != "_").cloned();
    let mut thread = None;
    let mut post = number(fragment.trim_start_matches(['p', 'q']));
    for w in parts.windows(2) {
        match w[0].as_str() {
            "thread" | "res" if thread.is_none() => thread = number(&w[1]),
            "post" => post = number(&w[1]),
            _ => {}
        }
    }
    (board, thread, post)
}

/// The number at the start of `s` (`123`, `123.html`, `123+50.html`, `123-slug`).
fn number(s: &str) -> Option<u64> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s[..end].parse().ok()
}

/// Undo percent-encoding (`%CE%BB` is λ).
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend;
    use crate::config::Config;

    fn sites() -> (Config, Vec<SiteInfo>) {
        let cfg: Config = toml::from_str(crate::config::DEFAULT_CONFIG).unwrap();
        let infos = cfg.sites.iter().map(|s| SiteInfo::new(s, &backend::build(s).board_url("x"))).collect();
        (cfg, infos)
    }

    fn t(site: usize, board: &str, thread: Option<u64>, post: Option<u64>) -> Target {
        Target { site, board: Some(board.into()), thread, post }
    }

    #[test]
    fn every_engine_round_trips() {
        let (cfg, infos) = sites();
        for (i, s) in cfg.sites.iter().enumerate() {
            let b = backend::build(s);
            let at = |url: String| resolve(&url, &infos, (0, None)).unwrap_or_else(|e| panic!("{}: {url}: {e}", s.name));
            assert_eq!(at(b.thread_url("tech", 123)), t(i, "tech", Some(123), None), "{}", s.name);
            assert_eq!(at(b.post_url("tech", 123, 456)), t(i, "tech", Some(123), Some(456)), "{}", s.name);
            assert_eq!(at(b.board_url("tech")), t(i, "tech", None, None), "{}", s.name);
            // Non-ASCII boards (lainchan's λ) survive percent-encoding.
            assert_eq!(at(b.thread_url("λ", 7)), t(i, "λ", Some(7), None), "{}", s.name);
        }
    }

    #[test]
    fn real_urls() {
        let (cfg, infos) = sites();
        let site = |name: &str| cfg.sites.iter().position(|s| s.name == name).unwrap();
        let at = |url: &str| resolve(url, &infos, (0, None)).unwrap();
        let fourchan = site("4chan");
        assert_eq!(at("https://boards.4chan.org/g/thread/109953009/lmg-local-models#p109953010"), t(fourchan, "g", Some(109953009), Some(109953010)));
        assert_eq!(at("https://boards.4channel.org/v/thread/1#q2"), t(fourchan, "v", Some(1), Some(2)));
        assert_eq!(at("boards.4chan.org/g/catalog#s=lmg%2F"), t(fourchan, "g", None, None));
        assert_eq!(at("https://lainchan.org/%CE%BB/res/42.html#43"), t(site("lainchan"), "λ", Some(42), Some(43)));
        assert_eq!(at("https://8kun.top/pnd/res/12345+50.html#12399"), t(site("8kun"), "pnd", Some(12345), Some(12399)));
        assert_eq!(at("https://2ch.hk/b/res/3000.html#3001"), t(site("2ch"), "b", Some(3000), Some(3001)));
        assert_eq!(at("https://zzzchan.xyz/tech/thread/5.html#9"), t(site("zzzchan"), "tech", Some(5), Some(9)));
        assert_eq!(at("https://endchan.net/art/res/8.html"), t(site("endchan"), "art", Some(8), None));
        assert_eq!(at("https://desuarchive.org/a/thread/1000/#1002"), t(site("desuarchive"), "a", Some(1000), Some(1002)));
        assert_eq!(at("https://desuarchive.org/a/post/1002/"), t(site("desuarchive"), "a", None, Some(1002)));
        assert!(resolve("https://example.com/g/thread/1", &infos, (0, None)).unwrap_err().to_string().contains("example.com"));
    }

    #[test]
    fn short_forms() {
        let (cfg, infos) = sites();
        let lain = cfg.sites.iter().position(|s| s.name == "lainchan").unwrap();
        let at = |s: &str| resolve(s, &infos, (0, Some("g"))).unwrap();
        assert_eq!(at("g"), t(0, "g", None, None));
        assert_eq!(at("/v/"), t(0, "v", None, None));
        assert_eq!(at("123"), t(0, "g", Some(123), None));
        assert_eq!(at("4chan/g"), t(0, "g", None, None));
        assert_eq!(at("4chan/g/123"), t(0, "g", Some(123), None));
        assert_eq!(at("lainchan/λ/42#43"), t(lain, "λ", Some(42), Some(43)));
        assert_eq!(at("LAINCHAN"), Target { site: lain, board: None, thread: None, post: None });
        assert_eq!(at(">>>/g/123"), t(0, "g", Some(123), None));
        assert!(resolve("g/abc", &infos, (0, None)).is_err());
        assert!(resolve("123", &infos, (0, None)).unwrap_err().to_string().contains("Open a board first"));
        assert!(resolve("", &infos, (0, None)).is_err());
    }
}
