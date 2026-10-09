//! vichan (and tinyboard, infinity): the post form of the thread's page (or the board's, for a
//! new thread) is sent back whole, hidden anti-spam fields and all, to `post.php`, with that
//! page as the referrer, as vichan checks. Its own captcha, where a board has one, is a
//! picture to read.

use std::sync::LazyLock;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::Value;

use super::form::Form;
use super::{Answered, Draft, Poster, Sent, Where, said, thread_in};
use crate::captcha::{Captcha, Challenge, Task, inline_image};
use crate::http::encode_segment as enc;
use crate::web::{Fetch, Helper, Upload};

pub struct Vichan {
    pub root: String,
}

impl Vichan {
    /// The page with the post form for `to`.
    fn form_page(&self, to: &Where) -> String {
        match to.thread {
            0 => format!("{}/{}/index.html", self.root, enc(&to.board)),
            t => format!("{}/{}/res/{t}.html", self.root, enc(&to.board)),
        }
    }

    /// The post form for `to`, and its page's HTML.
    fn form(&self, web: &Helper, to: &Where) -> Result<(String, Form, String)> {
        let url = self.form_page(to);
        let page = web.fetch(Fetch::get(&url))?;
        if page.status >= 400 {
            bail!("The site answered {} for {url}", page.status);
        }
        let form = Form::find(&page.body, "post").context("The page has no post form: the board may be locked")?;
        Ok((url, form, page.body))
    }
}

impl Poster for Vichan {
    fn page(&self) -> String {
        format!("{}/robots.txt", self.root)
    }

    fn captcha(&self, web: &Helper, to: &Where) -> Result<Captcha> {
        let (_, form, html) = self.form(web, to)?;
        if let Some((provider, extra)) = native_captcha(&html) {
            let r = web.fetch(Fetch::get(&format!("{provider}?mode=get&extra={extra}")))?;
            return native(&r.json()?, Instant::now());
        }
        if form.has("h-captcha-response") || form.has("g-recaptcha-response") || html.contains("class=\"h-captcha\"") || html.contains("class=\"g-recaptcha\"") {
            return Ok(Captcha::Refused("This board asks for hCaptcha or reCAPTCHA, which ck can't show yet: post from the browser".into()));
        }
        Ok(super::no_captcha(""))
    }

    fn answer(&self, _web: &Helper, challenge: &Challenge, answer: &str) -> Result<Answered> {
        if challenge.id.is_empty() {
            return Ok(Answered::Done(Vec::new()));
        }
        Ok(Answered::Done(vec![("captcha_cookie".into(), challenge.id.clone()), ("captcha_text".into(), answer.to_lowercase())]))
    }

    fn post(&self, web: &Helper, to: &Where, draft: &Draft, captcha: &[(String, String)]) -> Result<Sent> {
        let (referer, mut form, _) = self.form(web, to)?;
        let submit = form.submit.clone().unwrap_or_else(|| if to.new_thread() { "New Topic" } else { "New Reply" }.into());
        form.set("name", &draft.name);
        form.set("email", &draft.email);
        if to.new_thread() {
            form.set("subject", &draft.subject);
        }
        form.set("body", &draft.comment);
        form.set("password", &draft.password);
        if draft.spoiler && draft.file.is_some() {
            form.set("spoiler", "on");
        }
        for (k, v) in captcha {
            form.set(k, v.as_str());
        }
        form.set("json_response", "1");
        form.set("post", submit);
        let file = draft.file.as_ref().map(|p| Upload { field: form.file.clone().unwrap_or_else(|| "file".into()), path: p.display().to_string() });
        let url = format!("{}/post.php", self.root);
        let r = web.fetch(Fetch { file, ..Fetch::post(&url, form.fields) }.header("Referer", &referer))?;
        let (thread, no) = posted(to, &r.json()?)?;
        Ok(Sent::Posted { thread, no })
    }
}

/// The thread and number of a post vichan took, or its error.
fn posted(to: &Where, v: &Value) -> Result<(u64, u64)> {
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(said(e));
    }
    let no = v.get("id").and_then(crate::http::as_u64).context("The site didn't say the post's number")?;
    let thread = v.get("redirect").and_then(Value::as_str).and_then(thread_in).unwrap_or_else(|| to.thread_of(no));
    Ok((thread, no))
}

/// Where a page loads vichan's own captcha from: its provider and the letters it draws from
/// (`load_captcha("/8chan-captcha/entrypoint.php", "abc…")`).
fn native_captcha(html: &str) -> Option<(String, String)> {
    static LOAD: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r#"load_captcha\(\s*["']([^"']+)["']\s*,\s*["']([^"']*)["']"#).ok());
    let c = LOAD.as_ref()?.captures(html)?;
    Some((c.get(1)?.as_str().to_string(), c.get(2)?.as_str().to_string()))
}

/// vichan's captcha: `{cookie, captchahtml: <img src="data:…">, expires_in}`.
fn native(v: &Value, now: Instant) -> Result<Captcha> {
    let cookie = v.get("cookie").and_then(Value::as_str).context("The captcha didn't come")?;
    let picture = v.get("captchahtml").and_then(Value::as_str).and_then(inline_image).context("The captcha's picture didn't come")?;
    let secs = v.get("expires_in").and_then(Value::as_u64).unwrap_or(120);
    let key = format!("captcha:vichan:{}", super::next_key());
    let task = Task::Text { prompt: "Type the letters in the picture".into(), image: Some(key.clone()) };
    Ok(Captcha::Challenge(Challenge { id: cookie.to_string(), expires: super::after(now, secs), task, pictures: vec![(key, picture)] }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_new_thread_form_loads_its_captcha_and_posts_answer_json() {
        let html = std::fs::read_to_string("tests/fixtures/vichan_smugloli_new_thread_form.html").unwrap();
        assert_eq!(native_captcha(&html), Some(("/8chan-captcha/entrypoint.php".into(), "abcdefghijklmnopqrstuvwxyz".into())));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png.into_inner());
        let Ok(Captcha::Challenge(c)) = native(&json!({"cookie": "k1", "captchahtml": format!("<image src=\"data:image/png;base64,{b64}\">"), "expires_in": 120}), Instant::now()) else { panic!("no captcha") };
        assert!(matches!(&c.task, Task::Text { image: Some(k), .. } if k.starts_with("captcha:")));
        assert_eq!(c.pictures.len(), 1);
        let to = Where { site: "s".into(), board: "a".into(), thread: 0 };
        assert_eq!(posted(&to, &json!({"redirect": "/a/res/12.html", "noko": true, "id": 12})).unwrap(), (12, 12));
        let reply = Where { thread: 5, ..to };
        assert_eq!(posted(&reply, &json!({"redirect": "/a/res/5.html#13", "id": 13})).unwrap(), (5, 13));
        assert_eq!(posted(&reply, &json!({"error": "Your request looks automated; Post discarded."})).unwrap_err().to_string(), "Your request looks automated; Post discarded.");
    }
}
