//! Fuzzing without dependencies: a seeded generator, a runner that prints how to replay a
//! failure, and targets for the parsers and the request etiquette. The app itself is fuzzed
//! in `app::fuzz`.
//!
//! Each target has a short run in `cargo test` and a long one that's ignored:
//! `cargo test --profile fuzz -- --ignored _long --nocapture`, with `FUZZ_SEED` and
//! `FUZZ_RUNS` (or `FUZZ_SECS`, a time budget per target) to choose. A failure prints the
//! seed that replays it on its own.

use std::cell::Cell;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Line;
use serde_json::{Value, json};

use crate::http::{self, Cache, Limiter, MIN_REFETCH, Priority, Raw};
use crate::markup::{self, Flavor};
use crate::config::{SiteConfig, SiteKind};
use crate::route;

/// The wall clock fuzzed data is dated from.
const START: i64 = 1_790_000_000;

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
    let budget = env("FUZZ_SECS").filter(|_| long && env("FUZZ_RUNS").is_none()).map(Duration::from_secs);
    let (seed, runs) = if long {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(seed, |d| d.as_nanos() as u64);
        (env("FUZZ_SEED").unwrap_or(now), if budget.is_some() { u64::MAX } else { env("FUZZ_RUNS").unwrap_or(runs) })
    } else {
        (seed, runs)
    };
    if long {
        match budget {
            Some(b) => eprintln!("{name}: seed {seed}, for {b:?}"),
            None => eprintln!("{name}: seed {seed}, {runs} runs"),
        }
    }
    let start = Instant::now();
    let mut done = 0;
    for i in 0..runs {
        if budget.is_some_and(|b| start.elapsed() >= b) {
            break;
        }
        done += 1;
        // The first run uses the seed itself, so a reported seed replays with FUZZ_RUNS=1.
        let s = if i == 0 { seed } else { Rng::new(seed ^ i.wrapping_mul(0xa076_1d64_78bd_642f)).next() };
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| body(s))) {
            eprintln!("\n{name} failed on run {i}. Replay it with:");
            eprintln!("  FUZZ_SEED={s} FUZZ_RUNS=1 cargo test --profile fuzz {name}_long -- --ignored --nocapture\n");
            resume_unwind(panic);
        }
    }
    if long {
        eprintln!("{name}: {done} runs in {:.1?}", start.elapsed());
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
    "👨\u{200d}👩\u{200d}👧", "❤\u{fe0f}", "1\u{fe0f}\u{20e3}", "🏴\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f}", "e\u{301}\u{301}",
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
        // What's drawn is measured alike whole or a character at a time (as a terminal
        // without grapheme clustering does), so nothing lands in the wrong cell.
        for w in markup::wrap(line, 40) {
            for s in &w.spans {
                use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
                let safe = markup::for_terminal(&s.content);
                let shown: String = safe.chars().filter(|c| !c.is_control()).collect();
                let by_char: usize = shown.chars().map(|c| c.width().unwrap_or(0)).sum();
                assert_eq!(shown.width(), by_char, "measured differently: {shown:?} ({})", ctx());
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
#[ignore = "long fuzz run"]
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
#[ignore = "long fuzz run"]
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
#[ignore = "long fuzz run"]
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
#[ignore = "long fuzz run"]
fn fuzz_cache_long() {
    run("fuzz_cache", true, 0, 50_000, cache_once);
}

// ----- sites answering badly: real fixtures, mangled, through the real backends -----

/// The real responses each engine's answers are made from.
fn fixtures_for(kind: SiteKind) -> &'static [&'static str] {
    match kind {
        SiteKind::Fourchan | SiteKind::Vichan => &[
            "4chan_boards.json",
            "8kun_boards.json",
            "4chan_catalog.json",
            "8kun_catalog.json",
            "vichan_catalog.json",
            "wizchan_catalog.json",
            "leftypol_catalog.json",
            "leftypol_overboard.json",
            "4chan_thread.json",
            "4chan_pol_thread.json",
            "vichan_thread.json",
            "leftypol_thread.json",
            "4chan_spoiler_post.json",
            "4chan_pages.json",
            "vichan_pages.json",
        ],
        SiteKind::Lynxchan => &[
            "lynxchan_boards.json",
            "lynxchan_boards_wrapped.json",
            "lynxchan_catalog.json",
            "kohlchan_catalog.json",
            "lynxchan_overboard.json",
            "kohlchan_overboard.json",
            "lynxchan_thread.json",
            "kohlchan_thread.json",
        ],
        SiteKind::Foolfuuka => &[
            "foolfuuka_archives.json",
            "foolfuuka_index.json",
            "foolfuuka_search.json",
            "foolfuuka_thread.json",
            "foolfuuka_post.json",
        ],
        SiteKind::Jschan => &["jschan_boards.json", "jschan_catalog.json", "jschan_overboard.json", "jschan_thread.json"],
        SiteKind::Makaba => &["makaba_boards.json", "makaba_catalog.json", "makaba_thread.json", "makaba_post.json"],
    }
}

/// Every engine but 4chan's (its hosts are fixed; vichan speaks the same JSON).
pub const ENGINES: [SiteKind; 5] = [SiteKind::Vichan, SiteKind::Lynxchan, SiteKind::Foolfuuka, SiteKind::Jschan, SiteKind::Makaba];

static FIXTURES: std::sync::LazyLock<HashMap<&'static str, Value>> = std::sync::LazyLock::new(|| {
    ENGINES.iter().flat_map(|&k| fixtures_for(k)).map(|&name| (name, crate::backend::fixture(name))).collect()
});

/// Something odd in place of a JSON value.
fn odd_value(rng: &mut Rng) -> Value {
    match rng.below(14) {
        0 => Value::Null,
        1 => json!(true),
        2 => json!(0),
        3 => json!(-1),
        4 => json!(u64::MAX),
        5 => json!(i64::MIN),
        6 => json!(1.5e300),
        7 => json!(""),
        8 => json!("x".repeat(rng.below(20_000))),
        9 => json!(html(rng, 8)),
        10 => json!("123"),
        11 => json!([]),
        12 => json!({}),
        _ => (0..rng.below(300)).fold(json!(1), |v, _| json!([v])),
    }
}

/// Change a few things somewhere in `v`: values swapped for odd ones, keys and items dropped,
/// duplicated or reordered.
pub fn mutate(rng: &mut Rng, v: &mut Value) {
    // Somewhere down the tree, most of the time.
    let descend = rng.chance(75);
    match v {
        Value::Object(m) if descend && !m.is_empty() => {
            let k = m.keys().nth(rng.below(m.len())).cloned().unwrap_or_default();
            if let Some(child) = m.get_mut(&k) {
                return mutate(rng, child);
            }
        }
        Value::Array(a) if descend && !a.is_empty() => {
            let i = rng.below(a.len());
            if let Some(child) = a.get_mut(i) {
                return mutate(rng, child);
            }
        }
        _ => {}
    }
    match v {
        Value::Object(m) if rng.chance(50) && !m.is_empty() => {
            let k = m.keys().nth(rng.below(m.len())).cloned().unwrap_or_default();
            if rng.chance(50) {
                m.remove(&k);
            } else {
                m.insert(k, odd_value(rng));
            }
        }
        Value::Array(a) if rng.chance(50) && !a.is_empty() => match rng.below(4) {
            0 => {
                a.remove(rng.below(a.len()));
            }
            1 => {
                let i = rng.below(a.len());
                let dup = a[i].clone();
                a.insert(i, dup);
            }
            2 => a.reverse(),
            _ => a.truncate(rng.below(a.len())),
        },
        other => *other = odd_value(rng),
    }
}

/// A fake site of `kind`: what its API answers (badly, `percent` of the time).
pub fn fake_site(kind: SiteKind, seed: u64, percent: u64) -> http::TestHost {
    let rng = Mutex::new(Rng::new(seed));
    std::sync::Arc::new(move |url: &str, _since: Option<&str>| {
        let mut rng = http::lock(&rng);
        let names = fixtures_for(kind);
        let wanted: Vec<&&str> = names
            .iter()
            .filter(|n| {
                let want = |k: &str| n.contains(k);
                if url.contains("boards") || url.contains("archives") {
                    want("boards") || want("archives")
                } else if url.contains("search") {
                    want("search")
                } else if url.ends_with("/threads.json") {
                    want("pages")
                } else if url.contains("/post/") || url.contains("chan/post") {
                    want("post")
                } else if url.contains("catalog") || url.contains("index") || url.ends_with("/1.json") {
                    want("catalog") || want("overboard") || want("index")
                } else {
                    want("thread")
                }
            })
            .collect();
        let name = if wanted.is_empty() || rng.chance(5) { *rng.pick(names) } else { **rng.pick(&wanted) };
        let ok = |body: String| Raw { status: 200, last_modified: None, body };
        let mut v = FIXTURES.get(name).cloned().unwrap_or_default();
        if !rng.chance(percent) {
            return ok(v.to_string());
        }
        match rng.below(100) {
            0..6 => Raw { status: 404, last_modified: None, body: String::new() },
            6..9 => Raw { status: 429, last_modified: None, body: String::new() },
            9..12 => Raw { status: 503, last_modified: None, body: String::new() },
            12..14 => Raw { status: 304, last_modified: None, body: String::new() },
            14..17 => ok(String::new()),
            17..20 => ok("<html><body>Cloudflare</body></html>".into()),
            20..25 => {
                let s = v.to_string();
                let cut = rng.below(s.len());
                ok(s.chars().take(cut).collect())
            }
            _ => {
                for _ in 0..1 + rng.below(6) {
                    mutate(&mut rng, &mut v);
                }
                ok(v.to_string())
            }
        }
    })
}

/// A real backend of `kind`, talking to a fake site at `host`.
pub fn backend_at(kind: SiteKind, host: &str) -> std::sync::Arc<dyn crate::backend::Backend> {
    let kind = format!("{kind:?}").to_lowercase();
    let cfg: SiteConfig = toml::from_str(&format!("name = \"{host}\"\nkind = \"{kind}\"\nurl = \"https://{host}\"")).unwrap();
    crate::backend::build(&cfg)
}

fn backends_once(seed: u64) {
    let mut rng = Rng::new(seed);
    for kind in ENGINES {
        let host = format!("{kind:?}-{seed}.fuzz.invalid").to_lowercase();
        http::serve_test_host(&host, Some(fake_site(kind, rng.next(), 70)));
        let b = backend_at(kind, &host);
        let look = |posts: &[crate::model::Post]| {
            for p in posts {
                let _ = (p.plain_text(), p.search_text());
            }
        };
        let boards = b.boards(&|_| {}).unwrap_or_default();
        let board = boards.first().map_or_else(|| "g".to_string(), |b| b.uri.clone());
        if let Ok(cat) = b.catalog(&board, &|p| look(p)) {
            look(&cat);
            let no = cat.first().map_or(1, |p| p.no);
            let _ = b.thread_pages(&board);
            if let Ok(posts) = b.thread(&board, no) {
                look(&posts);
                let _ = b.find_thread(&board, posts.last().map_or(no, |p| p.no));
            }
        }
        if let Ok(page) = b.search(&board, "a", 1 + rng.below(3) as u32) {
            look(&page.hits.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>());
        }
        http::serve_test_host(&host, None);
    }
}

#[test]
fn fuzz_backends() {
    run("fuzz_backends", false, 1, 40, backends_once);
}

#[test]
#[ignore = "long fuzz run"]
fn fuzz_backends_long() {
    run("fuzz_backends", true, 0, 5_000, backends_once);
}

// ----- files on disk: data, config and cached images, broken -----

/// Break some bytes: as JSON when they are, else as bytes.
fn break_bytes(rng: &mut Rng, bytes: &[u8]) -> Vec<u8> {
    match rng.below(10) {
        0 => Vec::new(),
        1 => bytes.get(..rng.below(bytes.len() + 1)).unwrap_or_default().to_vec(),
        2 => (0..rng.below(300)).map(|_| rng.next() as u8).collect(),
        3 => rng.pick(&[&b"null"[..], b"[]", b"{}", b"0", b"\"\"", b"\xff\xfe", b"[[[[[[[[[["]).to_vec(),
        4 => {
            let mut b = bytes.to_vec();
            for _ in 0..1 + rng.below(8) {
                if !b.is_empty() {
                    let i = rng.below(b.len());
                    b[i] = rng.next() as u8;
                }
            }
            b
        }
        _ => match serde_json::from_slice::<Value>(bytes) {
            Ok(mut v) => {
                for _ in 0..1 + rng.below(4) {
                    mutate(rng, &mut v);
                }
                v.to_string().into_bytes()
            }
            Err(_) => bytes.to_vec(),
        },
    }
}

fn data_dir_once(seed: u64) {
    use crate::store::{Place, Session, Store, ThreadKey};
    let mut rng = Rng::new(seed);
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    // A plausible data directory first.
    let (mut store, _) = Store::load(Some(data.clone()));
    for i in 0..rng.below(6) as u64 {
        let key = ThreadKey { site: rng.pick(&["4chan", "lainchan", "nosuch"]).to_string(), board: "g".into(), no: 100 + i };
        store.toggle_watch(key.clone(), format!("thread {i}"), 10, 105 + i);
        store.visit(&key, "subject", 10, 109, START + i as i64);
        store.toggle_hidden(&key.site, "g", 200 + i);
        store.opened(&key.site, "g", key.no, 9, START);
        // A saved copy, some of them of dead threads.
        let html = format!("<span class=\"quote\">&gt;{i}</span><br><a href=\"#p{}\" class=\"quotelink\">&gt;&gt;{}</a> <s>spoiler</s>", key.no, key.no);
        let parsed = crate::markup::parse_html(&html, crate::markup::Flavor::Fourchan);
        let posts: Vec<crate::model::Post> = (0..3).map(|k| crate::model::Post { no: key.no + k, body: parsed.lines.clone(), anchors: parsed.anchors.clone(), ..Default::default() }).collect();
        store.keep_thread(&key, &format!("thread {i}"), "u", &posts, START + i as i64);
        if rng.chance(50) {
            store.saved_dead(&key);
        }
    }
    store.recent_boards = vec!["4chan/g".into(), "lainchan/λ".into(), "x".into()];
    assert!(store.flush(Duration::from_secs(10)).is_empty());
    store.save().unwrap();
    let place = |view: &str| Place { view: view.into(), site: "4chan".into(), board: Some("g".into()), thread: Some(100), ..Default::default() };
    store.save_session(&Session { tabs: vec![place("thread"), place("catalog"), place("watched"), place("saved")], active: rng.below(5) }).unwrap();
    store.save_boards("4chan", &[crate::model::Board { uri: "g".into(), title: "Technology".into(), nsfw: Some(false) }], START).unwrap();
    // Then break some of it.
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&data).unwrap().filter_map(|e| Some(e.ok()?.path())).filter(|p| p.is_file()).collect();
    files.push(data.join("boards").join("4chan.json"));
    // Saved copies: threads/<site>/<board>/<no>.json.
    let copies: Vec<ThreadKey> = store.saved.iter().map(|m| m.key.clone()).collect();
    files.extend(copies.iter().map(|k| crate::saved::path(&data, k)));
    files.sort();
    let mut broken: HashMap<std::path::PathBuf, Vec<u8>> = HashMap::new();
    for path in &files {
        if rng.chance(40) {
            let bytes = break_bytes(&mut rng, &std::fs::read(path).unwrap_or_default());
            std::fs::write(path, &bytes).unwrap();
            broken.insert(path.clone(), bytes);
        }
    }
    let (mut store, warnings) = Store::load(Some(data.clone()));
    // Every copy is read (as opening it from the Saved view would).
    for key in &copies {
        if let Ok(t) = store.load_saved(key) {
            let _: Vec<crate::model::Post> = t.posts.into_iter().map(Into::into).collect();
        }
    }
    // Nothing the user had is destroyed: a file that wouldn't load is kept beside.
    for (path, bytes) in &broken {
        if !path.exists() {
            let aside = path.with_extension("json.corrupt");
            assert_eq!(std::fs::read(&aside).ok().as_ref(), Some(bytes), "{} vanished (seed {seed}; {warnings:?})", path.display());
        }
    }
    // The app starts on it, restores the session, draws and saves.
    let mut app = crate::test_fixtures::test_app();
    let cfg: crate::config::Config = toml::from_str(crate::config::DEFAULT_CONFIG).unwrap();
    let filters = crate::filter::Filters::from_config(&cfg.filters, &cfg.hidden_words).unwrap();
    app = crate::app::App::new(cfg, app.keys.clone(), filters, None, store);
    app.config_path = None;
    app.restore_session();
    // The Saved view, and each copy in it.
    app.goto_str("saved");
    for key in copies {
        app.open_saved(&key);
        crate::test_fixtures::draw_at(&mut app, 60, 20);
    }
    for (w, h) in [(110, 32), (20, 5)] {
        crate::test_fixtures::draw_at(&mut app, w, h);
    }
    app.save_now();
    let (_, warnings) = Store::load(Some(data));
    assert!(warnings.is_empty(), "what was saved doesn't load cleanly (seed {seed}): {warnings:?}");
}

#[test]
fn fuzz_data_dir() {
    run("fuzz_data_dir", false, 1, 60, data_dir_once);
}

#[test]
#[ignore = "long fuzz run"]
fn fuzz_data_dir_long() {
    run("fuzz_data_dir", true, 0, 5_000, data_dir_once);
}

/// The page cache with broken files: reading never panics, a broken file is dropped, and
/// what's read parses (as opening from it would) without a request.
fn pages_once(seed: u64) {
    use crate::backend::Backend;
    use crate::pages::Pages;
    let mut rng = Rng::new(seed);
    let dir = tempfile::tempdir().unwrap();
    let pages = Pages::new(dir.path().to_path_buf(), 1 << 22);
    let fourchan = crate::backend::futaba::Futaba::fourchan(None);
    let fixture = crate::backend::fixture("4chan_thread.json");
    let n = 1 + rng.below(5) as u64;
    for no in 0..n {
        let body = if rng.chance(30) { mangle_json(&mut rng, &fixture) } else { fixture.clone() };
        let copy = http::Copy { url: format!("https://a.4cdn.org/g/thread/{no}.json"), last_modified: rng.chance(70).then(|| "day".into()), body };
        pages.write("4chan", "g", Some(no), &[copy], no as i64);
    }
    // Break some files.
    let walk = |p: &std::path::Path| std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).collect::<Vec<_>>();
    for file in walk(&dir.path().join("4chan").join("g")) {
        if rng.chance(40) {
            let bytes = break_bytes(&mut rng, &std::fs::read(&file).unwrap_or_default());
            std::fs::write(&file, bytes).unwrap();
        }
    }
    for no in 0..n {
        let Some((copies, _)) = pages.read("4chan", "g", Some(no)) else { continue };
        // From the copies only: an error at worst, never a request (4chan is a real host,
        // which tests refuse).
        let _ = http::from_copies(&copies, || fourchan.thread("g", no));
    }
    // What's left loads (broken files were dropped).
    for file in walk(&dir.path().join("4chan").join("g")) {
        let name = file.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse().ok());
        assert!(name.is_some_and(|no| pages.read("4chan", "g", Some(no)).is_some()), "{} stayed broken (seed {seed})", file.display());
    }
}

/// A JSON value with a few parts changed.
fn mangle_json(rng: &mut Rng, v: &Value) -> Value {
    let mut v = v.clone();
    for _ in 0..1 + rng.below(4) {
        mutate(rng, &mut v);
    }
    v
}

#[test]
fn fuzz_pages() {
    run("fuzz_pages", false, 1, 40, pages_once);
}

#[test]
#[ignore = "long fuzz run"]
fn fuzz_pages_long() {
    run("fuzz_pages", true, 0, 5_000, pages_once);
}

/// Odd values for a config setting.
fn odd_toml(rng: &mut Rng) -> toml_edit::Item {
    use toml_edit::value;
    match rng.below(12) {
        0 => value(-1),
        1 => value(i64::MAX),
        2 => value(0),
        3 => value(""),
        4 => value("nonsense"),
        5 => value(true),
        6 => value(1.5),
        7 => value(toml_edit::Array::from_iter(["a", "", "x/y/z"])),
        8 => value(toml_edit::Array::new()),
        9 => value(html(rng, 3)),
        10 => value("#zzzzzz"),
        _ => value("ctrl-alt-shift-f99"),
    }
}

fn config_once(seed: u64) {
    let mut rng = Rng::new(seed);
    let mut doc: toml_edit::DocumentMut = crate::config::DEFAULT_CONFIG.parse().unwrap();
    const KEYS: &[&str] = &[
        "theme", "color", "images", "notify", "notify_command", "catalog_layout", "compact_catalog", "refresh_thread_secs",
        "refresh_watched_secs", "restore_session", "download_dir", "favorites", "hidden_sites", "hidden_words", "nsfw_images",
        "follow_new_posts", "watched_first",
    ];
    for _ in 0..1 + rng.below(5) {
        match rng.below(8) {
            0 => doc[*rng.pick(KEYS)] = odd_toml(&mut rng),
            1 => {
                doc.remove(rng.pick(KEYS));
            }
            2 => {
                let action = rng.pick(crate::keys::ACTIONS).1;
                doc["keys"][action] = odd_toml(&mut rng);
            }
            3 => {
                let mut f = toml_edit::Table::new();
                f["pattern"] = toml_edit::value(*rng.pick(&["(", "a{99999}", "", "(?i)x", "[", "\\p{Han}"]));
                f[*rng.pick(&["action", "field", "label", "enabled", "sites", "boards", "op", "reply", "notify", "top"])] = odd_toml(&mut rng);
                doc["filter"].or_insert(toml_edit::Item::ArrayOfTables(Default::default()));
                if let Some(a) = doc["filter"].as_array_of_tables_mut() {
                    a.push(f);
                }
            }
            4 => {
                doc["themes"]["odd"][*rng.pick(&["base", "seed", "mode", "primary", "background"])] = odd_toml(&mut rng);
                if rng.chance(50) {
                    doc["theme"] = toml_edit::value("odd");
                }
            }
            // A valid filter: any fields, any scope, on or off.
            6 | 7 => {
                use crate::filter::{Field, FilterAction, FilterConfig};
                let fields: Vec<Field> = Field::ALL.into_iter().filter(|_| rng.chance(40)).collect();
                // (A file size's pattern is a range, and a regex too.)
                let patterns: &[&str] = if fields.contains(&Field::Filesize) { &[">2MB", "<=100KB", "1KB-5MB", "0"] } else { &["(?i)word", "^Anon$", "日本", "abc==", "x|y"] };
                let mut f = FilterConfig::new(rng.pick(patterns).to_string(), if fields.is_empty() { &[Field::Comment] } else { &fields });
                f.action = if rng.chance(50) { FilterAction::Hide } else { FilterAction::Highlight };
                f.sites = (0..rng.below(3)).map(|_| rng.pick(&["4chan", "lainchan", "nosuch"]).to_string()).collect();
                f.boards = (0..rng.below(3)).map(|_| rng.pick(&["g", "λ", "b"]).to_string()).collect();
                f.label = rng.chance(50).then(|| html(&mut rng, 1));
                f.enabled = rng.chance(70);
                f.recursive = rng.chance(30);
                // OPs or replies (never both: that's refused).
                (f.op, f.reply) = match rng.below(4) {
                    0 => (true, false),
                    1 => (false, true),
                    _ => (false, false),
                };
                f.notify = rng.chance(30);
                f.top = rng.chance(30);
                let mut t = toml_edit::Table::new();
                f.write(&mut t, None);
                doc["filter"].or_insert(toml_edit::Item::ArrayOfTables(Default::default()));
                if let Some(a) = doc["filter"].as_array_of_tables_mut() {
                    a.push(t);
                }
            }
            _ => {
                if let Some(sites) = doc["site"].as_array_of_tables_mut()
                    && !sites.is_empty()
                {
                    let i = rng.below(sites.len());
                    if let Some(t) = sites.get_mut(i) {
                        t[*rng.pick(&["name", "kind", "url", "boards", "thumb_ext", "archive", "media_url"])] = odd_toml(&mut rng);
                    }
                }
            }
        }
    }
    let mut text = doc.to_string();
    if rng.chance(10) {
        text = String::from_utf8_lossy(&break_bytes(&mut rng, text.as_bytes())).into_owned();
    }
    // As main does (and Config::load, which adds the built-in sites): each check may refuse
    // the config, none may panic.
    let Ok(cfg) = toml::from_str::<crate::config::Config>(&text).map_err(anyhow::Error::from).and_then(crate::config::Config::with_builtin_sites) else { return };
    let Ok(keys) = crate::keys::KeyMap::new(&cfg.keys) else { return };
    if crate::filter::Filters::from_config(&cfg.filters, &cfg.hidden_words).is_err() || crate::theme::from_config(cfg.theme.as_ref(), &cfg.themes).is_err() {
        return;
    }
    // Editing its filters changes just the one, and what's written reads back.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, &text).unwrap();
    if !cfg.filters.is_empty() {
        use crate::config::{FilterEdit, try_edit_at, edit_filters};
        let filters = |path: &std::path::Path| toml::from_str::<crate::config::Config>(&std::fs::read_to_string(path).unwrap()).unwrap().filters;
        let i = rng.below(cfg.filters.len());
        let old = cfg.filters[i].clone();
        let mut new = crate::filter::FilterConfig { enabled: !old.enabled, ..old.clone() };
        if rng.chance(50) {
            new.set_fields(&[crate::filter::Field::Name]);
            new.boards.clear();
        }
        let mut want = cfg.filters.clone();
        match rng.below(3) {
            0 => {
                try_edit_at(&path, |d| edit_filters(d, FilterEdit::Change(i, &old, &new))).unwrap();
                want[i] = new;
            }
            1 => {
                try_edit_at(&path, |d| edit_filters(d, FilterEdit::Remove(i, &old))).unwrap();
                want.remove(i);
            }
            _ => {
                try_edit_at(&path, |d| edit_filters(d, FilterEdit::Add(&new))).unwrap();
                want.push(new);
            }
        }
        assert_eq!(filters(&path), want, "seed {seed}");
        // An edit naming what isn't there is refused, and nothing is written.
        let before = std::fs::read_to_string(&path).unwrap();
        let other = crate::filter::FilterConfig::new("not there".into(), &[crate::filter::Field::Md5]);
        assert!(try_edit_at(&path, |d| edit_filters(d, FilterEdit::Remove(0, &other))).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        std::fs::write(&path, &text).unwrap();
    }
    let filters = crate::filter::Filters::from_config(&cfg.filters, &cfg.hidden_words).unwrap();
    let mut app = crate::app::App::new(cfg, keys, filters, None, crate::store::Store::default());
    app.config_path = Some(path);
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    for key in [KeyCode::Enter, KeyCode::Char(','), KeyCode::Char('j'), KeyCode::Enter, KeyCode::Esc] {
        app.on_key(KeyEvent::from(key));
        term.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    }
    // The filter list, and whatever its keys do.
    app.popup = Some(crate::app::Popup::Settings(app.filter_list(0)));
    for _ in 0..rng.below(12) {
        let key = *rng.pick(&[KeyCode::Char('j'), KeyCode::Char(' '), KeyCode::Char('x'), KeyCode::Enter, KeyCode::Char('a'), KeyCode::Char('k'), KeyCode::Esc, KeyCode::Char('w')]);
        app.on_key(KeyEvent::from(key));
        term.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    }
    // What's in memory is what's in the file.
    let on_disk = toml::from_str::<crate::config::Config>(&std::fs::read_to_string(app.config_path.as_ref().unwrap()).unwrap()).unwrap();
    assert_eq!(on_disk.filters, app.filter_cfgs, "seed {seed}");
}

#[test]
fn fuzz_config() {
    run("fuzz_config", false, 1, 300, config_once);
}

#[test]
#[ignore = "long fuzz run"]
fn fuzz_config_long() {
    run("fuzz_config", true, 0, 50_000, config_once);
}

/// Small real images of each format, encoded here.
pub static IMAGES: std::sync::LazyLock<Vec<Vec<u8>>> = std::sync::LazyLock::new(|| {
    use image::{DynamicImage, ImageFormat, RgbaImage};
    let img = DynamicImage::ImageRgba8(RgbaImage::from_fn(17, 11, |x, y| image::Rgba([x as u8 * 15, y as u8 * 20, 99, 255])));
    [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Gif, ImageFormat::WebP]
        .into_iter()
        .filter_map(|f| {
            let mut out = std::io::Cursor::new(Vec::new());
            let img = if f == ImageFormat::Jpeg { DynamicImage::ImageRgb8(img.to_rgb8()) } else { img.clone() };
            img.write_to(&mut out, f).ok()?;
            Some(out.into_inner())
        })
        .collect()
});

fn image_once(seed: u64) {
    let mut rng = Rng::new(seed);
    let mut bytes = rng.pick(&IMAGES).clone();
    match rng.below(4) {
        0 => bytes = break_bytes(&mut rng, &bytes),
        // A header that claims a huge picture.
        1 if bytes.len() > 24 => {
            for i in 6..10.min(bytes.len()) {
                bytes[i] = 0xff;
            }
            if bytes.starts_with(b"\x89PNG") {
                bytes[16..24].copy_from_slice(&[0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff]);
            }
        }
        2 => {
            let cut = rng.below(bytes.len());
            bytes.truncate(cut);
        }
        _ => {
            for _ in 0..1 + rng.below(20) {
                let i = rng.below(bytes.len());
                if let Some(b) = bytes.get_mut(i) {
                    *b ^= 1 << rng.below(8);
                }
            }
        }
    }
    if let Ok(img) = crate::images::decode(&bytes) {
        assert!(img.width() <= 4096 && img.height() <= 4096, "a {}x{} image kept (seed {seed})", img.width(), img.height());
    }
    let _ = crate::images::gif_frames_within(&bytes, 1 << 20);
}

#[test]
fn fuzz_images() {
    run("fuzz_images", false, 1, 300, image_once);
}

#[test]
#[ignore = "long fuzz run"]
fn fuzz_images_long() {
    run("fuzz_images", true, 0, 50_000, image_once);
}
