//! What ck and ck-web, its browser helper, say to each other: one JSON object a line, ck's
//! on the helper's stdin and the helper's on its stdout. ck-web includes this file as it is,
//! so it needs nothing but serde.

use serde::{Deserialize, Serialize};

/// What ck asks of the helper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Request {
    /// A captcha for posting on `board`: a reply in `thread`, or a new thread with 0.
    /// Answered with `Captcha`; first with `View` frames if the site wants a person to click.
    Captcha { board: String, thread: u64 },
    /// Send a post: the form's fields and the solved captcha's (`t-challenge`, `t-response`),
    /// and a file by its path.
    Post { board: String, thread: u64, fields: Vec<(String, String)>, file: Option<String> },
    /// A click on the browser view, in the page's pixels.
    Click { x: u32, y: u32 },
    /// The wheel over the browser view, in the page's pixels: `dy` (down is positive).
    Scroll { x: u32, y: u32, dy: i32 },
    /// Stop: drop the captcha being loaded and the browser view.
    Cancel,
}

/// What the helper tells ck.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "is", rename_all = "snake_case")]
pub enum Reply {
    /// The browser is up and on the site.
    Ready,
    /// The site's captcha, as its captcha page gave it (4chan's "twister").
    Captcha { twister: serde_json::Value },
    /// The browser view, while the site wants a person (Cloudflare's check, hCaptcha): the
    /// part of the page with something on it, as a PNG (base64), from `left`, `top` in the
    /// page's pixels.
    View { png: String, left: u32, top: u32 },
    /// The post went up: its thread and number.
    Posted { thread: u64, no: u64 },
    /// What went wrong, in the site's words when it said.
    Failed { error: String },
}
