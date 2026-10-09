//! kissu: its own engine now, still posting to `post.php` (vichan's fields, the password as
//! `pswrd`, no anti-spam form). Posting too often holds the post back behind its own captcha,
//! "captchouli" (pick every picture of a character, from nine): solved at `/captcha`, its code
//! then releases the held post.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::form::Form;
use super::{Answered, Draft, Poster, Sent, Where, said, thread_in};
use crate::captcha::{Captcha, Cell, Challenge, Task};
use crate::web::{Fetch, Helper, Upload};

pub struct Kissu {
    pub root: String,
}

impl Kissu {
    /// The captcha holding post `reference` back.
    fn captchouli(&self, web: &Helper, board: &str, reference: &str) -> Result<Challenge> {
        let html = web.fetch(Fetch::get(&format!("{}/captcha", self.root)))?.body;
        grid(&html, board, reference, Instant::now())
    }
}

impl Poster for Kissu {
    fn page(&self) -> String {
        format!("{}/robots.txt", self.root)
    }

    fn captcha(&self, _web: &Helper, _to: &Where) -> Result<Captcha> {
        Ok(super::no_captcha(""))
    }

    fn answer(&self, web: &Helper, challenge: &Challenge, answer: &str) -> Result<Answered> {
        if matches!(challenge.task, Task::None) {
            return Ok(Answered::Done(Vec::new()));
        }
        let held: Value = serde_json::from_str(&challenge.id).unwrap_or_default();
        let text = |k: &str| held.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let mut fields: Vec<(String, String)> = ["captchouli-id", "captchouli-color", "captchouli-background"].iter().map(|k| (k.to_string(), text(k))).collect();
        fields.extend(answer.chars().enumerate().filter(|&(_, c)| c == '1').map(|(i, _)| (format!("captchouli-{i}"), "on".to_string())));
        let html = web.fetch(Fetch::post(&format!("{}/captcha", self.root), fields))?.body;
        // Solved: its code alone, in a <pre>; else another grid.
        match html.split_once("<pre>").and_then(|(_, rest)| rest.split_once("</pre>")) {
            Some((code, _)) => Ok(Answered::Done(vec![("captchouli".into(), code.trim().to_string()), ("reference".into(), text("reference")), ("release".into(), "submit".into()), ("board".into(), text("board"))])),
            None => Ok(Answered::Next(grid(&html, &text("board"), &text("reference"), Instant::now())?)),
        }
    }

    fn post(&self, web: &Helper, to: &Where, draft: &Draft, captcha: &[(String, String)]) -> Result<Sent> {
        let url = format!("{}/post.php", self.root);
        let mut fields = vec![("json_response".to_string(), "1".to_string())];
        let mut file = None;
        if captcha.iter().any(|(k, _)| k == "release") {
            // The post held back goes up with the captcha's code: it isn't sent again.
            fields.extend(captcha.iter().cloned());
        } else {
            fields.extend([("board".into(), to.board.clone()), ("name".into(), draft.name.clone()), ("email".into(), draft.email.clone()), ("subject".into(), draft.subject.clone()), ("body".into(), draft.comment.clone()), ("pswrd".into(), draft.password.clone())]);
            if !to.new_thread() {
                fields.push(("thread".into(), to.thread.to_string()));
            }
            fields.push(("spoiler".into(), if draft.spoiler { "spoiler" } else { "default" }.into()));
            fields.extend([("post".into(), if to.new_thread() { "New Topic" } else { "New Reply" }.into()), ("captype".into(), "captchouli".into())]);
            file = draft.file.as_ref().map(|p| Upload { field: "file".into(), path: p.display().to_string() });
        }
        let v = web.fetch(Fetch { file, ..Fetch::post(&url, fields) }.header("Referer", &format!("{}/{}/", self.root, to.board)))?.json()?;
        if v.get("captcha").and_then(crate::http::as_u64) == Some(1) {
            let reference = v.get("id").map(|id| id.as_str().map_or_else(|| id.to_string(), str::to_string)).context("The site held the post back without saying which")?;
            return Ok(Sent::Again(Captcha::Challenge(self.captchouli(web, &to.board, &reference)?)));
        }
        let (thread, no) = posted(to, &v)?;
        Ok(Sent::Posted { thread, no })
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
/// picture, and the hidden fields that go back with the picks.
fn grid(html: &str, board: &str, reference: &str, now: Instant) -> Result<Challenge> {
    let pictures: Vec<_> = html.split("base64,").skip(1).filter_map(|rest| crate::captcha::inline_image(&format!("base64,{rest}"))).collect();
    if pictures.is_empty() {
        bail!("The site's captcha didn't come");
    }
    let form = Form::find(&format!("<form name=\"c\">{html}</form>"), "c").unwrap_or_default();
    let hidden = |k: &str| form.fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
    let whom = html.split_once("Select all images of").map(|(_, rest)| crate::markup::strip_tags(rest.split('<').take(3).collect::<Vec<_>>().join("<").as_str())).unwrap_or_default();
    let prompt = format!("Pick every picture of {}", whom.trim().trim_end_matches('.'));
    let id = json!({"captchouli-id": hidden("captchouli-id"), "captchouli-color": hidden("captchouli-color"), "captchouli-background": hidden("captchouli-background"), "reference": reference, "board": board}).to_string();
    let key = format!("captcha:kissu:{}", super::next_key());
    let cells = pictures.iter().map(|_| Cell::Label(String::new())).collect();
    let sheet = crate::captcha::sheet(&pictures, 3);
    Ok(Challenge { id, expires: super::after(now, 600), task: Task::Grid { prompt, image: Some(key.clone()), cells, single: false }, pictures: vec![(key, sheet)] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_post_and_its_grid() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png.into_inner());
        let imgs: String = (0..9).map(|i| format!(r#"<label><img src="data:image/png;base64,{b64}"><input type="checkbox" name="captchouli-{i}"></label>"#)).collect();
        let html = format!(r#"<form action="/captcha" method="post"><span>Select all images of <b>Kirino</b></span>{imgs}<input type="hidden" name="captchouli-id" value="abc"><input type="hidden" name="captchouli-color" value="1"><input type="hidden" name="captchouli-background" value="2"></form>"#);
        let c = grid(&html, "test", "77", Instant::now()).unwrap();
        let Task::Grid { cells, prompt, .. } = &c.task else { panic!("not a grid") };
        assert_eq!((cells.len(), prompt.as_str()), (9, "Pick every picture of Kirino"));
        assert!(c.id.contains("\"captchouli-id\":\"abc\"") && c.id.contains("\"reference\":\"77\""));
        let to = Where { site: "kissu".into(), board: "test".into(), thread: 5 };
        assert_eq!(posted(&to, &serde_json::json!({"redirect": "/test/thread/5", "id": 9, "thread": 5})).unwrap(), (5, 9));
    }
}
