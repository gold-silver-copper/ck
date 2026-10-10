//! Posting, per engine: what each site's post form takes, how it asks for a captcha and
//! takes the answer, and what it says back. Each engine posts in one go (`Poster::post`), on a
//! thread of its own: through ck-web (`Web`), from a page of the site's own, so the site's
//! cookies and its Cloudflare pass go with it; and asking the person at the reply box (`Ask`)
//! when it needs them.
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

use crate::captcha::Challenge;
use crate::config::SiteConfig;
pub use crate::web::Web;
pub use {fourchan::Fourchan, jschan::Jschan, kissu::Kissu, lynxchan::Lynxchan, makaba::Makaba, vichan::Vichan, work::Work};

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

/// A post that went up: its thread and number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Posted {
    pub thread: u64,
    pub no: u64,
}

/// The person at the reply box, asked by a post on its way. Each call waits for them; an
/// error means they stopped (or closed the box), and the post stops with it.
pub trait Ask {
    /// Show `challenge` and wait for the answer; None when they asked for another one (or it
    /// expired).
    fn solve(&self, challenge: &Challenge) -> Result<Option<String>>;
    /// The site says to wait (posting too often): show its message, and wait till it's time
    /// and they ask again.
    fn wait(&self, until: Instant, message: &str) -> Result<()>;
}

/// What a post goes through: the site (by ck-web), the person, and how much work a proof of
/// work may take.
pub struct Session<'a> {
    pub web: &'a dyn Web,
    pub ask: &'a dyn Ask,
    pub work: Work,
}

impl Session<'_> {
    /// The answer to a captcha from `next`, asked for again (another fetched) until there's
    /// one; and the challenge it answers.
    fn solve(&self, mut next: impl FnMut() -> Result<Challenge>) -> Result<(Challenge, String)> {
        loop {
            let challenge = next()?;
            if let Some(answer) = self.ask.solve(&challenge)? {
                return Ok((challenge, answer));
            }
        }
    }
}

/// An engine's way of posting.
pub trait Poster: Send + Sync {
    /// The page ck-web posts from: the site's own (its robots.txt, where there's nothing to
    /// run), so what it sends carries the site's cookies.
    fn page(&self) -> String;
    /// Send `draft` to `to`: its captcha, block bypass and proof of work on the way, as the
    /// site asks.
    fn post(&self, s: &Session, to: &Where, draft: &Draft) -> Result<Posted>;
    /// The longest comment it takes, in characters, where that's the same everywhere on it.
    fn comment_limit(&self) -> Option<usize> {
        None
    }
}

/// How a site of this config posts, if its engine can.
pub fn poster(cfg: &SiteConfig) -> Option<Arc<dyn Poster>> {
    cfg.kind.engine().post.map(|post| post(cfg))
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

/// A site and a person for tests: the site's answers recorded from it, the person's scripted.
#[cfg(test)]
pub(crate) mod fake {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::time::Instant;

    use anyhow::{Result, bail};

    use super::{Ask, Session, Work};
    use crate::captcha::{Challenge, Task};
    use crate::web::{Fetch, Fetched, Web};

    /// What was asked of the site.
    #[derive(Debug, Clone)]
    pub struct Asked {
        pub url: String,
        pub fields: Vec<(String, String)>,
        pub body: Option<String>,
    }

    impl Asked {
        pub fn field(&self, name: &str) -> Option<&str> {
            self.fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
        }
    }

    /// A site's answers, in the order they're asked for: each a part of the URL asked and what
    /// came back. A request with no answer left for it fails the test.
    #[derive(Default)]
    pub struct Site {
        answers: RefCell<VecDeque<(String, Fetched)>>,
        twisters: RefCell<VecDeque<serde_json::Value>>,
        pub asked: RefCell<Vec<Asked>>,
        pub widgets: RefCell<Vec<(String, String)>>,
    }

    impl Site {
        /// Answer a request whose URL has `url_part` in it with `status` and `body` (a file
        /// of `tests/fixtures` when it's named `fixture:<file>`).
        pub fn answers(self, url_part: &str, status: u16, body: &str) -> Self {
            let body = match body.strip_prefix("fixture:") {
                Some(file) => std::fs::read_to_string(format!("{}/tests/fixtures/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap(),
                None => body.to_string(),
            };
            self.answers.borrow_mut().push_back((url_part.into(), Fetched { status, body, cookies: String::new() }));
            self
        }

        /// The same, with the page's cookies after it.
        pub fn sets(self, cookies: &str) -> Self {
            if let Some(last) = self.answers.borrow_mut().back_mut() {
                last.1.cookies = cookies.into();
            }
            self
        }

        pub fn twister(self, v: serde_json::Value) -> Self {
            self.twisters.borrow_mut().push_back(v);
            self
        }

        /// What was sent to URLs with `url_part` in them, last first.
        pub fn sent(&self, url_part: &str) -> Asked {
            self.asked.borrow().iter().rev().find(|a| a.url.contains(url_part)).cloned().unwrap_or_else(|| panic!("nothing sent to {url_part}: {:?}", self.asked.borrow()))
        }
    }

    impl Web for Site {
        fn open(&self, _url: &str) -> Result<()> {
            Ok(())
        }

        fn fetch(&self, f: Fetch<'_>) -> Result<Fetched> {
            self.asked.borrow_mut().push(Asked { url: f.url.into(), fields: f.fields, body: f.body });
            let mut answers = self.answers.borrow_mut();
            let Some((part, answer)) = answers.pop_front() else { bail!("the site has no answer left for {}", f.url) };
            assert!(f.url.contains(&part), "asked {} where {part} was next", f.url);
            Ok(answer)
        }

        fn widget(&self, provider: &str, sitekey: &str) -> Result<String> {
            self.widgets.borrow_mut().push((provider.into(), sitekey.into()));
            Ok(format!("{provider}-token"))
        }

        fn fourchan_captcha(&self, _board: &str, _thread: u64) -> Result<serde_json::Value> {
            self.twisters.borrow_mut().pop_front().ok_or_else(|| anyhow::anyhow!("no captcha left"))
        }
    }

    /// A person answering each captcha shown with the next answer (None: "another one").
    #[derive(Default)]
    pub struct Person {
        answers: RefCell<VecDeque<Option<String>>>,
        pub shown: RefCell<Vec<Task>>,
        pub waited: RefCell<Vec<String>>,
    }

    impl Person {
        pub fn says(answers: &[Option<&str>]) -> Self {
            Person { answers: RefCell::new(answers.iter().map(|a| a.map(str::to_string)).collect()), ..Default::default() }
        }
    }

    impl Ask for Person {
        fn solve(&self, challenge: &Challenge) -> Result<Option<String>> {
            self.shown.borrow_mut().push(challenge.task.clone());
            self.answers.borrow_mut().pop_front().ok_or_else(|| anyhow::anyhow!("the person has no answer left"))
        }

        fn wait(&self, _until: Instant, message: &str) -> Result<()> {
            self.waited.borrow_mut().push(message.into());
            Ok(())
        }
    }

    pub fn session<'a>(site: &'a Site, person: &'a Person) -> Session<'a> {
        Session { web: site, ask: person, work: Work::new(None, None) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widgets_are_found_by_their_class_and_sitekey() {
        assert_eq!(widget_in(r#"<div class="h-captcha" data-sitekey="k1"></div>"#), Some(("hcaptcha", "k1".into())));
        assert_eq!(widget_in(r#"<div data-sitekey="k2" class="g-recaptcha x"></div>"#), Some(("recaptcha", "k2".into())));
        assert_eq!(widget_in(r#"<div class="other" data-sitekey="k3"></div>"#), None);
    }

    #[test]
    fn thread_numbers_from_links() {
        assert_eq!(thread_in("/b/res/123.html#456"), Some(123));
        assert_eq!(thread_in("https://a.example/b/thread/7.html"), Some(7));
        assert_eq!(thread_in("/b/index.html"), None);
    }
}
