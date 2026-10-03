//! What a site runs, worked out from its APIs: adding a site from a link to one of its
//! pages. Each engine is asked once, through the rate limiter like any request.

use anyhow::{Result, bail};
use serde_json::Value;

use crate::config::{BoardConfig, SiteConfig, SiteKind};
use crate::http::{HttpError, encode_segment as enc, get_json, get_text};

/// What a link says about its site: the address (scheme and host), the host, and the board
/// the page is on, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub base: String,
    pub host: String,
    pub board: Option<String>,
}

/// A link to a page of a site (`https://somechan.org/b/res/1.html`, `somechan.org/b/`,
/// `somechan.org`), or `None` if it doesn't look like one.
pub fn link(input: &str) -> Option<Link> {
    let input = input.trim();
    let (scheme, rest) = match input.split_once("://") {
        Some((s @ ("http" | "https"), rest)) => (s, rest),
        Some(_) => return None,
        None => ("https", input.strip_prefix("//").unwrap_or(input)),
    };
    let host = rest.split(['/', '?', '#']).next()?;
    let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':');
    let labels: Vec<&str> = host.split(':').next()?.split('.').collect();
    // At least a name and a top-level domain of letters, nothing empty.
    let tld = labels.last().is_some_and(|t| t.len() >= 2 && t.chars().all(|c| c.is_ascii_alphabetic()));
    if labels.len() < 2 || !tld || labels.iter().any(|l| l.is_empty()) || !host.chars().all(ok) {
        return None;
    }
    let after = rest.get(host.len()..).unwrap_or_default();
    let (path, fragment) = after.split_once('#').unwrap_or((after, ""));
    let path = path.split('?').next().unwrap_or(path);
    // `/index.html` and the like are pages, not boards.
    let board = crate::route::parse_path(path, fragment).0.filter(|b| !b.contains('.'));
    let host = host.to_ascii_lowercase();
    Some(Link { base: format!("{scheme}://{host}"), host, board })
}

/// A name for a site on a host: `somechan` for `www.somechan.org`.
pub fn name_for(host: &str) -> String {
    let host = host.split(':').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    match host.rsplit_once('.') {
        Some((name, _)) if !name.is_empty() => name.to_string(),
        _ => host.to_string(),
    }
}

/// The boards in a vichan page's board bar (`<div class="boardlist">`), in its order: links
/// on this site to `/X/` or `/X/index.html` with a `title`, which is how vichan writes its
/// boards. Pages (`/rules.html`), the home link and overboards without a title are left out.
pub fn boardlist(html: &str, host: &str) -> Vec<BoardConfig> {
    let Some(start) = html.find(r#"class="boardlist""#) else { return Vec::new() };
    let bar = html.get(start..).unwrap_or_default();
    let bar = bar.get(..bar.find("</div>").unwrap_or(bar.len())).unwrap_or(bar);
    let bare = |h: &str| h.strip_prefix("www.").unwrap_or(h).to_ascii_lowercase();
    let mut out: Vec<BoardConfig> = Vec::new();
    for tag in bar.split("<a").skip(1).filter_map(|rest| rest.split_once('>').map(|(tag, _)| tag)).filter(|t| t.starts_with(char::is_whitespace)) {
        let (Some(href), Some(title)) = (attr(tag, "href"), attr(tag, "title").filter(|t| !t.trim().is_empty())) else { continue };
        // On this site: a path, or a link to this host.
        let path = match href.split_once("://").map(|(_, rest)| rest).or_else(|| href.strip_prefix("//")) {
            Some(rest) => match rest.split_once('/') {
                Some((h, p)) if bare(h) == bare(host) => format!("/{p}"),
                _ => continue,
            },
            None => href,
        };
        let parts: Vec<&str> = path.split('/').collect();
        let uri = match parts.as_slice() {
            ["", uri, ""] | ["", uri, "index.html"] => crate::route::decode(uri),
            _ => continue,
        };
        let seen = out.iter().any(|b| matches!(b, BoardConfig::Full { uri: u, .. } if *u == uri));
        if uri.is_empty() || uri.contains(['.', '?', '#']) || seen {
            continue;
        }
        out.push(BoardConfig::Full { uri, title: title.trim().to_string() });
    }
    out
}

/// An attribute's value in a tag (`name="value"`, entities decoded).
fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let mut from = 0;
    while let Some(i) = tag.get(from..)?.find(&key).map(|i| i + from) {
        // Not the end of another attribute's name (`data-title`).
        if tag.get(..i)?.ends_with(char::is_whitespace) {
            let value = tag.get(i + key.len()..)?;
            return Some(crate::markup::decode(value.get(..value.find('"')?)?));
        }
        from = i + key.len();
    }
    None
}

/// A 4chan-style catalog: pages of `threads` (vichan's `/{board}/catalog.json`).
fn is_catalog(v: &Value) -> bool {
    v.as_array().is_some_and(|pages| !pages.is_empty() && pages.iter().all(|p| p.get("threads").is_some_and(Value::is_array)))
}

/// Whether a vichan site has `board` (its catalog answers).
pub fn has_board(base: &str, board: &str) -> bool {
    get_json(&format!("{base}/{}/catalog.json", enc(board))).is_ok_and(|v| is_catalog(&v))
}

/// Ask the site's APIs, one engine after another, what it runs. A vichan site has no board
/// list to find, so it needs the link's board; that board is its list then.
pub fn detect(link: &Link) -> Result<SiteConfig> {
    let base = &link.base;
    let site = |kind, boards| SiteConfig {
        name: name_for(&link.host),
        kind,
        url: Some(base.clone()),
        boards,
        thumb_ext: None,
        archive: None,
        media_url: None,
    };
    // Why nothing matched: no answer at all, a refusal, or answers that weren't an API.
    let (mut unreachable, mut refused, mut answered) = (None, None, false);
    let mut get = |path: &str| match get_json(&format!("{base}{path}")) {
        Ok(v) => {
            answered = true;
            Some(v)
        }
        Err(e) => {
            match e.downcast_ref::<HttpError>() {
                Some(HttpError::Status(code @ (401 | 403 | 503), _)) => refused = Some(*code),
                Some(_) => answered = true,
                None if e.chain().any(|c| c.downcast_ref::<serde_json::Error>().is_some()) => answered = true,
                None => {
                    unreachable.get_or_insert(e);
                }
            }
            None
        }
    };
    // jschan has boards.json, and so do some vichan installs (4chan's format).
    if let Some(v) = get("/boards.json") {
        if !super::jschan::parse_boards(&v).0.is_empty() {
            return Ok(site(SiteKind::Jschan, None));
        }
        if !super::futaba::parse_boards(&v).is_empty() {
            return Ok(site(SiteKind::Vichan, None));
        }
    }
    if let Some(v) = get("/boards.js?json=1")
        && !super::lynxchan::parse_boards(&super::lynxchan::unwrap(v)).0.is_empty()
    {
        return Ok(site(SiteKind::Lynxchan, None));
    }
    if let Some(v) = get("/_/api/chan/archives/")
        && !super::foolfuuka::parse_archives(&v).is_empty()
    {
        return Ok(site(SiteKind::Foolfuuka, None));
    }
    // vichan has no board list API, but its pages have the boards in a bar at the top: the
    // link's board's page, or the front page for a link to the site.
    let page = |path: &str| get_text(&format!("{base}{path}")).map(|html| boardlist(&html, &link.host)).unwrap_or_default();
    let mut listed = Vec::new();
    let board = match &link.board {
        Some(b) => Some(b.clone()),
        None => {
            listed = page("/");
            listed.first().map(|b| b.uri().to_string())
        }
    };
    if let Some(board) = board
        && get(&format!("/{}/catalog.json", enc(&board))).is_some_and(|v| is_catalog(&v))
    {
        if link.board.is_some() {
            listed = page(&format!("/{}/index.html", enc(&board)));
        }
        // The link's board, whatever the bar says.
        if !listed.iter().any(|b| b.uri() == board) {
            listed.insert(0, BoardConfig::Uri(board));
        }
        return Ok(site(SiteKind::Vichan, Some(listed)));
    }
    if let Some(v) = get("/api/mobile/v2/boards")
        && !super::makaba::parse_boards(&v).is_empty()
    {
        return Ok(site(SiteKind::Makaba, None));
    }
    let host = &link.host;
    let hint = match &link.board {
        None => " A vichan site has no board list to find: try a link to one of its boards.",
        Some(_) => "",
    };
    match (answered, refused, unreachable) {
        (false, Some(code), _) => bail!("{host} refused ck's requests (HTTP {code}); some sites let only browsers in"),
        (false, None, Some(e)) => bail!("Couldn't reach {host}: {e:#}"),
        _ => bail!("{host} doesn't answer like jschan, LynxChan, FoolFuuka, vichan or makaba.{hint} README › Adding sites shows how to add one by hand."),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::backend::fixture;
    use crate::http::{Raw, serve_test_host};

    /// Serve `host` with `pages` (path and query → JSON), 404 for the rest.
    pub fn serve(host: &str, pages: Vec<(&'static str, Value)>) {
        serve_text(host, pages.into_iter().map(|(p, v)| (p, v.to_string())).collect());
    }

    /// Serve `host` with `pages` (path and query → body), 404 for the rest.
    pub fn serve_text(host: &str, pages: Vec<(&'static str, String)>) {
        let host_name = host.to_string();
        serve_test_host(
            host,
            Some(Arc::new(move |url: &str, _| {
                let path = url.strip_prefix(&format!("https://{host_name}")).unwrap_or(url);
                match pages.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => Raw { status: 200, last_modified: None, body: body.clone() },
                    None => Raw { status: 404, last_modified: None, body: String::new() },
                }
            })),
        );
    }

    #[test]
    fn links() {
        let l = |s: &str| link(s);
        let at = |base: &str, host: &str, board: Option<&str>| Some(Link { base: base.into(), host: host.into(), board: board.map(String::from) });
        assert_eq!(l("https://somechan.org/b/res/1.html#2"), at("https://somechan.org", "somechan.org", Some("b")));
        assert_eq!(l("somechan.org/tech/"), at("https://somechan.org", "somechan.org", Some("tech")));
        assert_eq!(l("http://SomeChan.org"), at("http://somechan.org", "somechan.org", None));
        assert_eq!(l("//x.example.net/index.html"), at("https://x.example.net", "x.example.net", None));
        assert_eq!(l("https://somechan.org:8080/λ/"), at("https://somechan.org:8080", "somechan.org:8080", Some("λ")));
        for not in ["g", "4chan/g", "123", "ftp://x.org", "x.o", "a..b.org", "x.org1", "some chan.org", "", ">>>/g/1"] {
            assert_eq!(l(not), None, "{not}");
        }
        assert_eq!(name_for("www.somechan.org"), "somechan");
        assert_eq!(name_for("boards.example.co:8080"), "boards.example");
    }

    /// Board bars as lainchan, wizchan and sushigirl write them (trimmed).
    const LAINCHAN: &str = r#"<div class="boardlist"><span class="sub" data-description="Notices">[ <a href="/donate.html">$$$</a> / <a href="/rules.html">rules</a> ]</span>  <span class="sub" data-description="STEM">[ <a href="/λ/index.html" title="Programming">λ</a> / <a href="/Δ/index.html" title="Do It Yourself">diy</a> / <a href="/%CE%A9/index.html" title="Tech &amp; stuff">tech</a> ]</span></div><div class="boardlist"><a href="/zzz/index.html" title="Not the top bar">zzz</a></div>"#;
    const WIZCHAN: &str = r#"<div class="boardlist"><span class="sub" data-description="0">[ <a href="/"><i class="fa fa-home"></i> Home</a> ]</span>  <span class="sub" data-description="1">[ <a href="/wiz/index.html" title="Wizardry">wiz</a> / <a href="/dep/index.html" title="Depression">dep</a> ]</span>  <span class="sub" data-description="2">[ <a href="/all/">all</a> ]</span>  <span class="sub" data-description="3">[ <a href="/rules.html"><i class="fa fa-book"></i>&nbsp;<b>Rules</b></a> ]</span></div>"#;
    const SUSHI: &str = r#"<div class="boardlist"><span class="sub" data-description="0">[ <a href="/kaitensushi">kaitensushi</a> ]</span>  <span class="sub" data-description="1">[ <a href="/lounge/index.html" title="sushi social">lounge</a> / <a href="https://sushigirl.cafe/arcade/" title="vidya">arcade</a> / <a href="https://elsewhere.example/x/" title="Another site">x</a> / <a data-title="no" href="/kawaii/index.html" title="cute things">kawaii</a> / <a href="/lounge/" title="again">lounge</a> ]</span></div>"#;

    #[test]
    fn board_bars() {
        let uris = |html, host| boardlist(html, host).iter().map(|b| (b.uri().to_string(), match b { BoardConfig::Full { title, .. } => title.clone(), BoardConfig::Uri(_) => String::new() })).collect::<Vec<_>>();
        let pairs = |v: &[(&str, &str)]| v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
        // The board's address, not its text; titles decoded; only the first bar.
        assert_eq!(uris(LAINCHAN, "lainchan.org"), pairs(&[("λ", "Programming"), ("Δ", "Do It Yourself"), ("Ω", "Tech & stuff")]));
        // No home link, pages or overboard without a title.
        assert_eq!(uris(WIZCHAN, "wizchan.org"), pairs(&[("wiz", "Wizardry"), ("dep", "Depression")]));
        // Full links to this host count, other hosts don't; each board once.
        assert_eq!(uris(SUSHI, "www.sushigirl.cafe"), pairs(&[("lounge", "sushi social"), ("arcade", "vidya"), ("kawaii", "cute things")]));
        assert!(boardlist("<html>no bar</html>", "x.org").is_empty());
        assert!(boardlist(r#"<div class="boardlist"><a href="/b/" title="unclosed"#, "x.org").is_empty());
    }

    #[test]
    fn vichan_boards_come_from_the_bar() {
        let catalog = fixture("vichan_catalog.json").to_string();
        serve_text(
            "wb.invalid",
            vec![("/", WIZCHAN.into()), ("/wiz/catalog.json", catalog.clone()), ("/dep/catalog.json", catalog.clone()), ("/dep/index.html", WIZCHAN.into()), ("/x/catalog.json", catalog), ("/x/index.html", WIZCHAN.into())],
        );
        let found = |board: Option<&str>| {
            let l = Link { base: "https://wb.invalid".into(), host: "wb.invalid".into(), board: board.map(String::from) };
            detect(&l).unwrap().boards.unwrap().iter().map(|b| b.uri().to_string()).collect::<Vec<_>>()
        };
        // A link to the site: the front page's bar (its first board checked).
        assert_eq!(found(None), ["wiz", "dep"]);
        // A link to a board: its page's bar, in the bar's order; a board not in it first.
        assert_eq!(found(Some("dep")), ["wiz", "dep"]);
        assert_eq!(found(Some("x")), ["x", "wiz", "dep"]);
        serve_test_host("wb.invalid", None);
    }

    #[test]
    fn engines() {
        let found = |host: &str, board: Option<&str>| {
            let l = Link { base: format!("https://{host}"), host: host.into(), board: board.map(String::from) };
            detect(&l)
        };
        serve("js.invalid", vec![("/boards.json", fixture("jschan_boards.json"))]);
        serve("v4.invalid", vec![("/boards.json", fixture("8kun_boards.json"))]);
        serve("lynx.invalid", vec![("/boards.js?json=1", fixture("lynxchan_boards_wrapped.json"))]);
        serve("ff.invalid", vec![("/_/api/chan/archives/", fixture("foolfuuka_archives.json"))]);
        serve("vi.invalid", vec![("/tech/catalog.json", fixture("vichan_catalog.json"))]);
        serve("mk.invalid", vec![("/api/mobile/v2/boards", fixture("makaba_boards.json"))]);
        serve("none.invalid", vec![("/boards.json", serde_json::json!({"hello": 1}))]);
        let kind = |host, board| found(host, board).map(|s| (s.kind, s.boards)).unwrap();
        assert_eq!(kind("js.invalid", None), (SiteKind::Jschan, None));
        assert_eq!(kind("v4.invalid", None), (SiteKind::Vichan, None));
        assert_eq!(kind("lynx.invalid", None), (SiteKind::Lynxchan, None));
        assert_eq!(kind("ff.invalid", Some("a")), (SiteKind::Foolfuuka, None));
        assert_eq!(kind("vi.invalid", Some("tech")), (SiteKind::Vichan, Some(vec![BoardConfig::Uri("tech".into())])));
        assert_eq!(kind("mk.invalid", None), (SiteKind::Makaba, None));
        let s = found("vi.invalid", Some("tech")).unwrap();
        assert_eq!((s.name.as_str(), s.url.as_deref()), ("vi", Some("https://vi.invalid")));
        // A vichan site without a board in the link: nothing to find; it says so.
        let e = found("vi.invalid", None).unwrap_err().to_string();
        assert!(e.contains("doesn't answer like") && e.contains("try a link to one of its boards"), "{e}");
        assert!(found("none.invalid", Some("b")).unwrap_err().to_string().contains("doesn't answer like"));
        // Nothing there at all, or a site that keeps programs out.
        assert!(found("nobody.invalid", None).unwrap_err().to_string().starts_with("Couldn't reach nobody.invalid"));
        serve_test_host("wall.invalid", Some(Arc::new(|_, _| Raw { status: 403, last_modified: None, body: "<html>".into() })));
        assert!(found("wall.invalid", None).unwrap_err().to_string().contains("refused ck's requests (HTTP 403)"));
        assert!(has_board("https://vi.invalid", "tech") && !has_board("https://vi.invalid", "b"));
    }
}

/// Hits the network: `cargo test -- --ignored live_detect --nocapture`. Built-in sites, whose
/// engines are known, found from links as if they weren't.
#[cfg(test)]
#[test]
#[ignore]
fn live_detect() {
    crate::http::NETWORK.store(true, std::sync::atomic::Ordering::Relaxed);
    let cases = [
        ("https://lainchan.org/%CE%BB/catalog.html", SiteKind::Vichan),
        ("wizchan.org", SiteKind::Vichan),
        ("https://zzzchan.xyz/tech/", SiteKind::Jschan),
        ("https://endchan.net/art/", SiteKind::Lynxchan),
        ("https://desuarchive.org/a/", SiteKind::Foolfuuka),
        ("https://8kun.top/pnd/", SiteKind::Vichan),
    ];
    let mut failures = Vec::new();
    for (url, kind) in cases {
        let got = link(url).ok_or_else(|| anyhow::anyhow!("not a link")).and_then(|l| detect(&l));
        eprintln!("{url}: {:?}", got.as_ref().map(|s| (s.kind, s.boards.as_ref().map(|b| b.iter().map(|b| b.uri().to_string()).collect::<Vec<_>>()))));
        match got {
            // vichan's boards, from the bar on its pages.
            Ok(s) if s.kind == SiteKind::Vichan && s.boards.as_ref().is_some_and(|b| b.len() < 4) => {
                failures.push(format!("{url}: only {:?}", s.boards));
            }
            Ok(s) if s.kind == kind => {}
            other => failures.push(format!("{url}: wanted {kind:?}, got {:?}", other.map(|s| s.kind))),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
