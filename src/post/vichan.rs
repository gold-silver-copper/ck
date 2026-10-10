//! vichan (and tinyboard, infinity): the post form of the thread's page (or the board's, for a
//! new thread) is sent back whole, hidden anti-spam fields and all, to `post.php`, with that
//! page as the referrer, as vichan checks. Its own captcha, where a board has one, is a
//! picture to read; a service's (hCaptcha, reCAPTCHA…) is done in the browser view. 8kun
//! posts to its `sys` host, and asks for its own captcha ("popcaptcha") once a day.

use std::sync::LazyLock;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::Value;

use super::form::Form;
use super::{Draft, Posted, Poster, Session, Where, said, thread_in};
use crate::captcha::{Challenge, Task, inline_image};
use crate::http::encode_segment as enc;
use crate::web::{Fetch, Upload};

pub struct Vichan {
    pub root: String,
    /// Where posts go, when not to the site itself (8kun's `https://sys.8kun.top`).
    pub sys: Option<String>,
}

/// The form field each service's token goes in.
fn token_field(provider: &str) -> &'static str {
    match provider {
        "hcaptcha" => "h-captcha-response",
        "turnstile" => "cf-turnstile-response",
        "yandex" => "smart-token",
        _ => "g-recaptcha-response",
    }
}

impl Vichan {
    /// Where posts (and 8kun's captcha) go.
    fn sys(&self) -> &str {
        self.sys.as_deref().unwrap_or(&self.root)
    }

    /// The fields the page's captcha adds to the form, once it's done: vichan's own (a
    /// picture to read), a service's widget, or leftypol's hCaptcha (once its cookie says the
    /// site has flagged you, its sitekey in its script).
    fn captcha(&self, s: &Session, form: &Form, page: &crate::web::Fetched) -> Result<Vec<(String, String)>> {
        if let Some((provider, extra)) = native_captcha(&page.body) {
            let (challenge, answer) = s.solve(|| native(&s.web.fetch(Fetch::get(&format!("{provider}?mode=get&extra={extra}")))?.json()?, Instant::now()))?;
            return Ok(vec![("captcha_cookie".into(), challenge.id), ("captcha_text".into(), answer.to_lowercase())]);
        }
        if let Some((provider, sitekey)) = super::widget_in(&page.body) {
            return Ok(vec![(token_field(provider).into(), s.web.widget(provider, &sitekey)?)]);
        }
        if form.has("captcha-response") && page.cookie("captcha-required") == Some("1") {
            let script = s.web.fetch(Fetch::get(&format!("{}/main.js", self.root)))?.body;
            let sitekey = sitekey_in(&script).context("The site wants hCaptcha, and ck couldn't find its key")?;
            let token = s.web.widget("hcaptcha", &sitekey)?;
            return Ok(vec![("captcha-response".into(), token.clone()), ("h-captcha-response".into(), token), ("captcha-form-id".into(), super::random_password())]);
        }
        Ok(Vec::new())
    }

    /// 8kun's daily captcha, answered in its popup until it's taken.
    fn popcaptcha(&self, s: &Session) -> Result<()> {
        let url = format!("{}/captcha-post-popup.php", self.sys());
        let mut html = s.web.fetch(Fetch::get(&url))?.body;
        loop {
            let challenge = popcaptcha(&html, Instant::now())?;
            let Some(answer) = s.ask.solve(&challenge)? else {
                html = s.web.fetch(Fetch::get(&url))?.body;
                continue;
            };
            let fields = vec![("captcha_phrase".to_string(), answer.trim().to_string()), ("captcha_key".into(), challenge.id), ("agreed".into(), "true".into())];
            let v = s.web.fetch(Fetch::post(&url, fields))?.json()?;
            if v.get("status").and_then(crate::http::as_u64) == Some(1) {
                return Ok(());
            }
            html = v.get("new_captcha").and_then(Value::as_str).context("The captcha wasn't taken")?.to_string();
        }
    }
}

impl Poster for Vichan {
    fn page(&self) -> String {
        format!("{}/robots.txt", self.root)
    }

    fn post(&self, s: &Session, to: &Where, draft: &Draft) -> Result<Posted> {
        let referer = match to.thread {
            0 => format!("{}/{}/index.html", self.root, enc(&to.board)),
            t => format!("{}/{}/res/{t}.html", self.root, enc(&to.board)),
        };
        let page = s.web.fetch(Fetch::get(&referer))?;
        if page.status >= 400 {
            bail!("The site answered {} for {referer}", page.status);
        }
        let mut form = Form::find(&page.body, "post").context("The page has no post form: the board may be locked")?;
        for (k, v) in self.captcha(s, &form, &page)? {
            form.set(&k, v);
        }
        if let Some(sys) = &self.sys {
            // As 8kun's own script sends it.
            form.set("domain_name_post", sys.trim_start_matches("https://sys."));
        }
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
        form.set("json_response", "1");
        form.set("post", submit);
        let field = form.file.clone().unwrap_or_else(|| "file".into());
        let url = format!("{}/post.php", self.sys());
        let send = || {
            let file = draft.file.as_ref().map(|p| Upload { field: field.clone(), path: p.display().to_string() });
            s.web.fetch(Fetch { file, ..Fetch::post(&url, form.fields.clone()) }.header("Referer", &referer))
        };
        let mut r = send()?;
        // 8kun's captcha demands, told by how the answer starts: its own is answered, and the
        // post sent again.
        if r.body.starts_with("{\"popcaptcha\":true") {
            self.popcaptcha(s)?;
            r = send()?;
        }
        if ["{\"hcaptcha\":true", "{\"k_n_m_n\":true", "{\"captcha_thunder\":true", "{\"captcha\":true"].iter().any(|p| r.body.starts_with(p)) {
            bail!("The site wants a check ck can't show yet: post from the browser");
        }
        let (thread, no) = posted(to, &r.json()?)?;
        Ok(Posted { thread, no })
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

/// 8kun's daily captcha, from its popup's HTML: a picture (inline), and its key as the id.
fn popcaptcha(html: &str, now: Instant) -> Result<Challenge> {
    let picture = inline_image(html).context("8kun's captcha picture didn't come")?;
    let key = super::form::Form::find(&format!("<form name=\"c\">{html}</form>"), "c").and_then(|f| f.fields.into_iter().find(|(k, _)| k == "captcha_key")).map(|(_, v)| v).context("8kun's captcha came without its key")?;
    let at = format!("captcha:vichan:{}", super::next_key());
    let task = Task::Text { prompt: "Type the text in the picture (8kun asks once a day)".into(), image: Some(at.clone()) };
    Ok(Challenge { id: key, expires: super::after(now, 600), task, pictures: vec![(at, picture)] })
}

/// An hCaptcha sitekey in a site's script (`sitekey: "…"`).
fn sitekey_in(script: &str) -> Option<String> {
    static KEY: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r#"sitekey["']?\s*:\s*["']([0-9a-f-]{20,})["']"#).ok());
    KEY.as_ref()?.captures(script)?.get(1).map(|m| m.as_str().to_string())
}

/// Where a page loads vichan's own captcha from: its provider and the letters it draws from
/// (`load_captcha("/8chan-captcha/entrypoint.php", "abc…")`).
fn native_captcha(html: &str) -> Option<(String, String)> {
    static LOAD: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r#"load_captcha\(\s*["']([^"']+)["']\s*,\s*["']([^"']*)["']"#).ok());
    let c = LOAD.as_ref()?.captures(html)?;
    Some((c.get(1)?.as_str().to_string(), c.get(2)?.as_str().to_string()))
}

/// vichan's captcha: `{cookie, captchahtml: <img src="data:…">, expires_in}`; its cookie as
/// the id.
fn native(v: &Value, now: Instant) -> Result<Challenge> {
    let cookie = v.get("cookie").and_then(Value::as_str).context("The captcha didn't come")?;
    let picture = v.get("captchahtml").and_then(Value::as_str).and_then(inline_image).context("The captcha's picture didn't come")?;
    let secs = v.get("expires_in").and_then(Value::as_u64).unwrap_or(120);
    let key = format!("captcha:vichan:{}", super::next_key());
    let task = Task::Text { prompt: "Type the letters in the picture".into(), image: Some(key.clone()) };
    Ok(Challenge { id: cookie.to_string(), expires: super::after(now, secs), task, pictures: vec![(key, picture)] })
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Person, Site, session};
    use super::*;

    fn png_b64() -> String {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2).write_to(&mut png, image::ImageFormat::Png).unwrap();
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png.into_inner())
    }

    fn draft() -> Draft {
        Draft { comment: "hi".into(), password: "pw".into(), ..Default::default() }
    }

    #[test]
    fn a_reply_sends_the_form_back_whole() {
        let site = Site::default().answers("/wiz/res/230170.html", 200, "fixture:vichan_wizchan_form.html").answers("/post.php", 200, r#"{"redirect":"/wiz/res/230170.html#230200","noko":false,"id":230200}"#);
        let to = Where { site: "wizchan".into(), board: "wiz".into(), thread: 230170 };
        let v = Vichan { root: "https://wizchan.org".into(), sys: None };
        assert_eq!(v.post(&session(&site, &Person::default()), &to, &draft()).unwrap(), Posted { thread: 230170, no: 230200 });
        let sent = site.sent("/post.php");
        // Its hidden fields as they were, its own button, the post's fields filled in.
        assert_eq!(sent.field("lastname"), Some("⛁epu,⛠XfjUlI-(HzBK"));
        assert_eq!(sent.field("hash"), Some("e7759aaa77dd0d0dee866362f39cfb734acaedfe"));
        assert_eq!((sent.field("post"), sent.field("body"), sent.field("password"), sent.field("email")), (Some("New Wisdom"), Some("hi"), Some("pw"), Some("")));
        assert_eq!(sent.field("thread"), Some("230170"));
    }

    #[test]
    fn a_new_thread_answers_the_boards_captcha() {
        let captcha = format!(r#"{{"cookie":"k1","captchahtml":"<image src=\"data:image/png;base64,{}\">","expires_in":120}}"#, png_b64());
        let site = Site::default()
            .answers("/a/index.html", 200, "fixture:vichan_smugloli_new_thread_form.html")
            .answers("/8chan-captcha/entrypoint.php?mode=get", 200, &captcha.replace("k1", "k0"))
            .answers("/8chan-captcha/entrypoint.php?mode=get", 200, &captcha)
            .answers("/post.php", 200, r#"{"redirect":"/a/res/12.html","noko":true,"id":12}"#);
        // The first picture is passed over for another.
        let person = Person::says(&[None, Some("AbC")]);
        let to = Where { site: "smug".into(), board: "a".into(), thread: 0 };
        let v = Vichan { root: "https://smuglo.li".into(), sys: None };
        assert_eq!(v.post(&session(&site, &person), &to, &draft()).unwrap(), Posted { thread: 12, no: 12 });
        let sent = site.sent("/post.php");
        assert_eq!((sent.field("captcha_cookie"), sent.field("captcha_text"), sent.field("post")), (Some("k1"), Some("abc"), Some("New Topic")));
    }

    #[test]
    fn eightkuns_daily_captcha_then_the_post_again() {
        let popup = format!(r#"<img src="data:image/jpeg;base64,{}"><input name="captcha_phrase" maxlength=8><input type="hidden" name="captcha_key" value="k20"><input type="checkbox" name="agreed">"#, png_b64());
        let site = Site::default()
            .answers("/test/res/51520.html", 200, "fixture:vichan_wizchan_form.html")
            .answers("sys.8kun.top/post.php", 200, r#"{"popcaptcha":true}"#)
            .answers("sys.8kun.top/captcha-post-popup.php", 200, &popup)
            .answers("sys.8kun.top/captcha-post-popup.php", 200, r#"{"status":1,"message":"ok"}"#)
            .answers("sys.8kun.top/post.php", 200, r#"{"redirect":"/test/res/51520.html#88616","id":88616}"#);
        let to = Where { site: "8kun".into(), board: "test".into(), thread: 51520 };
        let v = Vichan { root: "https://8kun.top".into(), sys: Some("https://sys.8kun.top".into()) };
        assert_eq!(v.post(&session(&site, &Person::says(&[Some("x9")])), &to, &draft()).unwrap(), Posted { thread: 51520, no: 88616 });
        let answer = site.sent("captcha-post-popup.php");
        assert_eq!((answer.field("captcha_key"), answer.field("captcha_phrase")), (Some("k20"), Some("x9")));
        assert_eq!(site.sent("post.php").field("domain_name_post"), Some("8kun.top"));
    }

    #[test]
    fn leftypols_hcaptcha_once_it_flags_you() {
        let page = r#"<form name="post"><input type="hidden" name="board" value="meta"><textarea name="captcha-response"></textarea><textarea name="body"></textarea><input type="submit" name="post" value="New Reply"></form>"#;
        let site = Site::default()
            .answers("/meta/res/37383.html", 200, page)
            .sets("captcha-required=1")
            .answers("/main.js", 200, r#"hcaptcha.render(el, {sitekey: "2dae1350-7926-40b2-b59e-24a02272c072", callback"#)
            .answers("/post.php", 200, r#"{"redirect":"/meta/res/37383.html#46282","id":46282}"#);
        let to = Where { site: "leftypol".into(), board: "meta".into(), thread: 37383 };
        let v = Vichan { root: "https://leftypol.org".into(), sys: None };
        v.post(&session(&site, &Person::default()), &to, &draft()).unwrap();
        assert_eq!(site.widgets.borrow().as_slice(), [("hcaptcha".to_string(), "2dae1350-7926-40b2-b59e-24a02272c072".to_string())]);
        let sent = site.sent("/post.php");
        assert_eq!((sent.field("captcha-response"), sent.field("h-captcha-response")), (Some("hcaptcha-token"), Some("hcaptcha-token")));
    }

    #[test]
    fn what_vichan_answers() {
        let reply = Where { site: "s".into(), board: "a".into(), thread: 5 };
        assert_eq!(posted(&reply, &serde_json::json!({"error": "Your request looks automated; Post discarded."})).unwrap_err().to_string(), "Your request looks automated; Post discarded.");
    }
}
