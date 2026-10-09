//! LynxChan: posts go to `/replyThread.js` or `/newThread.js`, answered in JSON with `?json=1`
//! (`{status, data}`). Its captcha, when the board wants one (`captchaMode`), is a picture to
//! read; its id rides in a cookie. A "block bypass" (when the site answers `bypassable`) is
//! the same captcha, sent to `/renewBypass.js`. Older versions (endchan's) answer the form with
//! a message page instead, linking to the post.

use std::time::Instant;

use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::Value;

use super::{Answered, Draft, Poster, Sent, Where, said, work};
use crate::captcha::{Captcha, Challenge, Task};
use crate::http::encode_segment as enc;
use crate::web::{Fetch, Fetched, Helper, Upload};

pub struct Lynxchan {
    pub root: String,
}

/// The id of the captcha asked for a block bypass rather than a post.
const BYPASS: &str = "bypass";

impl Lynxchan {
    /// A captcha's picture (`/captcha.js` sets its cookie); `id` says what it's for.
    fn fresh_captcha(&self, web: &Helper, id: &str, board: &str) -> Result<Captcha> {
        let r = web.fetch(Fetch::get(&format!("{}/captcha.js?boardUri={}&d={}", self.root, enc(board), super::next_key())))?;
        let key = format!("captcha:lynxchan:{}", super::next_key());
        let task = Task::Text { prompt: "Type the text in the picture".into(), image: Some(key.clone()) };
        Ok(Captcha::Challenge(Challenge { id: id.into(), expires: super::after(Instant::now(), 300), task, pictures: vec![(key, r.image()?)] }))
    }
}

impl Lynxchan {
    /// A new block bypass that has to be validated first (its cookie carries the work: the
    /// session after its 24-character id, then the hash): the number found, sent to
    /// `/validateBypass.js`.
    fn validate(&self, web: &Helper, cookies: &str) -> Result<()> {
        let bypass = cookies.split(';').filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == "bypass").map(|(_, v)| percent_decoded(v)).unwrap_or_default();
        let (Some(session), Some(hash)) = (bypass.get(24..368), bypass.get(368..)) else { return Ok(()) };
        let code = work::lynxchan_bypass(session, hash, VALIDATION_LIMIT).context("The site's block bypass has a proof of work ck didn't find the answer to")?;
        let v = web.fetch(Fetch::post(&format!("{}/validateBypass.js?json=1", self.root), vec![("code".into(), code.to_string())]))?.json()?;
        match v.get("status").and_then(Value::as_str) {
            Some("ok") => Ok(()),
            _ => Err(said(v.get("data").and_then(Value::as_str).unwrap_or("The block bypass wasn't validated"))),
        }
    }

    /// kohlchan's "hashcash" for a block bypass: the secret number an argon2 hash is of.
    fn hashcash(&self, web: &Helper) -> Result<()> {
        let v = web.fetch(Fetch::get(&format!("{}/addon.js/hashcash?action=get&json=1", self.root)))?.json()?;
        let data = v.get("data").cloned().unwrap_or_default();
        if data.get("solved").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }
        let hash = data.get("hash").and_then(Value::as_str).context("The site's proof of work came without its hash")?;
        let difficulty = data.get("difficulty").and_then(crate::http::as_u64).unwrap_or(0).min(VALIDATION_LIMIT);
        let secret = work::argon2_secret(hash, difficulty).context("ck didn't find the answer to the site's proof of work")?;
        let v = web.fetch(Fetch::post(&format!("{}/addon.js/hashcash?action=solve&json=1", self.root), vec![("secret".into(), secret.to_string())]))?.json()?;
        match v.get("status").and_then(Value::as_str) {
            Some("ok") => Ok(()),
            _ => Err(said(v.get("data").and_then(Value::as_str).unwrap_or("The proof of work wasn't taken"))),
        }
    }
}

/// The most numbers a proof of work is searched through.
const VALIDATION_LIMIT: u64 = 1_000_000;

/// A cookie's value with its `%XX` escapes undone.
fn percent_decoded(v: &str) -> String {
    let mut out = Vec::with_capacity(v.len());
    let mut bytes = v.bytes();
    while let Some(b) = bytes.next() {
        let hex = |c: Option<u8>| c.and_then(|c| char::from(c).to_digit(16)).and_then(|d| u8::try_from(d).ok());
        if b == b'%' {
            let mut ahead = bytes.clone();
            if let (Some(hi), Some(lo)) = (hex(ahead.next()), hex(ahead.next())) {
                out.push(hi.saturating_mul(16).saturating_add(lo));
                bytes = ahead;
                continue;
            }
        }
        out.push(b);
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Poster for Lynxchan {
    fn page(&self) -> String {
        format!("{}/robots.txt", self.root)
    }

    fn captcha(&self, web: &Helper, to: &Where) -> Result<Captcha> {
        let board = web.fetch(Fetch::get(&format!("{}/{}/1.json", self.root, enc(&to.board))))?.json()?;
        // A board that doesn't say is asked as if it wanted one.
        let mode = board.get("captchaMode").and_then(Value::as_u64).unwrap_or(2);
        if mode == 2 || (mode == 1 && to.new_thread()) {
            return self.fresh_captcha(web, "post", &to.board);
        }
        Ok(super::no_captcha("post"))
    }

    fn answer(&self, web: &Helper, challenge: &Challenge, answer: &str) -> Result<Answered> {
        if matches!(challenge.task, Task::None) {
            return Ok(Answered::Done(Vec::new()));
        }
        let fields = vec![("captcha".to_string(), answer.trim().to_string())];
        if challenge.id != BYPASS {
            return Ok(Answered::Done(fields));
        }
        let r = web.fetch(Fetch::post(&format!("{}/renewBypass.js?json=1", self.root), fields))?;
        let v = r.json()?;
        match v.get("status").and_then(Value::as_str) {
            Some("ok" | "finish") => self.validate(web, &r.cookies)?,
            Some("hashcash") => self.hashcash(web)?,
            _ => return Err(said(v.get("data").and_then(Value::as_str).unwrap_or("The block bypass wasn't taken"))),
        }
        Ok(Answered::Done(Vec::new()))
    }

    fn post(&self, web: &Helper, to: &Where, draft: &Draft, captcha: &[(String, String)]) -> Result<Sent> {
        let mut fields = vec![("boardUri".to_string(), to.board.clone())];
        if !to.new_thread() {
            fields.push(("threadId".into(), to.thread.to_string()));
        }
        fields.extend([("name".into(), draft.name.clone()), ("email".into(), draft.email.clone()), ("subject".into(), draft.subject.clone()), ("message".into(), draft.comment.clone())]);
        // LynxChan keeps eight characters of it.
        fields.push(("password".into(), draft.password.chars().take(8).collect()));
        if draft.sage() {
            fields.push(("sage".into(), "true".into()));
        }
        if draft.spoiler && draft.file.is_some() {
            fields.push(("spoiler".into(), "true".into()));
        }
        fields.extend(captcha.iter().cloned());
        let form = if to.new_thread() { "newThread" } else { "replyThread" };
        let url = format!("{}/{form}.js?json=1", self.root);
        let referer = match to.thread {
            0 => format!("{}/{}/", self.root, enc(&to.board)),
            t => format!("{}/{}/res/{t}.html", self.root, enc(&to.board)),
        };
        let file = draft.file.as_ref().map(|p| Upload { field: "files".into(), path: p.display().to_string() });
        let r = web.fetch(Fetch { file, ..Fetch::post(&url, fields) }.header("Referer", &referer))?;
        match answered(to, &r)? {
            Answer::Posted(thread, no) => Ok(Sent::Posted { thread, no }),
            Answer::Bypass => Ok(Sent::Again(self.fresh_captcha(web, BYPASS, &to.board)?)),
        }
    }
}

enum Answer {
    Posted(u64, u64),
    /// It wants a block bypass first.
    Bypass,
}

/// What LynxChan answered a post with.
fn answered(to: &Where, r: &Fetched) -> Result<Answer> {
    let Ok(v) = r.json() else { return message_page(r) };
    let data = v.get("data");
    match v.get("status").and_then(Value::as_str) {
        Some("ok") => {
            let no = data.and_then(crate::http::as_u64).context("The site didn't say the post's number")?;
            Ok(Answer::Posted(to.thread_of(no), no))
        }
        Some("bypassable") => Ok(Answer::Bypass),
        Some("hashBan") => bail!("The site has banned that file"),
        Some("maintenance") => bail!("The site is down for maintenance"),
        Some("banned") => bail!("You're banned there{}", data.and_then(|d| d.get("reason")).and_then(Value::as_str).map(|r| format!(": {r}")).unwrap_or_default()),
        _ => Err(said(data.and_then(Value::as_str).unwrap_or("The site didn't take the post"))),
    }
}

/// An older version's answer: its message page, linking to the post when it went up
/// ("Post created", `/b/res/1.html#2`), or saying what went wrong.
fn message_page(r: &Fetched) -> Result<Answer> {
    static LINK: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r#"/res/(\d+)\.html#q?(\d+)"#).ok());
    static LABEL: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r#"id="labelMessage"[^>]*>([^<]*)<"#).ok());
    if r.status < 400
        && let Some(c) = LINK.as_ref().and_then(|re| re.captures(&r.body))
        && let (Some(thread), Some(no)) = (c.get(1).and_then(|m| m.as_str().parse().ok()), c.get(2).and_then(|m| m.as_str().parse().ok()))
    {
        return Ok(Answer::Posted(thread, no));
    }
    match LABEL.as_ref().and_then(|re| re.captures(&r.body)).and_then(|c| c.get(1)) {
        Some(m) => Err(said(m.as_str())),
        None => bail!("The site answered {} with a page ck didn't expect", r.status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetched(status: u16, body: &str) -> Fetched {
        Fetched { status, body: body.into(), cookies: String::new() }
    }

    #[test]
    fn cookies_are_read_unescaped() {
        assert_eq!(percent_decoded("ab%2Bc%3D%3D%zz"), "ab+c==%zz");
    }

    #[test]
    fn what_lynxchan_answers_a_post_with() {
        let reply = Where { site: "k".into(), board: "test".into(), thread: 4 };
        assert!(matches!(answered(&reply, &fetched(200, r#"{"status":"ok","data":10}"#)), Ok(Answer::Posted(4, 10))));
        let new = Where { thread: 0, ..reply.clone() };
        assert!(matches!(answered(&new, &fetched(200, r#"{"status":"ok","data":11}"#)), Ok(Answer::Posted(11, 11))));
        assert!(matches!(answered(&reply, &fetched(200, r#"{"status":"bypassable"}"#)), Ok(Answer::Bypass)));
        assert_eq!(answered(&reply, &fetched(200, r#"{"status":"error","data":"Wrong captcha."}"#)).err().map(|e| e.to_string()).as_deref(), Some("Wrong captcha."));
        // An older version's message page: the post's link, or what went wrong.
        let created = r#"<span id="labelMessage">Post created.</span> <a id="linkRedirect" href="/test/res/6520.html#7934">"#;
        assert!(matches!(answered(&reply, &fetched(200, created)), Ok(Answer::Posted(6520, 7934))));
        let wrong = r#"<title>Error 500</title><span id="labelMessage">Wrong captcha.</span>"#;
        assert_eq!(answered(&reply, &fetched(500, wrong)).err().map(|e| e.to_string()).as_deref(), Some("Wrong captcha."));
    }
}
