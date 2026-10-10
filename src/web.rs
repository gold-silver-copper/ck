//! ck-web, the browser helper ck posts to 4chan through (see `ck-web/`): finding it,
//! starting it, and talking to it.

pub mod install;
mod protocol;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
pub use protocol::{Reply, Request, Upload};

const NAME: &str = "ck-web";

/// What to say when ck-web answers in a way this ck can't read.
const STALE: &str = "ck-web answered in a way this ck doesn't understand: it's another version. \
                     If you built it, build it again (cargo build --release -p ck-web)";

/// A running ck-web, shared by the threads that ask it things. It stops when the last clone
/// is dropped (its stdin closes, and it's killed).
#[derive(Clone)]
pub struct Helper(Arc<Shared>);

struct Shared {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    /// Who's waiting for an answer: one asker at a time (`turn`).
    waiter: Arc<Mutex<Option<Sender<Option<Reply>>>>>,
    turn: Mutex<()>,
    /// The page it's on, as last opened.
    page: Mutex<Option<String>>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        let child = self.child.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Helper {
    /// Start the helper at `path`. The browser view, while the site wants a person, goes to
    /// `on_view` (on a thread of the helper's); `on_stop` hears it's ended, with the last thing
    /// it printed (a missing library, a crash).
    pub fn start(path: &Path, on_view: impl Fn(Reply) + Send + 'static, on_stop: impl FnOnce(Option<String>) + Send + 'static) -> Result<Helper> {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Couldn't start {}", path.display()))?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            bail!("Couldn't talk to {}", path.display());
        };
        let waiter: Arc<Mutex<Option<Sender<Option<Reply>>>>> = Arc::default();
        let last_words = std::thread::spawn(move || BufReader::new(stderr).lines().map_while(Result::ok).filter(|l| !l.trim().is_empty()).last());
        let answers = waiter.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // A line this ck can't read is from a ck-web of another version: what's asked
                // fails, rather than waiting on an answer that won't come.
                let reply = serde_json::from_str(&line).unwrap_or_else(|_| Reply::Failed { error: STALE.into() });
                match reply {
                    view @ Reply::View { .. } => on_view(view),
                    reply => {
                        if let Some(tx) = crate::http::lock(&answers).take() {
                            let _ = tx.send(Some(reply));
                        }
                    }
                }
            }
            if let Some(tx) = crate::http::lock(&answers).take() {
                let _ = tx.send(None);
            }
            on_stop(last_words.join().ok().flatten());
        });
        Ok(Helper(Arc::new(Shared { child: Mutex::new(child), stdin: Mutex::new(stdin), waiter, turn: Mutex::new(()), page: Mutex::new(None) })))
    }

    /// Ask, and wait for the answer. A `Failed` comes back as an error, as does ck-web
    /// stopping or the asking being cancelled.
    fn ask(&self, request: &Request) -> Result<Reply> {
        let _turn = crate::http::lock(&self.0.turn);
        let (tx, rx) = channel();
        *crate::http::lock(&self.0.waiter) = Some(tx);
        self.send(request)?;
        match rx.recv() {
            Ok(Some(Reply::Failed { error })) => bail!(error),
            Ok(Some(reply)) => Ok(reply),
            Ok(None) | Err(_) => bail!("ck-web stopped"),
        }
    }

    /// Tell ck-web without waiting (a click, the wheel); stopping what's being asked fails it.
    pub fn send(&self, request: &Request) -> Result<()> {
        if matches!(request, Request::Cancel) {
            *crate::http::lock(&self.0.page) = None;
            if let Some(tx) = crate::http::lock(&self.0.waiter).take() {
                let _ = tx.send(Some(Reply::Failed { error: "Cancelled".into() }));
            }
        }
        let line = serde_json::to_string(request)?;
        let mut stdin = crate::http::lock(&self.0.stdin);
        writeln!(stdin, "{line}").and_then(|()| stdin.flush()).context("ck-web has stopped")
    }
}

/// What a post goes through to its site: ck-web (`Helper`), or answers recorded from the
/// site (tests). Each call waits for the answer.
pub trait Web {
    /// Be on `url` (a page of the site posted to), opening it if it isn't the one open.
    fn open(&self, url: &str) -> Result<()>;
    /// Send a request from the page: what came back.
    fn fetch(&self, f: Fetch<'_>) -> Result<Fetched>;
    /// A captcha widget of a service's, done by a person in the browser view: its token.
    fn widget(&self, provider: &str, sitekey: &str) -> Result<String>;
    /// 4chan's captcha (its "twister"): it comes only in a frame of sys.4chan.org.
    fn fourchan_captcha(&self, board: &str, thread: u64) -> Result<serde_json::Value>;
}

impl Web for Helper {
    fn open(&self, url: &str) -> Result<()> {
        if crate::http::lock(&self.0.page).as_deref() == Some(url) {
            return Ok(());
        }
        self.ask(&Request::Open { url: url.to_string() })?;
        *crate::http::lock(&self.0.page) = Some(url.to_string());
        Ok(())
    }

    fn fetch(&self, f: Fetch<'_>) -> Result<Fetched> {
        let headers = f.headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let request = Request::Fetch { method: f.method.into(), url: f.url.into(), headers, fields: f.fields, file: f.file, body: f.body };
        match self.ask(&request)? {
            Reply::Fetched { status, body, cookies } => Ok(Fetched { status, body, cookies }),
            other => bail!("ck-web answered {other:?}"),
        }
    }

    fn widget(&self, provider: &str, sitekey: &str) -> Result<String> {
        match self.ask(&Request::Widget { provider: provider.into(), sitekey: sitekey.into() })? {
            Reply::Token { token } => Ok(token),
            other => bail!("ck-web answered {other:?}"),
        }
    }

    fn fourchan_captcha(&self, board: &str, thread: u64) -> Result<serde_json::Value> {
        match self.ask(&Request::Captcha { board: board.into(), thread })? {
            Reply::Captcha { twister } => Ok(twister),
            other => bail!("ck-web answered {other:?}"),
        }
    }
}

/// A request for `Helper::fetch`: a GET with nothing, unless said otherwise.
#[derive(Debug, Clone, Default)]
pub struct Fetch<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: Vec<(&'a str, &'a str)>,
    /// A form: its fields, and a file.
    pub fields: Vec<(String, String)>,
    pub file: Option<Upload>,
    /// Or a body as it is (with its `Content-Type` among the headers).
    pub body: Option<String>,
}

impl<'a> Fetch<'a> {
    pub fn get(url: &'a str) -> Self {
        Fetch { method: "GET", url, ..Default::default() }
    }

    pub fn post(url: &'a str, fields: Vec<(String, String)>) -> Self {
        Fetch { method: "POST", url, fields, ..Default::default() }
    }

    pub fn header(mut self, name: &'a str, value: &'a str) -> Self {
        self.headers.push((name, value));
        self
    }
}

/// What a `fetch` got back.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub status: u16,
    /// Text, or base64 for an image.
    pub body: String,
    /// The page's cookies after it, those its scripts can read.
    pub cookies: String,
}

impl Fetched {
    /// The body as JSON, or why it isn't.
    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_str(&self.body).with_context(|| format!("the site answered {} with {}", self.status, crate::markup::strip_tags(&self.body).chars().take(200).collect::<String>()))
    }

    /// The page's cookie `name`.
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies.split(';').filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
    }

    /// The body as an image.
    pub fn image(&self) -> Result<image::DynamicImage> {
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &self.body).context("the image didn't come through")?;
        image::load_from_memory(&bytes).context("not an image")
    }
}

/// Where ck-web is: the configured path, else as downloaded, else next to ck, else on the
/// PATH. Tests find none.
pub fn find(configured: Option<&str>) -> Option<PathBuf> {
    if crate::sandboxed() {
        return None;
    }
    if let Some(p) = configured {
        return Some(crate::config::expand_home(p));
    }
    if let Some(p) = install::installed() {
        return Some(p);
    }
    let exe = format!("{NAME}{}", std::env::consts::EXE_SUFFIX);
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(&exe)));
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).map(|d| d.join(&exe));
    beside.into_iter().chain(on_path).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // page.js writes its replies by hand.
    fn replies_read_as_page_js_writes_them() {
        let fetched: Reply = serde_json::from_str(r#"{"is":"fetched","status":200,"body":"hi","cookies":"a=1"}"#).unwrap();
        assert!(matches!(fetched, Reply::Fetched { status: 200, .. }));
    }
}
