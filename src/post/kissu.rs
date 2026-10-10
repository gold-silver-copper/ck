//! kissu: its own engine now, still posting to `post.php` (vichan's fields, the password as
//! `pswrd`, no anti-spam form). Posting too often holds the post back behind its own captcha,
//! "captchouli" (pick every picture of a character, from nine): solved at `/captcha`, its code
//! then releases the held post.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::form::Form;
use super::{Draft, Posted, Poster, Session, Where, said, thread_in};
use crate::captcha::{Cell, Challenge, Task};
use crate::web::{Fetch, Upload};

pub struct Kissu {
    pub root: String,
}

impl Kissu {
    /// captchouli, answered until it gives its code: each wrong answer brings another grid.
    fn captchouli(&self, s: &Session) -> Result<String> {
        let url = format!("{}/captcha", self.root);
        let mut html = s.web.fetch(Fetch::get(&url))?.body;
        loop {
            let (challenge, hidden) = grid(&html, Instant::now())?;
            let Some(answer) = s.ask.solve(&challenge)? else {
                html = s.web.fetch(Fetch::get(&url))?.body;
                continue;
            };
            let mut fields = hidden;
            fields.extend(answer.chars().enumerate().filter(|&(_, c)| c == '1').map(|(i, _)| (format!("captchouli-{i}"), "on".to_string())));
            html = s.web.fetch(Fetch::post(&url, fields))?.body;
            // Solved: its code alone, in a <pre>; else another grid.
            if let Some((code, _)) = html.split_once("<pre>").and_then(|(_, rest)| rest.split_once("</pre>")) {
                return Ok(code.trim().to_string());
            }
        }
    }
}

impl Poster for Kissu {
    fn page(&self) -> String {
        format!("{}/robots.txt", self.root)
    }

    fn post(&self, s: &Session, to: &Where, draft: &Draft) -> Result<Posted> {
        let url = format!("{}/post.php", self.root);
        let referer = format!("{}/{}/", self.root, to.board);
        let mut fields = vec![("json_response".to_string(), "1".to_string()), ("board".into(), to.board.clone()), ("name".into(), draft.name.clone()), ("email".into(), draft.email.clone()), ("subject".into(), draft.subject.clone()), ("body".into(), draft.comment.clone()), ("pswrd".into(), draft.password.clone())];
        if !to.new_thread() {
            fields.push(("thread".into(), to.thread.to_string()));
        }
        fields.push(("spoiler".into(), if draft.spoiler { "spoiler" } else { "default" }.into()));
        fields.extend([("post".into(), if to.new_thread() { "New Topic" } else { "New Reply" }.into()), ("captype".into(), "captchouli".into())]);
        let file = draft.file.as_ref().map(|p| Upload { field: "file".into(), path: p.display().to_string() });
        let mut v = s.web.fetch(Fetch { file, ..Fetch::post(&url, fields) }.header("Referer", &referer))?.json()?;
        // Posting too often holds the post back: captchouli's code releases it (it isn't sent
        // again).
        if v.get("captcha").and_then(crate::http::as_u64) == Some(1) {
            let reference = v.get("id").map(|id| id.as_str().map_or_else(|| id.to_string(), str::to_string)).context("The site held the post back without saying which")?;
            let code = self.captchouli(s)?;
            let release = vec![("json_response".to_string(), "1".to_string()), ("captchouli".into(), code), ("reference".into(), reference), ("release".into(), "submit".into()), ("board".into(), to.board.clone())];
            v = s.web.fetch(Fetch::post(&url, release).header("Referer", &referer))?.json()?;
        }
        let (thread, no) = posted(to, &v)?;
        Ok(Posted { thread, no })
    }
}

/// The thread and number of a post kissu took (`{redirect, id, thread}`), or its error.
fn posted(to: &Where, v: &Value) -> Result<(u64, u64)> {
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(said(e));
    }
    if v.get("banned").is_some() {
        bail!("You're banned there{}", v.get("reason").and_then(Value::as_str).map(|r| format!(": {r}")).unwrap_or_default());
    }
    let no = v.get("id").and_then(crate::http::as_u64).context("The site didn't say the post's number")?;
    let thread = v.get("thread").and_then(crate::http::as_u64).filter(|&t| t > 0).or_else(|| v.get("redirect").and_then(Value::as_str).and_then(thread_in)).unwrap_or_else(|| to.thread_of(no));
    Ok((thread, no))
}

/// captchouli's grid: "Select all images of …", nine pictures (inline) in one numbered
/// picture; and its hidden fields, that go back with its picks.
fn grid(html: &str, now: Instant) -> Result<(Challenge, Vec<(String, String)>)> {
    let pictures: Vec<_> = html.split("base64,").skip(1).filter_map(|rest| crate::captcha::inline_image(&format!("base64,{rest}"))).collect();
    if pictures.is_empty() {
        bail!("The site's captcha didn't come");
    }
    let form = Form::find(&format!("<form name=\"c\">{html}</form>"), "c").unwrap_or_default();
    let hidden = form.fields.into_iter().filter(|(k, _)| ["captchouli-id", "captchouli-color", "captchouli-background"].contains(&k.as_str())).collect();
    let whom = html.split_once("Select all images of").map(|(_, rest)| crate::markup::strip_tags(rest.split('<').take(3).collect::<Vec<_>>().join("<").as_str())).unwrap_or_default();
    let prompt = format!("Pick every picture of {}", whom.trim().trim_end_matches('.'));
    let key = format!("captcha:kissu:{}", super::next_key());
    let cells = pictures.iter().map(|_| Cell::Label(String::new())).collect();
    let sheet = crate::captcha::sheet(&pictures, 3);
    Ok((Challenge { id: String::new(), expires: super::after(now, 600), task: Task::Grid { prompt, image: Some(key.clone()), cells, single: false }, pictures: vec![(key, sheet)] }, hidden))
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Person, Site, session};
    use super::*;

    fn captcha_page(who: &str) -> String {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png.into_inner());
        let imgs: String = (0..9).map(|i| format!(r#"<label><img src="data:image/png;base64,{b64}"><input type="checkbox" name="captchouli-{i}"></label>"#)).collect();
        format!(r#"<form action="/captcha" method="post"><span>Select all images of <b>{who}</b></span>{imgs}<input type="hidden" name="captchouli-id" value="abc"><input type="hidden" name="captchouli-color" value="1"><input type="hidden" name="captchouli-background" value="2"></form>"#)
    }

    #[test]
    fn a_held_post_is_released_by_captchoulis_code() {
        let site = Site::default()
            .answers("/post.php", 200, r#"{"captcha":1,"id":77,"error":"Flood detected. Solve the captcha."}"#)
            .answers("/captcha", 200, &captcha_page("Kirino"))
            .answers("/captcha", 200, &captcha_page("Kuroneko"))
            .answers("/captcha", 200, "<pre>code-1</pre>")
            .answers("/post.php", 200, r#"{"redirect":"/test/thread/5","noko":true,"id":9,"thread":5}"#);
        let person = Person::says(&[Some("100000000"), Some("010000001")]);
        let to = Where { site: "kissu".into(), board: "test".into(), thread: 5 };
        let draft = Draft { comment: "hi".into(), password: "pw".into(), ..Default::default() };
        assert_eq!(Kissu { root: "https://kissu.moe".into() }.post(&session(&site, &person), &to, &draft).unwrap(), Posted { thread: 5, no: 9 });
        let picks = site.sent("/captcha");
        assert_eq!((picks.field("captchouli-id"), picks.field("captchouli-1"), picks.field("captchouli-8")), (Some("abc"), Some("on"), Some("on")));
        let release = site.sent("/post.php");
        assert_eq!((release.field("captchouli"), release.field("reference"), release.field("release"), release.field("body")), (Some("code-1"), Some("77"), Some("submit"), None));
        assert!(matches!(person.shown.borrow().last(), Some(Task::Grid { prompt, .. }) if prompt == "Pick every picture of Kuroneko"));
    }
}
