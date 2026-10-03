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
        // Not the end of another attribute's name (`data-title`); right after another
        // attribute's quote counts (8kun writes `class="a"src="b"`).
        if tag.get(..i)?.ends_with(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
            let value = tag.get(i + key.len()..)?;
            return Some(crate::markup::decode(value.get(..value.find('"')?)?));
        }
        from = i + key.len();
    }
    None
}

/// What a vichan page's thread previews say about where files are: thumbnails all in one
/// format whatever the file's (`thumb_ext`: `Some(Some(ext))`), or each in its file's
/// (`Some(None)`), or no telling (`None`); and the files' host, when it isn't the site's
/// (`media_url`).
pub fn files_from_page(html: &str, board: &str, host: &str) -> (Option<Option<String>>, Option<String>) {
    let bare = |h: &str| h.strip_prefix("www.").unwrap_or(h).to_ascii_lowercase();
    // A file or thumbnail link: what comes before `/{board}/src|thumb/` (or vichan's flat
    // `/file_store/`), and the file's name and extension.
    let split = |url: &str, dir: &str| -> Option<(String, String, String)> {
        let url = crate::route::decode(url);
        // Flat files are right in `/file_store/`, their thumbnails in `/file_store/thumb/`.
        let flat = if dir == "src" { "/file_store/".to_string() } else { format!("/file_store/{dir}/") };
        let (prefix, name) = [format!("/{board}/{dir}/"), flat].iter().find_map(|m| {
            let at = url.rfind(m.as_str())?;
            Some((url.get(..at)?.to_string(), url.get(at + m.len()..)?.to_string()))
        })?;
        if name.contains('/') {
            return None;
        }
        let (stem, ext) = name.split(['?', '#']).next()?.rsplit_once('.')?;
        Some((prefix, stem.to_string(), ext.to_ascii_lowercase()))
    };
    let attrs = |tag: &str, attr: &str| -> Vec<String> {
        html.split(tag).skip(1).filter_map(|t| t.split_once('>').map(|(t, _)| t)).filter_map(|t| self::attr(t, attr)).collect()
    };
    let thumbs: Vec<(String, String, String)> = attrs("<img", "src").iter().filter_map(|u| split(u, "thumb")).collect();
    let files: Vec<(String, String, String)> = attrs("<a", "href").iter().filter_map(|u| split(u, "src")).collect();
    // The host the thumbnails are on, when it isn't the site's.
    let media = thumbs.iter().find_map(|(prefix, ..)| {
        let rest = prefix.split_once("://").map(|(_, r)| r).or_else(|| prefix.strip_prefix("//"))?;
        let (h, _) = rest.split_once('/').unwrap_or((rest, ""));
        (bare(h) != bare(host)).then(|| if prefix.starts_with("//") { format!("https:{prefix}") } else { prefix.clone() })
    });
    let image = |e: &str| matches!(e, "jpg" | "jpeg" | "png" | "gif" | "webp");
    let pairs: Vec<(&str, &str)> = thumbs
        .iter()
        .filter_map(|(_, stem, thumb)| Some((files.iter().find(|(_, s, _)| s == stem).map(|(.., e)| e.as_str()).filter(|e| image(e))?, thumb.as_str())))
        .collect();
    let thumb_ext = match pairs.first() {
        None => None,
        Some(&(_, first)) if pairs.iter().all(|&(_, t)| t == first) && pairs.iter().any(|&(f, _)| f != first) => Some(Some(first.to_string())),
        // Each in its file's format (now and then not: a GIF's thumbnail may be a PNG).
        Some(_) => Some(None),
    };
    (thumb_ext, media)
}

/// With no previews to go by: one of the catalog's files, its thumbnail asked for in the
/// file's format, then png, then jpg (at most three small requests); the first that's there
/// says.
fn probe_thumb_ext(media: &str, board: &str, catalog: &Value) -> Option<Option<String>> {
    let ext = |t: &Value| t["ext"].as_str().map(str::to_ascii_lowercase);
    let has_tim = |t: &&Value| t["tim"].is_string() || t["tim"].is_u64();
    let threads: Vec<&Value> = catalog.as_array()?.iter().flat_map(|p| p["threads"].as_array().into_iter().flatten()).filter(has_tim).collect();
    // A PNG, GIF or WebP says most: its thumbnail is in its format or a fixed one (a JPEG's is
    // a JPEG either way). The newest, which is how the site makes them now: `tim`s are times,
    // sometimes after a letter or two.
    let when = |t: &Value| t["tim"].as_u64().or_else(|| t["tim"].as_str()?.trim_start_matches(|c: char| !c.is_ascii_digit()).parse().ok()).unwrap_or(0);
    let newest = |exts: &[&str]| threads.iter().filter(|t| ext(t).is_some_and(|e| exts.contains(&e.as_str()))).max_by_key(|t| when(t));
    let post = newest(&[".png", ".gif", ".webp"]).or_else(|| newest(&[".jpg", ".jpeg"]))?;
    let tim = post["tim"].as_str().map(String::from).or_else(|| post["tim"].as_u64().map(|t| t.to_string()))?;
    let own = post["ext"].as_str()?.trim_start_matches('.').to_ascii_lowercase();
    let dir = if post["fpath"].as_u64() == Some(1) { "file_store/thumb".to_string() } else { format!("{}/thumb", enc(board)) };
    let mut tries = vec![own.clone()];
    tries.extend(["png", "jpg"].iter().map(|e| e.to_string()).filter(|e| *e != own));
    tries.into_iter().find(|e| crate::http::exists(&format!("{media}/{dir}/{tim}.{e}"))).map(|e| (e != own).then_some(e))
}

/// Where a vichan board's files are: from its page's thread previews (`html`, else fetched),
/// else one thumbnail asked for.
fn vichan_files(base: &str, host: &str, board: &str, html: Option<String>) -> (Option<String>, Option<String>) {
    let html = html.unwrap_or_else(|| get_text(&format!("{base}/{}/index.html", enc(board))).unwrap_or_default());
    let (thumb_ext, media) = files_from_page(&html, board, host);
    let thumb_ext = thumb_ext.or_else(|| {
        // The catalog was just fetched: this is the cache's copy.
        let catalog = get_json(&format!("{base}/{}/catalog.json", enc(board))).ok()?;
        probe_thumb_ext(media.as_deref().unwrap_or(base), board, &catalog)
    });
    (thumb_ext.flatten(), media)
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
        let listed = super::futaba::parse_boards(&v);
        if !listed.is_empty()
            && let Some(board) = link.board.clone().or_else(|| listed.first().map(|b| b.uri.clone()))
        {
            let (thumb_ext, media_url) = vichan_files(base, &link.host, &board, None);
            return Ok(SiteConfig { thumb_ext, media_url, ..site(SiteKind::Vichan, None) });
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
    let page = |path: &str| get_text(&format!("{base}{path}")).unwrap_or_default();
    let mut html = String::new();
    let board = match &link.board {
        Some(b) => Some(b.clone()),
        None => {
            html = page("/");
            boardlist(&html, &link.host).first().map(|b| b.uri().to_string())
        }
    };
    if let Some(board) = board
        && get(&format!("/{}/catalog.json", enc(&board))).is_some_and(|v| is_catalog(&v))
    {
        if link.board.is_some() {
            html = page(&format!("/{}/index.html", enc(&board)));
        }
        let mut listed = boardlist(&html, &link.host);
        // The link's board, whatever the bar says.
        if !listed.iter().any(|b| b.uri() == board) {
            listed.insert(0, BoardConfig::Uri(board.clone()));
        }
        // Where files are, from the thread previews on the page, else one thumbnail asked for.
        let (thumb_ext, media_url) = vichan_files(base, &link.host, &board, Some(html));
        return Ok(SiteConfig { thumb_ext, media_url, ..site(SiteKind::Vichan, Some(listed)) });
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
        ("https://kissu.moe/qa/", SiteKind::Vichan),
        ("https://tvch.moe/tv/", SiteKind::Vichan),
        ("https://sushigirl.cafe/lounge/", SiteKind::Vichan),
        ("https://uboachan.net/yn/", SiteKind::Vichan),
        ("https://zzzchan.xyz/tech/", SiteKind::Jschan),
        ("https://endchan.net/art/", SiteKind::Lynxchan),
        ("https://desuarchive.org/a/", SiteKind::Foolfuuka),
        ("https://8kun.top/pnd/", SiteKind::Vichan),
    ];
    let mut failures = Vec::new();
    for (url, kind) in cases {
        let got = link(url).ok_or_else(|| anyhow::anyhow!("not a link")).and_then(|l| detect(&l));
        eprintln!("{url}: {:?}", got.as_ref().map(|s| (s.kind, s.boards.as_ref().map(|b| b.iter().map(|b| b.uri().to_string()).collect::<Vec<_>>()))));
        // Where files are: what the built-in site of that host says.
        if let Ok(s) = &got
            && let Some(b) = crate::config::builtin_sites().iter().find(|b| b.url.as_deref().is_some_and(|u| crate::http::host(u) == crate::http::host(s.url.as_deref().unwrap_or_default())))
            && (s.thumb_ext != b.thumb_ext || s.media_url != b.media_url)
        {
            failures.push(format!("{url}: thumb_ext {:?} media_url {:?}, built in: {:?} {:?}", s.thumb_ext, s.media_url, b.thumb_ext, b.media_url));
        }
        match got {
            // vichan's boards, from the bar on its pages (kissu's pages are drawn in the
            // browser: no bar, so just the link's board).
            Ok(s) if s.kind == SiteKind::Vichan && !url.contains("kissu") && s.boards.as_ref().is_some_and(|b| b.len() < 4) => {
                failures.push(format!("{url}: only {:?}", s.boards));
            }
            Ok(s) if s.kind == kind => {}
            other => failures.push(format!("{url}: wanted {kind:?}, got {:?}", other.map(|s| s.kind))),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[cfg(test)]
mod files_tests {
    use super::*;

    /// Thread previews as lainchan writes them (trimmed): files on the site, thumbnails PNG.
    const LAINCHAN: &str = r#"<div class="file"><p class="fileinfo">File: <a href="/λ/src/1754702648060-0.jpg">1754702648060-0.jpg</a> <span class="details"><a href="http://imgops.com/lainchan.org/λ/src/1754702648060-0.jpg">ImgOps</a></span></p><a href="/λ/src/1754702648060-0.jpg" target="_blank"><img class="post-image" src="/λ/thumb/1754702648060-0.png" alt="" /></a></div><a href="/%CE%BB/src/170.gif"><img class="post-image" src="/%CE%BB/thumb/170.png"></a><img src="/static/spoiler.png">"#;
    /// 8kun: files and thumbnails on another host, each thumbnail in its file's format.
    const EIGHTKUN: &str = r#"<a href="/file_store/4c65.jpeg"><img class="pnd_new sjj post-image"src="https://nerv.8kun.top/file_store/thumb/4c65.jpeg"></a><a href="https://nerv.8kun.top/file_store/a411.png"><img class="post-image" src="https://nerv.8kun.top/file_store/thumb/a411.png"></a>"#;

    #[test]
    fn from_thread_previews() {
        assert_eq!(files_from_page(LAINCHAN, "λ", "lainchan.org"), (Some(Some("png".into())), None));
        assert_eq!(files_from_page(EIGHTKUN, "pnd", "8kun.top"), (Some(None), Some("https://nerv.8kun.top".into())));
        // Thumbnails in their files' formats, on the site.
        let own = r#"<a href="/b/src/1.jpg"><img src="/b/thumb/1.jpg"></a><a href="/b/src/2.png"><img src="/b/thumb/2.png"></a>"#;
        assert_eq!(files_from_page(own, "b", "x.org"), (Some(None), None));
        // A media host under a path, given without a scheme; the site's own host isn't one.
        let pathed = r#"<a href="//cdn.x.org/m/b/src/1.jpg"><img src="//cdn.x.org/m/b/thumb/1.png"></a>"#;
        assert_eq!(files_from_page(pathed, "b", "x.org"), (Some(Some("png".into())), Some("https://cdn.x.org/m".into())));
        let own_host = r#"<a href="https://www.x.org/b/src/1.jpg"><img src="https://www.x.org/b/thumb/1.png"></a>"#;
        assert_eq!(files_from_page(own_host, "b", "x.org").1, None);
        // Nothing to go by: no previews, or only a spoiler and videos.
        assert_eq!(files_from_page("<html></html>", "b", "x.org"), (None, None));
        let videos = r#"<a href="/b/src/1.webm"><img src="/b/thumb/1.jpg"></a><img src="/static/spoiler.png">"#;
        assert_eq!(files_from_page(videos, "b", "x.org"), (None, None));
    }

    #[test]
    fn probing_one_thumbnail() {
        use crate::backend::detect::tests::serve_text;
        let catalog = serde_json::json!([{ "threads": [{ "no": 1, "tim": 1700, "ext": ".webm" }, { "no": 2, "tim": 1701, "ext": ".jpg" }] }]);
        // The newest PNG, GIF or WebP is the one asked about.
        let mixed = serde_json::json!([{ "threads": [{ "no": 1, "tim": "m1600", "ext": ".png" }, { "no": 2, "tim": "mm1800", "ext": ".gif" }, { "no": 3, "tim": 1900, "ext": ".jpg" }] }]);
        serve_text("newest.invalid", vec![("/b/thumb/1600.png", "png".into()), ("/b/thumb/mm1800.jpg", "jpg".into())]);
        assert_eq!(probe_thumb_ext("https://newest.invalid", "b", &mixed), Some(Some("jpg".into())));
        crate::http::serve_test_host("newest.invalid", None);
        serve_text("pj.invalid", vec![("/b/thumb/1701.png", "png".into())]);
        assert_eq!(probe_thumb_ext("https://pj.invalid", "b", &catalog), Some(Some("png".into())));
        serve_text("own.invalid", vec![("/b/thumb/1701.jpg", "jpg".into())]);
        assert_eq!(probe_thumb_ext("https://own.invalid", "b", &catalog), Some(None));
        serve_text("none.invalid", vec![]);
        assert_eq!(probe_thumb_ext("https://none.invalid", "b", &catalog), None);
        let flat = serde_json::json!([{ "threads": [{ "no": 2, "tim": "abc", "ext": ".png", "fpath": 1 }] }]);
        serve_text("flat.invalid", vec![("/file_store/thumb/abc.png", "png".into())]);
        assert_eq!(probe_thumb_ext("https://flat.invalid", "b", &flat), Some(None));
        for h in ["pj.invalid", "own.invalid", "none.invalid", "flat.invalid"] {
            crate::http::serve_test_host(h, None);
        }
    }
}
