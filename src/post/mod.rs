//! Posting, per engine: what each site's post form takes, how it asks for a captcha and
//! takes the answer, and what it says back. All of it goes through ck-web (`crate::web`), from
//! a page of the site's own, so the site's cookies and its Cloudflare pass go with it. The
//! reply box (`app::posting`) runs these on a thread of their own: they wait on the site.
#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects)]

mod form;
mod fourchan;
mod jschan;
mod kissu;
mod lynxchan;
mod makaba;
mod vichan;
mod work;

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use regex::Regex;

use crate::captcha::{Captcha, Challenge, Task};
use crate::config::{SiteConfig, SiteKind};
use crate::web::Helper;

/// Where a post goes: a site's board, and a thread there (0: a new thread).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Where {
    pub site: String,
    pub board: String,
    pub thread: u64,
}

impl Where {
    pub fn new_thread(&self) -> bool {
        self.thread == 0
    }

    /// The thread post `no` went to: this one, or (a new thread) itself.
    fn thread_of(&self, no: u64) -> u64 {
        if self.new_thread() { no } else { self.thread }
    }
}

/// A post as written in the reply box.
#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub name: String,
    /// The email field: where "sage" goes.
    pub email: String,
    pub subject: String,
    pub comment: String,
    pub file: Option<PathBuf>,
    pub spoiler: bool,
    /// For deleting it later, where the site lets you.
    pub password: String,
}

impl Draft {
    pub fn sage(&self) -> bool {
        self.email.trim().eq_ignore_ascii_case("sage")
    }
}

/// What a captcha's answer led to.
#[derive(Debug)]
pub enum Answered {
    /// It's done: the fields that go with the post.
    Done(Vec<(String, String)>),
    /// Another round of it (2ch's, a pick at a time).
    Next(Challenge),
}

/// What sending a post led to.
#[derive(Debug)]
pub enum Sent {
    /// It's up: its thread and number.
    Posted { thread: u64, no: u64 },
    /// The site wants a captcha first (a block bypass), then the post again.
    Again(Captcha),
}

/// An engine's way of posting.
pub trait Poster: Send + Sync {
    /// The page ck-web posts from: the site's own (its robots.txt, where there's nothing to
    /// run), so what it sends carries the site's cookies.
    fn page(&self) -> String;
    /// What posting `to` takes: a captcha, or none.
    fn captcha(&self, web: &Helper, to: &Where) -> Result<Captcha>;
    /// The answer to `challenge`: the fields for the post, or another round.
    fn answer(&self, web: &Helper, challenge: &Challenge, answer: &str) -> Result<Answered>;
    /// Send the post, with the captcha's fields.
    fn post(&self, web: &Helper, to: &Where, draft: &Draft, captcha: &[(String, String)]) -> Result<Sent>;
    /// The longest comment it takes, in characters, where that's the same everywhere on it.
    fn comment_limit(&self) -> Option<usize> {
        None
    }
}

/// How a site of this config posts, if its engine can.
pub fn poster(cfg: &SiteConfig) -> Option<Arc<dyn Poster>> {
    let root = crate::backend::site_url(cfg);
    let host = root.split("://").nth(1).unwrap_or_default().split('/').next().unwrap_or_default().trim_start_matches("www.").to_string();
    Some(match cfg.kind {
        SiteKind::Fourchan => Arc::new(fourchan::Fourchan),
        // 8kun posts to its sys host; kissu has its own engine now, on vichan's URLs.
        SiteKind::Vichan if host == "8kun.top" => Arc::new(vichan::Vichan { root, sys: Some("https://sys.8kun.top".into()) }),
        SiteKind::Vichan if host == "kissu.moe" => Arc::new(kissu::Kissu { root }),
        SiteKind::Vichan => Arc::new(vichan::Vichan { root, sys: None }),
        SiteKind::Jschan => Arc::new(jschan::Jschan { root }),
        SiteKind::Lynxchan => Arc::new(lynxchan::Lynxchan { root }),
        SiteKind::Makaba => Arc::new(makaba::Makaba { root }),
        SiteKind::Foolfuuka => return None,
    })
}

/// Eight letters and digits, new each run: what posts are sent with when `post_password`
/// isn't set (LynxChan keeps eight).
pub fn random_password() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut n = std::collections::hash_map::RandomState::new().build_hasher().finish();
    (0..8)
        .map(|_| {
            let c = char::from_digit(u32::try_from(n % 36).unwrap_or(0), 36).unwrap_or('0');
            n /= 36;
            c
        })
        .collect()
}

/// `secs` after `now`.
fn after(now: Instant, secs: u64) -> Instant {
    now.checked_add(Duration::from_secs(secs)).unwrap_or(now)
}

/// No captcha needed: a challenge that asks nothing (`id` says what for).
fn no_captcha(id: &str) -> Captcha {
    Captcha::Challenge(Challenge { id: id.into(), expires: after(Instant::now(), 3600), task: Task::None, pictures: Vec::new() })
}

/// A captcha done without the box (a service's widget, done in the browser view): nothing
/// left to answer, with the fields that go with the post.
fn solved(fields: &[(String, String)]) -> Captcha {
    let id = serde_json::to_string(fields).unwrap_or_default();
    Captcha::Challenge(Challenge { id, expires: after(Instant::now(), 110), task: Task::None, pictures: Vec::new() })
}

/// The fields of a `solved` captcha (none for one that asked nothing).
fn solved_fields(challenge: &Challenge) -> Vec<(String, String)> {
    serde_json::from_str(&challenge.id).unwrap_or_default()
}

/// The provider and sitekey of a service's captcha widget in a page (`data-sitekey` on an
/// `h-captcha`, `g-recaptcha`, `cf-turnstile` or `smart-captcha` element).
fn widget_in(html: &str) -> Option<(&'static str, String)> {
    static SITEKEY: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r#"class="([^"]*)"[^>]*data-sitekey="([^"]+)"|data-sitekey="([^"]+)"[^>]*class="([^"]*)""#).ok());
    SITEKEY.as_ref()?.captures_iter(html).find_map(|c| {
        let class = c.get(1).or_else(|| c.get(4))?.as_str();
        let key = c.get(2).or_else(|| c.get(3))?.as_str().to_string();
        let provider = [("h-captcha", "hcaptcha"), ("g-recaptcha", "recaptcha"), ("cf-turnstile", "turnstile"), ("smart-captcha", "yandex")].into_iter().find(|(c, _)| class.split_whitespace().any(|w| w == *c))?.1;
        Some((provider, key))
    })
}

/// A fresh number, for keys of captcha pictures (so a new one is never drawn from an old one's
/// cache) and cache-busting.
fn next_key() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A site's error message, as text.
fn said(message: &str) -> anyhow::Error {
    anyhow::anyhow!(crate::markup::strip_tags(message))
}

/// The number at the end of `/res/123.html` or `/thread/123.html` in a URL.
fn thread_in(url: &str) -> Option<u64> {
    let path = url.split(['#', '?']).next()?;
    let file = path.rsplit('/').next()?;
    file.strip_suffix(".html").or(Some(file)).and_then(|n| n.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widgets_are_found_by_their_class_and_sitekey() {
        assert_eq!(widget_in(r#"<div class="h-captcha" data-sitekey="k1"></div>"#), Some(("hcaptcha", "k1".into())));
        assert_eq!(widget_in(r#"<div data-sitekey="k2" class="g-recaptcha x"></div>"#), Some(("recaptcha", "k2".into())));
        assert_eq!(widget_in(r#"<div class="other" data-sitekey="k3"></div>"#), None);
        let c = solved(&[("captcha".into(), "t".into())]);
        let Captcha::Challenge(c) = c else { panic!("not a challenge") };
        assert_eq!(solved_fields(&c), [("captcha".to_string(), "t".to_string())]);
    }

    #[test]
    fn thread_numbers_from_links() {
        assert_eq!(thread_in("/b/res/123.html#456"), Some(123));
        assert_eq!(thread_in("https://a.example/b/thread/7.html"), Some(7));
        assert_eq!(thread_in("/b/index.html"), None);
    }
}
