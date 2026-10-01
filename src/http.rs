//! HTTP with imageboard API etiquette: per-host rate limiting, If-Modified-Since caching,
//! and a minimum interval between refetches of the same URL.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::Value;

/// At most one API request per second per host (4chan's rule, applied everywhere).
const API_INTERVAL: Duration = Duration::from_secs(1);
/// Hosts that only serve media (e.g. i.4cdn.org) get a looser budget.
const MEDIA_INTERVAL: Duration = Duration::from_millis(250);
/// Don't refetch the same URL more often than this; answer from the cache instead.
pub const MIN_REFETCH: Duration = Duration::from_secs(10);
const CACHE_ENTRIES: usize = 48;

static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(concat!("ck/", env!("CARGO_PKG_VERSION")))
        .http_status_as_error(false)
        .build()
        .into()
});

static LIMITER: LazyLock<Limiter> = LazyLock::new(Limiter::default);
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::new(CACHE_ENTRIES)));
static MEDIA_HOSTS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

thread_local! {
    /// Set when the last `get_json` on this thread was answered from the cache without a request.
    static CACHED_AGE: Cell<Option<Duration>> = const { Cell::new(None) };
}

/// HTTP failures worth telling apart from generic errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    NotFound(String),
    RateLimited,
    Status(u16, String),
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            HttpError::NotFound(url) => write!(f, "Not found (deleted or archived): {url}"),
            HttpError::RateLimited => write!(f, "Rate limited, try again shortly"),
            HttpError::Status(code, url) => write!(f, "HTTP {code} from {url}"),
        }
    }
}

impl std::error::Error for HttpError {}

pub fn is_not_found(e: &anyhow::Error) -> bool {
    matches!(e.downcast_ref::<HttpError>(), Some(HttpError::NotFound(_)))
}

/// Mark a host as serving only media, so it gets `MEDIA_INTERVAL` instead of `API_INTERVAL`.
pub fn register_media_host(url: &str) {
    MEDIA_HOSTS.lock().unwrap().insert(host(url).to_string());
}

/// How long ago the last `get_json` on this thread was fetched, if it came from the cache
/// without a request.
pub fn take_cached_age() -> Option<Duration> {
    CACHED_AGE.take()
}

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

/// Block until a request to `url`'s host is allowed.
fn throttle(url: &str) {
    let host = host(url);
    let interval = if MEDIA_HOSTS.lock().unwrap().contains(host) { MEDIA_INTERVAL } else { API_INTERVAL };
    let wait = LIMITER.reserve(host, interval, Instant::now());
    if !wait.is_zero() {
        std::thread::sleep(wait);
    }
}

/// Like `throttle`, but never books a future slot: background media fetches wait until the
/// host is idle, so they can't delay API requests by more than one interval.
fn throttle_low(url: &str) {
    let host = host(url);
    let interval = if MEDIA_HOSTS.lock().unwrap().contains(host) { MEDIA_INTERVAL } else { API_INTERVAL };
    while !LIMITER.try_reserve(host, interval, Instant::now()) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Per-host request scheduler. Each caller reserves the next free slot and sleeps until it,
/// so the lock is never held while waiting.
#[derive(Default)]
pub struct Limiter {
    next: Mutex<HashMap<String, Instant>>,
}

impl Limiter {
    /// Reserve a slot for `host` and return how long to wait for it.
    pub fn reserve(&self, host: &str, interval: Duration, now: Instant) -> Duration {
        let mut next = self.next.lock().unwrap();
        let slot = next.get(host).copied().filter(|&t| t > now).unwrap_or(now);
        next.insert(host.to_string(), slot + interval);
        slot - now
    }

    /// Reserve a slot for `host` only if one is free right now.
    pub fn try_reserve(&self, host: &str, interval: Duration, now: Instant) -> bool {
        let mut next = self.next.lock().unwrap();
        if next.get(host).is_some_and(|&t| t > now) {
            return false;
        }
        next.insert(host.to_string(), now + interval);
        true
    }
}

/// A response as far as the cache cares.
pub struct Raw {
    pub status: u16,
    pub last_modified: Option<String>,
    pub body: String,
}

struct Entry {
    last_modified: Option<String>,
    body: Value,
    checked: Instant,
}

/// Parsed JSON bodies keyed by URL, evicting the least recently checked entry.
pub struct Cache {
    entries: HashMap<String, Entry>,
    cap: usize,
}

impl Cache {
    pub fn new(cap: usize) -> Self {
        Self { entries: HashMap::new(), cap }
    }

    /// The cached body if it was checked less than `MIN_REFETCH` ago, with its age.
    fn fresh(&self, url: &str, now: Instant) -> Option<(Value, Duration)> {
        let e = self.entries.get(url)?;
        let age = now.saturating_duration_since(e.checked);
        (age < MIN_REFETCH).then(|| (e.body.clone(), age))
    }

    fn last_modified(&self, url: &str) -> Option<String> {
        self.entries.get(url)?.last_modified.clone()
    }

    /// Fold a response into the cache and return the body to use.
    fn update(&mut self, url: &str, raw: Raw, now: Instant) -> Result<Value> {
        match raw.status {
            304 => {
                if let Some(e) = self.entries.get_mut(url) {
                    e.checked = now;
                    return Ok(e.body.clone());
                }
                anyhow::bail!("{url} answered 304 Not Modified to a request we have no copy for")
            }
            200..=299 => {
                let body: Value =
                    serde_json::from_str(&raw.body).with_context(|| format!("{url} did not return JSON"))?;
                if raw.last_modified.is_some() {
                    if self.entries.len() >= self.cap && !self.entries.contains_key(url) {
                        let oldest = self.entries.iter().min_by_key(|(_, e)| e.checked).map(|(k, _)| k.clone());
                        if let Some(k) = oldest {
                            self.entries.remove(&k);
                        }
                    }
                    let entry = Entry { last_modified: raw.last_modified, body: body.clone(), checked: now };
                    self.entries.insert(url.to_string(), entry);
                }
                Ok(body)
            }
            404 | 410 => {
                self.entries.remove(url);
                Err(HttpError::NotFound(url.to_string()).into())
            }
            429 => Err(HttpError::RateLimited.into()),
            code => Err(HttpError::Status(code, url.to_string()).into()),
        }
    }
}

/// The cache logic around a transport, separated so it can be tested without the network.
pub fn cached_get(
    cache: &Mutex<Cache>,
    url: &str,
    now: impl Fn() -> Instant,
    transport: impl FnOnce(&str, Option<&str>) -> Result<Raw>,
) -> Result<(Value, Option<Duration>)> {
    let since = {
        let c = cache.lock().unwrap();
        if let Some((body, age)) = c.fresh(url, now()) {
            return Ok((body, Some(age)));
        }
        c.last_modified(url)
    };
    let raw = transport(url, since.as_deref())?;
    let body = cache.lock().unwrap().update(url, raw, now())?;
    Ok((body, None))
}

fn transport(url: &str, since: Option<&str>) -> Result<Raw> {
    throttle(url);
    let mut req = AGENT.get(url);
    if let Some(s) = since {
        req = req.header("If-Modified-Since", s);
    }
    let mut resp = req.call().with_context(|| format!("GET {url}"))?;
    let status = resp.status().as_u16();
    let last_modified = resp.headers().get("last-modified").and_then(|v| v.to_str().ok()).map(String::from);
    let body = if status == 200 {
        resp.body_mut()
            .with_config()
            .limit(32 * 1024 * 1024)
            .read_to_string()
            .with_context(|| format!("reading {url}"))?
    } else {
        String::new()
    };
    Ok(Raw { status, last_modified, body })
}

/// GET a URL and parse the body as JSON, through the rate limiter and cache.
pub fn get_json(url: &str) -> Result<Value> {
    let (body, age) = cached_get(&CACHE, url, Instant::now, transport)?;
    CACHED_AGE.set(age);
    Ok(body)
}

/// GET raw bytes (images, downloads) at low priority through the rate limiter. Not cached.
pub fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>> {
    throttle_low(url);
    let mut resp = AGENT.get(url).call().with_context(|| format!("GET {url}"))?;
    match resp.status().as_u16() {
        200..=299 => {}
        404 | 410 => return Err(HttpError::NotFound(url.to_string()).into()),
        429 => return Err(HttpError::RateLimited.into()),
        code => return Err(HttpError::Status(code, url.to_string()).into()),
    }
    resp.body_mut().with_config().limit(limit).read_to_vec().with_context(|| format!("reading {url}"))
}

/// Percent-encode a single path segment (board names can be non-ASCII, e.g. `λ`).
pub fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// Lenient accessors: imageboards are inconsistent about numbers vs. strings.

pub fn as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

pub fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

pub fn as_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub fn as_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_u64().is_some_and(|n| n != 0),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[test]
    fn host_of_url() {
        assert_eq!(host("https://a.4cdn.org/g/catalog.json"), "a.4cdn.org");
        assert_eq!(host("https://lainchan.org"), "lainchan.org");
        assert_eq!(host("http://x.net?y=1"), "x.net");
    }

    #[test]
    fn limiter_spaces_requests_per_host() {
        let l = Limiter::default();
        let t0 = Instant::now();
        let s = Duration::from_secs(1);
        assert_eq!(l.reserve("a", s, t0), Duration::ZERO);
        assert_eq!(l.reserve("a", s, t0), s);
        assert_eq!(l.reserve("a", s, t0), 2 * s);
        // Other hosts are independent.
        assert_eq!(l.reserve("b", s, t0), Duration::ZERO);
        // Once time has passed, slots free up again.
        assert_eq!(l.reserve("a", s, t0 + 10 * s), Duration::ZERO);
        assert_eq!(l.reserve("a", s, t0 + 10 * s + s / 2), s / 2);
    }

    #[test]
    fn low_priority_never_books_ahead() {
        let l = Limiter::default();
        let t0 = Instant::now();
        let s = Duration::from_secs(1);
        assert!(l.try_reserve("a", s, t0));
        assert!(!l.try_reserve("a", s, t0 + s / 2));
        // A normal request still gets the next slot, not one behind queued media.
        assert_eq!(l.reserve("a", s, t0 + s / 2), s / 2);
        assert!(!l.try_reserve("a", s, t0 + s + s / 2));
        assert!(l.try_reserve("a", s, t0 + 2 * s));
    }

    fn raw(status: u16, lm: Option<&str>, body: &str) -> Raw {
        Raw { status, last_modified: lm.map(String::from), body: body.into() }
    }

    #[test]
    fn cache_revalidates_and_serves_304() {
        let cache = Mutex::new(Cache::new(4));
        let t0 = Instant::now();
        let now = RefCell::new(t0);
        let clock = || *now.borrow();
        let seen = RefCell::new(Vec::new());

        // First fetch: no If-Modified-Since.
        let (v, age) = cached_get(&cache, "u", clock, |_, since| {
            seen.borrow_mut().push(since.map(String::from));
            Ok(raw(200, Some("LM1"), r#"{"n":1}"#))
        })
        .unwrap();
        assert_eq!((v["n"].as_u64(), age), (Some(1), None));

        // Within MIN_REFETCH: answered from cache, transport not called.
        *now.borrow_mut() = t0 + Duration::from_secs(4);
        let (v, age) = cached_get(&cache, "u", clock, |_, _| panic!("should not fetch")).unwrap();
        assert_eq!((v["n"].as_u64(), age), (Some(1), Some(Duration::from_secs(4))));

        // After MIN_REFETCH: revalidate with If-Modified-Since, 304 serves the cached copy.
        *now.borrow_mut() = t0 + Duration::from_secs(11);
        let (v, age) = cached_get(&cache, "u", clock, |_, since| {
            seen.borrow_mut().push(since.map(String::from));
            Ok(raw(304, None, ""))
        })
        .unwrap();
        assert_eq!((v["n"].as_u64(), age), (Some(1), None));

        // 304 reset the clock, so this is fresh again.
        *now.borrow_mut() = t0 + Duration::from_secs(15);
        assert!(cached_get(&cache, "u", clock, |_, _| panic!("should not fetch")).is_ok());

        // New content replaces the entry.
        *now.borrow_mut() = t0 + Duration::from_secs(30);
        let (v, _) = cached_get(&cache, "u", clock, |_, _| Ok(raw(200, Some("LM2"), r#"{"n":2}"#))).unwrap();
        assert_eq!(v["n"].as_u64(), Some(2));
        assert_eq!(*seen.borrow(), [None, Some("LM1".to_string())]);
    }

    #[test]
    fn cache_errors_and_eviction() {
        let cache = Mutex::new(Cache::new(2));
        let t0 = Instant::now();
        let e = cached_get(&cache, "gone", || t0, |_, _| Ok(raw(404, None, ""))).unwrap_err();
        assert!(is_not_found(&e));
        let e = cached_get(&cache, "slow", || t0, |_, _| Ok(raw(429, None, ""))).unwrap_err();
        assert_eq!(e.to_string(), "Rate limited, try again shortly");

        for (i, url) in ["a", "b", "c"].iter().enumerate() {
            let t = t0 + Duration::from_secs(i as u64);
            cached_get(&cache, url, || t, |_, _| Ok(raw(200, Some("x"), "1"))).unwrap();
        }
        let c = cache.lock().unwrap();
        assert_eq!(c.entries.len(), 2);
        assert!(!c.entries.contains_key("a"));
    }
}
