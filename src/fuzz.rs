//! Fuzzing without dependencies: a seeded generator, a runner that prints how to replay a
//! failure, and targets for the parsers and the request etiquette. The app itself is fuzzed
//! in `app::fuzz`.
//!
//! Each target has a short run in `cargo test` and a long one that's ignored:
//! `cargo test --release -- --ignored fuzz --nocapture`, with `FUZZ_SEED` and `FUZZ_RUNS`
//! to choose. A failure prints the seed that replays it on its own.

use std::cell::Cell;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ratatui::text::Line;
use serde_json::json;

use crate::http::{self, Cache, Limiter, MIN_REFETCH, Priority, Raw};
use crate::markup::{self, Flavor};
use crate::route;

/// splitmix64: small, fast, and good enough to drive a fuzzer.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }

    /// True `percent` times in a hundred.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Run `body` once per seed: `runs` seeds from `seed` (or `FUZZ_SEED` / `FUZZ_RUNS` when
/// `long`). A panic is reported with the seed that replays it alone, then re-raised.
pub fn run(name: &str, long: bool, seed: u64, runs: u64, mut body: impl FnMut(u64)) {
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    let (seed, runs) = if long {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(seed, |d| d.as_nanos() as u64);
        (env("FUZZ_SEED").unwrap_or(now), env("FUZZ_RUNS").unwrap_or(runs))
    } else {
        (seed, runs)
    };
    if long {
        eprintln!("{name}: seed {seed}, {runs} runs");
    }
    let start = Instant::now();
    for i in 0..runs {
        // The first run uses the seed itself, so a reported seed replays with FUZZ_RUNS=1.
        let s = if i == 0 { seed } else { Rng::new(seed ^ i.wrapping_mul(0xa076_1d64_78bd_642f)).next() };
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| body(s))) {
            eprintln!("\n{name} failed on run {i}. Replay it with:");
            eprintln!("  FUZZ_SEED={s} FUZZ_RUNS=1 cargo test {name}_long -- --ignored --nocapture\n");
            resume_unwind(panic);
        }
    }
    if long {
        eprintln!("{name}: {runs} runs in {:.1?}", start.elapsed());
    }
}

/// Cells a line takes on screen: ratatui skips control characters when drawing.
fn drawn_width(line: &Line) -> usize {
    use unicode_width::UnicodeWidthStr;
    line.spans.iter().map(|s| s.content.chars().filter(|c| !c.is_control()).collect::<String>().width()).sum()
}

/// The text of a line, spans joined.
fn text(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

// ----- markup: HTML in, wrapped and highlighted lines out -----

const HTML: &[&str] = &[
    "<br>", "<br/>", "<p>", "</p>", "<wbr>", "<b>", "</b>", "<i>", "<em>", "<strong>", "<u>", "<s>", "</s>",
    "<span class=\"quote\">&gt;", "<span class=\"spoiler\">", "<span class=\"heading\">", "<span class=\"redtext\">",
    "<span class=\"unkfunc\">", "</span>", "<pre>", "</pre>", "<code>", "</code>", "<div class=\"code\">", "</div>",
    "<a href=\"#p{n}\" class=\"quotelink\">&gt;&gt;{n}</a>", "<a href=\"/g/thread/{n}#p{m}\" class=\"quotelink\">&gt;&gt;{m}</a>",
    "<a href=\"/b/res/{n}.html#{m}\">&gt;&gt;&gt;/b/{m}</a>", "<a href=\"https://example.com/{n}?q=a&amp;b\">", "</a>",
    "<a href=\"/g/\">&gt;&gt;&gt;/g/</a>", "<a>", "<a href=\"", "<a href=\"javascript:x\">", "<small>(OP)</small>",
    "&gt;&gt;{n}", "&gt;&gt;&gt;/g/{n}", ">>{n}", ">>>/b/", "&amp;", "&lt;", "&#39;", "&#x1F600;", "&bogus;", "&",
    "https://example.com/{n}", "http://x.y/(a)", "www.example.org", "\n", "\r\n", "  ", " ", "\t", "word", "words and more",
    "supercalifragilisticexpialidocious_without_any_space_to_break_at", "日本語のテキスト", "e\u{301}", "👍🏽", "🇯🇵",
    "\u{200b}", "\u{0}", "\u{1b}[31m", "\u{7f}", "<", ">", "\"", "'", "<!-- a comment -->", "<script>x()</script>",
    "<img src=\"x.png\">", "<span", "</", "<<", ">>", "#", "%", "(OP)", " (OP)", " (You)", "*", "**bold**", "==red==",
    "''", "[spoiler]", "[/spoiler]", "```", "→", "\u{feff}", "\u{2028}",
];

/// Markup from fragments that sites send (and some they shouldn't).
pub fn html(rng: &mut Rng, pieces: usize) -> String {
    let mut out = String::new();
    for _ in 0..pieces {
        let piece = rng.pick(HTML).replace("{n}", &rng.below(2000).to_string()).replace("{m}", &rng.below(2000).to_string());
        out.push_str(&piece);
        if rng.chance(5) {
            out.push(char::from_u32(rng.below(0x3000) as u32).unwrap_or('?'));
        }
    }
    out
}

const FLAVORS: [Flavor; 5] = [Flavor::Fourchan, Flavor::Vichan, Flavor::Lynxchan, Flavor::Jschan, Flavor::Makaba];

fn markup_once(seed: u64) {
    let mut rng = Rng::new(seed);
    let pieces = rng.below(60);
    let src = html(&mut rng, pieces);
    let flavor = *rng.pick(&FLAVORS);
    let parsed = if rng.chance(15) { markup::parse_plain(&src) } else { markup::parse_html(&src, flavor) };
    let ctx = || format!("seed {seed}, {flavor:?}: {src:?}");
    for link in &parsed.links {
        assert!(link.post.is_some() || link.thread.is_some() || link.board.is_some(), "an empty link: {link:?} in {}", ctx());
    }
    let needle: String = if rng.chance(50) { String::new() } else { rng.pick(&["a", "OP", " ", "日", "1", "é", "(you)", ">>"]).to_string() };
    for line in &parsed.lines {
        for width in [0, 1, 2, 3, 7, 20, 79, 200, rng.below(120)] {
            let wrapped = markup::wrap(line, width);
            for w in &wrapped {
                // `wrap` treats widths under 2 as 2, so a wide character always fits.
                // Only a character wider than the whole width may stick out, alone.
                let drawn = drawn_width(w);
                let alone = text(w).trim_start_matches('↪').chars().count() == 1;
                assert!(drawn <= width.max(2) || alone, "a {drawn}-wide line at width {width}: {:?} ({})", text(w), ctx());
            }
            if line.style != markup::CODE_LINE {
                let kept = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
                let joined: String = wrapped.iter().map(text).collect();
                assert_eq!(kept(&joined), kept(&text(line)), "wrapping lost or added text ({})", ctx());
            }
        }
        let lit = markup::highlight(line, &needle, ratatui::style::Style::new());
        assert_eq!(text(&lit), text(line), "highlighting changed the text ({})", ctx());
        assert_eq!(text(&markup::reveal(line)), text(line), "revealing changed the text ({})", ctx());
    }
}

#[test]
fn fuzz_markup() {
    run("fuzz_markup", false, 1, 2_000, markup_once);
}

#[test]
#[ignore]
fn fuzz_markup_long() {
    run("fuzz_markup", true, 0, 200_000, markup_once);
}

// ----- routes: what's typed after `:` or pasted -----

const ROUTE: &[&str] = &[
    "https://", "http://", "boards.4chan.org", "4chan.org", "4chan", "lainchan.org", "lainchan", "desuarchive.org", "2ch.hk",
    "8kun.top", "endchan.net", "/", "//", "g", "λ", "%CE%BB", "b", "thread", "res", "123", "45678", ".html", ".json", "#p",
    "#q", "#", "999", "?", "&", "%2F", "%", "%zz", ":", "..", " ", "www.", "_", "last", "-", "+", "\u{0}", "日本", "@",
];

fn route_once(seed: u64, sites: &[route::SiteInfo]) {
    let mut rng = Rng::new(seed);
    let input: String = (0..rng.below(12)).map(|_| *rng.pick(ROUTE)).collect();
    let here = (rng.below(sites.len()), rng.chance(50).then_some("g"));
    if let Ok(t) = route::resolve(&input, sites, here) {
        assert!(t.site < sites.len(), "site {} of {} for {input:?}", t.site, sites.len());
        assert!(t.board.as_ref().is_none_or(|b| !b.is_empty()), "an empty board for {input:?}");
    }
    let (path, fragment) = input.split_once('#').unwrap_or((&input, ""));
    let _ = route::parse_path(path, fragment);
}

fn default_sites() -> Vec<route::SiteInfo> {
    let cfg: crate::config::Config = toml::from_str(crate::config::DEFAULT_CONFIG).unwrap();
    cfg.sites.iter().map(|s| route::SiteInfo::new(s, &crate::backend::build(s).board_url("x"))).collect()
}

#[test]
fn fuzz_routes() {
    let sites = default_sites();
    run("fuzz_routes", false, 1, 5_000, |s| route_once(s, &sites));
}

#[test]
#[ignore]
fn fuzz_routes_long() {
    let sites = default_sites();
    run("fuzz_routes", true, 0, 1_000_000, |s| route_once(s, &sites));
}

// ----- the rate limiter: one request per interval per host, whoever asks -----

fn limiter_once(seed: u64) {
    let mut rng = Rng::new(seed);
    let limiter = Limiter::default();
    let base = Instant::now();
    let mut now = Duration::ZERO;
    let hosts = [("api.example", Duration::from_secs(1)), ("media.example", Duration::from_millis(250))];
    let mut last: HashMap<&str, Duration> = HashMap::new();
    for step in 0..rng.below(400) {
        now += Duration::from_millis(rng.below(1500) as u64);
        let (host, interval) = *rng.pick(&hosts);
        let prio = *rng.pick(&[Priority::User, Priority::Low, Priority::Background]);
        let waiting = Duration::from_millis(rng.below(20_000) as u64);
        let idle = Duration::from_millis(rng.below(3_000) as u64);
        let Some(wait) = limiter.admit(host, interval, base + now, prio, waiting, idle) else {
            // Only low-priority requests are ever told to check again.
            assert_ne!(prio, Priority::User, "a user request refused (seed {seed}, step {step})");
            continue;
        };
        if prio == Priority::Background && waiting < Duration::from_secs(15) {
            assert!(idle >= Duration::from_millis(1500), "a background request while the user is busy (seed {seed}, step {step})");
        }
        let start = now + wait;
        if let Some(&prev) = last.get(host) {
            assert!(start >= prev + interval, "{host}: a request {:?} after the last (seed {seed}, step {step})", start.saturating_sub(prev));
        }
        last.insert(host, start);
    }
}

#[test]
fn fuzz_limiter() {
    run("fuzz_limiter", false, 1, 300, limiter_once);
}

#[test]
#[ignore]
fn fuzz_limiter_long() {
    run("fuzz_limiter", true, 0, 50_000, limiter_once);
}

// ----- the cache: no refetch within 10s, If-Modified-Since, answers kept straight -----

/// What a URL's server does next.
#[derive(Clone, Copy, Debug)]
enum Reply {
    /// 200 with a new body, with or without Last-Modified.
    Fresh(bool),
    NotModified,
    NotFound,
    TooMany,
    Broken,
    NotJson,
}

fn cache_once(seed: u64) {
    let mut rng = Rng::new(seed);
    // More URLs than the cache holds, sometimes, so eviction is exercised.
    let urls: Vec<String> = (0..1 + rng.below(70)).map(|i| format!("https://api.example/{i}.json")).collect();
    let cache = Mutex::new(Cache::new(48));
    let clock = Cell::new(Instant::now());
    // The model: per URL, what we were last told (body, Last-Modified) and when it was checked.
    let mut known: HashMap<String, (u64, String, Instant)> = HashMap::new();
    let mut version = 0u64;
    for step in 0..rng.below(300) {
        // Strictly later each step, so no two copies were checked at the same moment.
        clock.set(clock.get() + Duration::from_millis(1 + rng.below(6_000) as u64));
        let url = rng.pick(&urls).clone();
        let reply = *rng.pick(&[
            Reply::Fresh(true),
            Reply::Fresh(true),
            Reply::Fresh(false),
            Reply::NotModified,
            Reply::NotModified,
            Reply::NotFound,
            Reply::TooMany,
            Reply::Broken,
            Reply::NotJson,
        ]);
        version += 1;
        let ctx = format!("seed {seed}, step {step}, {url}, {reply:?}");
        let mut asked = None;
        let res = http::cached_get(&cache, &url, || clock.get(), |u, since| {
            asked = Some(since.map(String::from));
            assert_eq!(u, url);
            Ok(match reply {
                Reply::Fresh(lm) => {
                    Raw { status: 200, last_modified: lm.then(|| format!("v{version}")), body: json!({ "v": version }).to_string() }
                }
                Reply::NotModified => Raw { status: 304, last_modified: None, body: String::new() },
                Reply::NotFound => Raw { status: 404, last_modified: None, body: String::new() },
                Reply::TooMany => Raw { status: 429, last_modified: None, body: String::new() },
                Reply::Broken => Raw { status: 503, last_modified: None, body: String::new() },
                Reply::NotJson => Raw { status: 200, last_modified: Some("x".into()), body: "<html>".into() },
            })
        });
        let fresh = known.get(&url).filter(|(_, _, checked)| clock.get().saturating_duration_since(*checked) < MIN_REFETCH);
        match (asked, fresh) {
            (Some(_), Some(_)) => panic!("refetched within {MIN_REFETCH:?} ({ctx})"),
            (None, Some(&(v, _, _))) => {
                let (body, age) = res.unwrap_or_else(|e| panic!("a cached answer failed: {e:#} ({ctx})"));
                assert_eq!(body["v"], v, "the cached body isn't the last one ({ctx})");
                assert!(age.is_some_and(|a| a < MIN_REFETCH), "a cached answer without its age ({ctx})");
                continue;
            }
            (None, None) => panic!("no request, and nothing fresh to answer with ({ctx})"),
            (Some(since), None) => {
                // If-Modified-Since exactly when there's a copy, with what it last said.
                assert_eq!(since.as_ref(), known.get(&url).map(|(_, lm, _)| lm), "wrong If-Modified-Since ({ctx})");
                match (reply, res) {
                    (Reply::Fresh(lm), Ok((body, None))) => {
                        assert_eq!(body["v"], version, "({ctx})");
                        if lm {
                            // A full cache drops the copy checked longest ago.
                            if !known.contains_key(&url) && known.len() >= 48 {
                                let oldest = known.iter().min_by_key(|(_, (_, _, checked))| *checked).map(|(k, _)| k.clone());
                                known.remove(&oldest.unwrap_or_default());
                            }
                            known.insert(url.clone(), (version, format!("v{version}"), clock.get()));
                        }
                    }
                    (Reply::NotModified, Ok((body, None))) => {
                        let entry = known.get_mut(&url).filter(|_| since.is_some());
                        let entry = entry.unwrap_or_else(|| panic!("a 304 answered without a copy ({ctx})"));
                        assert_eq!(body["v"], entry.0, "({ctx})");
                        entry.2 = clock.get();
                    }
                    (Reply::NotModified, Err(_)) => assert!(since.is_none(), "a 304 for a copy we have failed ({ctx})"),
                    (Reply::NotFound, Err(e)) => {
                        assert!(http::is_not_found(&e), "({ctx})");
                        known.remove(&url);
                    }
                    (Reply::TooMany | Reply::Broken | Reply::NotJson, Err(_)) => {}
                    (_, r) => panic!("unexpected {r:?} ({ctx})"),
                }
            }
        }
    }
}

#[test]
fn fuzz_cache() {
    run("fuzz_cache", false, 1, 300, cache_once);
}

#[test]
#[ignore]
fn fuzz_cache_long() {
    run("fuzz_cache", true, 0, 50_000, cache_once);
}
