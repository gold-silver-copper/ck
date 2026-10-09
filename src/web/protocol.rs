//! What ck and ck-web, its browser helper, say to each other: one JSON object a line, ck's
//! on the helper's stdin and the helper's on its stdout. ck-web includes this file as it is,
//! so it needs nothing but serde.

use serde::{Deserialize, Serialize};

/// What ck asks of the helper. One at a time: each is answered (or fails) before the next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Request {
    /// Go to `url`, a page on the site posted to, so what's sent from it carries the site's
    /// cookies. Answered with `Ready` once it's there; first with `View` frames if the site
    /// wants a person to click (Cloudflare's check, DDoS-Guard's).
    Open { url: String },
    /// Send a request from the page, with its cookies: `body` as it is, or else `fields` (and
    /// a file, by its path) as a form when there are any. A `Referer` header is sent as the
    /// request's referrer. Answered with `Fetched`.
    Fetch { method: String, url: String, headers: Vec<(String, String)>, fields: Vec<(String, String)>, file: Option<Upload>, body: Option<String> },
    /// 4chan's captcha for posting on `board`: a reply in `thread`, or a new thread with 0.
    /// Its captcha comes in a frame of sys.4chan.org, and only to a page of 4chan's. Answered
    /// with `Captcha`; first with `View` frames if the site wants a person to click.
    Captcha { board: String, thread: u64 },
    /// A click on the browser view, in the page's pixels.
    Click { x: u32, y: u32 },
    /// The wheel over the browser view, in the page's pixels: `dy` (down is positive).
    Scroll { x: u32, y: u32, dy: i32 },
    /// Stop: drop what's being done and the browser view.
    Cancel,
}

/// A file sent with a form: its path, and the form field it goes in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Upload {
    pub field: String,
    pub path: String,
}

/// What the helper tells ck.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "is", rename_all = "snake_case")]
pub enum Reply {
    /// The page asked for is open.
    Ready,
    /// The answer to a `Fetch`: its status, and its body: text, or base64 for an image.
    Fetched { status: u16, body: String },
    /// 4chan's captcha, as its captcha page gave it (the "twister").
    Captcha { twister: serde_json::Value },
    /// The browser view, while the site wants a person (Cloudflare's check, hCaptcha): the
    /// part of the page with something on it, as a PNG (base64), from `left`, `top` in the
    /// page's pixels.
    View { png: String, left: u32, top: u32 },
    /// What went wrong.
    Failed { error: String },
}
