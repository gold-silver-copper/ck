//! LynxChan: posts go to `/replyThread.js` or `/newThread.js`, answered in JSON with `?json=1`
//! (`{status, data}`). Its captcha, when the board wants one (`captchaMode`), is a picture to
//! read; its id rides in a cookie. A "block bypass" (when the site answers `bypassable`) is
//! the same captcha, sent to `/renewBypass.js`. Older versions (endchan's) answer the form with
//! a message page instead, linking to the post.

use std::sync::LazyLock;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::Value;

use super::{Draft, Posted, Poster, Session, Where, said, work};
use crate::captcha::{Challenge, Task};
use crate::http::encode_segment as enc;
use crate::web::{Fetch, Fetched, Upload};

pub struct Lynxchan {
    pub root: String,
}

impl Lynxchan {
    /// The site's captcha, answered: its picture from `/captcha.js` (which sets its cookie),
    /// asked for again until there's an answer.
    fn captcha(&self, s: &Session, board: &str) -> Result<String> {
        let (_, answer) = s.solve(|| {
            let r = s.web.fetch(Fetch::get(&format!("{}/captcha.js?boardUri={}&d={}", self.root, enc(board), super::next_key())))?;
            let key = format!("captcha:lynxchan:{}", super::next_key());
            let task = Task::Text { prompt: "Type the text in the picture".into(), image: Some(key.clone()) };
            Ok(Challenge { id: String::new(), expires: super::after(Instant::now(), 300), task, pictures: vec![(key, r.image()?)] })
        })?;
        Ok(answer.trim().to_string())
    }

    /// A block bypass: the captcha, sent to `/renewBypass.js`, and the proof of work it may
    /// come with (alogs' validation, kohlchan's hashcash).
    fn bypass(&self, s: &Session, board: &str) -> Result<()> {
        let answer = self.captcha(s, board)?;
        let r = s.web.fetch(Fetch::post(&format!("{}/renewBypass.js?json=1", self.root), vec![("captcha".into(), answer)]))?;
        let v = r.json()?;
        match v.get("status").and_then(Value::as_str) {
            Some("ok" | "finish") => self.validate(s, &r.cookies),
            Some("hashcash") => self.hashcash(s),
            _ => Err(said(v.get("data").and_then(Value::as_str).unwrap_or("The block bypass wasn't taken"))),
        }
    }

    /// A new block bypass that has to be validated first (its cookie carries the work: the
    /// session after its 24-character id, then the hash): the number found, sent to
    /// `/validateBypass.js`.
    fn validate(&self, s: &Session, cookies: &str) -> Result<()> {
        let bypass = cookies.split(';').filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == "bypass").map(|(_, v)| percent_decoded(v)).unwrap_or_default();
        let (Some(session), Some(hash)) = (bypass.get(24..368), bypass.get(368..)) else { return Ok(()) };
        let code = work::lynxchan_bypass(s.work, session, hash).context("ck didn't find the answer to the site's proof of work in time (proof_of_work_seconds)")?;
        let v = s.web.fetch(Fetch::post(&format!("{}/validateBypass.js?json=1", self.root), vec![("code".into(), code.to_string())]))?.json()?;
        match v.get("status").and_then(Value::as_str) {
            Some("ok") => Ok(()),
            _ => Err(said(v.get("data").and_then(Value::as_str).unwrap_or("The block bypass wasn't validated"))),
        }
    }

    /// kohlchan's "hashcash" for a block bypass: the secret number an argon2 hash is of.
    fn hashcash(&self, s: &Session) -> Result<()> {
        let v = s.web.fetch(Fetch::get(&format!("{}/addon.js/hashcash?action=get&json=1", self.root)))?.json()?;
        let data = v.get("data").cloned().unwrap_or_default();
        if data.get("solved").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }
        let hash = data.get("hash").and_then(Value::as_str).context("The site's proof of work came without its hash")?;
        let difficulty = data.get("difficulty").and_then(crate::http::as_u64).unwrap_or(0);
        let secret = work::argon2_secret(s.work, hash, difficulty).context("ck didn't find the answer to the site's proof of work in time (proof_of_work_seconds)")?;
        let v = s.web.fetch(Fetch::post(&format!("{}/addon.js/hashcash?action=solve&json=1", self.root), vec![("secret".into(), secret.to_string())]))?.json()?;
        match v.get("status").and_then(Value::as_str) {
            Some("ok") => Ok(()),
            _ => Err(said(v.get("data").and_then(Value::as_str).unwrap_or("The proof of work wasn't taken"))),
        }
    }
}

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

    fn post(&self, s: &Session, to: &Where, draft: &Draft) -> Result<Posted> {
        let board = s.web.fetch(Fetch::get(&format!("{}/{}/1.json", self.root, enc(&to.board))))?.json()?;
        // A board that doesn't say is asked as if it wanted one.
        let mode = board.get("captchaMode").and_then(Value::as_u64).unwrap_or(2);
        let wants = mode == 2 || (mode == 1 && to.new_thread());
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
        let form = if to.new_thread() { "newThread" } else { "replyThread" };
        let url = format!("{}/{form}.js?json=1", self.root);
        let referer = match to.thread {
            0 => format!("{}/{}/", self.root, enc(&to.board)),
            t => format!("{}/{}/res/{t}.html", self.root, enc(&to.board)),
        };
        let send = |captcha: Option<String>| {
            let file = draft.file.as_ref().map(|p| Upload { field: "files".into(), path: p.display().to_string() });
            let fields = [fields.as_slice(), captcha.map(|c| ("captcha".to_string(), c)).as_slice()].concat();
            s.web.fetch(Fetch { file, ..Fetch::post(&url, fields) }.header("Referer", &referer))
        };
        let captcha = if wants { Some(self.captcha(s, &to.board)?) } else { None };
        let (thread, no) = match answered(to, &send(captcha)?)? {
            Answer::Posted(thread, no) => (thread, no),
            // A block bypass first, then the post again (each captcha is taken once).
            Answer::Bypass => {
                self.bypass(s, &to.board)?;
                let captcha = if wants { Some(self.captcha(s, &to.board)?) } else { None };
                match answered(to, &send(captcha)?)? {
                    Answer::Posted(thread, no) => (thread, no),
                    Answer::Bypass => bail!("The site still wants a block bypass"),
                }
            }
        };
        Ok(Posted { thread, no })
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
/// ("Post created", `/b/res/1.html#2`), or its error page, saying what went wrong.
fn message_page(r: &Fetched) -> Result<Answer> {
    static LINK: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r#"/res/(\d+)\.html#q?(\d+)"#).ok());
    static LABEL: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r#"id="(?:labelMessage|errorLabel)"[^>]*>([^<]*)<"#).ok());
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
    fn a_reply_with_its_captcha_through_a_block_bypass_and_endchans_page() {
        use super::super::fake::{Person, Site, session};
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(30, 10).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let picture = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png.into_inner());
        let site = Site::default()
            .answers("/test/1.json", 200, r#"{"captchaMode":2,"threads":[]}"#)
            .answers("/captcha.js", 200, &picture)
            .answers("/replyThread.js?json=1", 200, r#"{"status":"bypassable"}"#)
            .answers("/captcha.js", 200, &picture)
            // No bypass cookie readable: nothing to validate.
            .answers("/renewBypass.js?json=1", 200, r#"{"status":"ok","data":null}"#)
            .answers("/captcha.js", 200, &picture)
            .answers("/replyThread.js?json=1", 200, "fixture:lynxchan_endchan_post_created.html");
        let person = Person::says(&[Some("f61801"), Some(" bypas "), Some("again1")]);
        let to = Where { site: "endchan".into(), board: "test".into(), thread: 6520 };
        let draft = Draft { comment: "hi".into(), password: "a long password".into(), ..Default::default() };
        let lynx = Lynxchan { root: "https://endchan.net".into() };
        assert_eq!(lynx.post(&session(&site, &person), &to, &draft).unwrap(), Posted { thread: 6520, no: 7934 });
        assert_eq!(site.sent("/renewBypass.js").field("captcha"), Some("bypas"));
        let sent = site.sent("/replyThread.js");
        assert_eq!((sent.field("captcha"), sent.field("password"), sent.field("threadId")), (Some("again1"), Some("a long p"), Some("6520")));
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
        let wrong = std::fs::read_to_string("tests/fixtures/lynxchan_endchan_error.html").unwrap();
        assert_eq!(answered(&reply, &fetched(500, &wrong)).err().map(|e| e.to_string()).as_deref(), Some("Either a message or a file is required."));
    }
}
