//! 4chan: its captcha (the "twister", read the way 4chan's own `tcaptcha.js` reads it) comes
//! in a frame of sys.4chan.org, which ck-web asks for; posts go to sys.4chan.org.

use std::time::Instant;

use anyhow::{Result, bail};
use serde_json::Value;

use super::{Draft, Posted, Poster, Session, Where, said};
use crate::captcha::{Cell, Challenge, Step, Task, decode, inline_image};
use crate::markup::strip_tags;
use crate::web::{Fetch, Upload};

/// Where 4chan keeps the images of its newer captchas (as its script has it).
const IMAGES: &str = "https://s.4cdn.org/image/temp/april2026";

pub struct Fourchan;

impl Poster for Fourchan {
    fn page(&self) -> String {
        "https://boards.4chan.org/robots.txt".into()
    }

    fn comment_limit(&self) -> Option<usize> {
        Some(2000)
    }

    fn post(&self, s: &Session, to: &Where, draft: &Draft) -> Result<Posted> {
        let (challenge, answer) = loop {
            match parse(&s.web.fourchan_captcha(&to.board, to.thread)?, Instant::now()) {
                Twister::Refused(why) => bail!(why),
                Twister::Wait { until, message } => s.ask.wait(until, &message)?,
                Twister::Challenge(c) if matches!(c.task, Task::None) => break (c, String::new()),
                Twister::Challenge(c) => {
                    if let Some(answer) = s.ask.solve(&c)? {
                        break (c, answer);
                    }
                }
            }
        };
        let mut fields = vec![("mode".to_string(), "regist".to_string())];
        if !to.new_thread() {
            fields.push(("resto".into(), to.thread.to_string()));
        }
        fields.extend([("name".into(), draft.name.clone()), ("email".into(), draft.email.clone())]);
        if to.new_thread() {
            fields.push(("sub".into(), draft.subject.clone()));
        }
        fields.push(("com".into(), draft.comment.clone()));
        if draft.spoiler && draft.file.is_some() {
            fields.push(("spoiler".into(), "on".into()));
        }
        fields.push(("pwd".into(), draft.password.clone()));
        fields.extend(answer_fields(&challenge, &answer));
        let url = format!("https://sys.4chan.org/{}/post", crate::http::encode_segment(&to.board));
        let file = draft.file.as_ref().map(|p| Upload { field: "upfile".into(), path: p.display().to_string() });
        let r = s.web.fetch(Fetch { file, ..Fetch::post(&url, fields) }.header("Accept", "application/json"))?;
        posted(r.status, &r.body).map(|(thread, no)| Posted { thread, no })
    }
}

/// What 4chan's captcha page said.
#[derive(Debug)]
enum Twister {
    /// It won't give one now: why.
    Refused(String),
    /// Not for a while (posting too often): how long, and its message.
    Wait { until: Instant, message: String },
    Challenge(Challenge),
}

/// The form's fields for an answer. A picture's text goes as 4chan's form sends it: lower
/// case, letters and digits.
fn answer_fields(challenge: &Challenge, answer: &str) -> Vec<(String, String)> {
    let answer = match challenge.task {
        Task::Text { .. } => answer.to_lowercase().chars().filter(char::is_ascii_alphanumeric).collect(),
        _ => answer.to_string(),
    };
    vec![("t-challenge".into(), challenge.id.clone()), ("t-response".into(), answer)]
}

/// What 4chan answered a post with: JSON, or its HTML page (an error, or the thread and number
/// in a comment).
fn posted(status: u16, body: &str) -> Result<(u64, u64)> {
    if let Ok(j) = serde_json::from_str::<Value>(body) {
        if let Some(e) = j.get("error").and_then(Value::as_str) {
            return Err(said(e));
        }
        if let Some(no) = j.get("pid").and_then(crate::http::as_u64) {
            let thread = j.get("tid").and_then(crate::http::as_u64).filter(|&t| t > 0).unwrap_or(no);
            return Ok((thread, no));
        }
    }
    let between = |start: &str, end: &str| body.split_once(start).and_then(|(_, rest)| rest.split_once(end)).map(|(v, _)| v);
    if let Some(e) = between("id=\"errmsg\"", "</span").and_then(|e| e.split_once('>')) {
        return Err(said(e.1));
    }
    if let Some((thread, no)) = between("<!-- thread:", " -->").and_then(|t| t.split_once(",no:")) {
        let (thread, no) = (thread.parse::<u64>()?, no.parse::<u64>()?);
        return Ok((if thread == 0 { no } else { thread }, no));
    }
    bail!("4chan answered {status} and didn't say whether it posted")
}

/// What 4chan's captcha page said.
fn parse(v: &Value, now: Instant) -> Twister {
    let str_of = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let secs = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Twister::Refused(strip_tags(e));
    }
    if secs("pcd") > 0 {
        let message = v.get("pcd_msg").and_then(Value::as_str).map_or_else(|| "Please wait a while.".into(), strip_tags);
        return Twister::Wait { until: super::after(now, secs("pcd")), message };
    }
    // Answers aren't taken in the last few seconds (as 4chan's script counts).
    let ttl = secs("ttl").max(10).saturating_sub(3);
    let task = match v.get("extTask") {
        Some(ext) => ext_task(ext),
        None => match v.get("tasks").and_then(Value::as_array) {
            Some(tasks) => Task::Slider(tasks.iter().map(step).collect()),
            None => Task::None,
        },
    };
    Twister::Challenge(Challenge { id: str_of("challenge"), expires: super::after(now, ttl), task, pictures: Vec::new() })
}

/// A slider step. Its words (`str`) are HTML, with the reference in them as an inline
/// `<img src="data:image/png;base64,…">`; or the reference is on its own (`img`).
fn step(t: &Value) -> Step {
    let html = t.get("str").and_then(Value::as_str).unwrap_or_default();
    let reference = t.get("img").and_then(Value::as_str).and_then(decode).or_else(|| inline_image(html));
    let items = t.get("items").and_then(Value::as_array).map(|a| a.iter().filter_map(|i| i.as_str().and_then(decode)).collect()).unwrap_or_default();
    Step { text: strip_tags(html), reference, items }
}

fn ext_task(ext: &Value) -> Task {
    let prompt = ext.get("str").and_then(Value::as_str).map(strip_tags).unwrap_or_default();
    let root = ext.get("imgRoot").and_then(Value::as_str).unwrap_or_default();
    // `[file, width, height]`.
    let url = |img: &Value| img.get(0).and_then(Value::as_str).map(|f| format!("{IMAGES}/{root}/{f}"));
    match ext.get("mode").and_then(Value::as_u64) {
        Some(2) => {
            let cells = match ext.get("imgs").and_then(Value::as_array) {
                Some(imgs) => imgs.iter().filter_map(url).map(Cell::Image).collect(),
                None => ext.get("labels").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(|s| Cell::Label(s.to_string())).collect()).unwrap_or_default(),
            };
            let single = ext.get("single").is_some_and(|s| s.as_bool().unwrap_or_else(|| s.as_u64().is_some_and(|n| n > 0)));
            Task::Grid { prompt, image: None, cells, single }
        }
        _ => Task::Text { prompt, image: ext.get("img").and_then(url) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::captcha::Solving;
    use image::DynamicImage;
    use serde_json::json;

    /// A 1x1 PNG, base64.
    fn png() -> String {
        let mut out = std::io::Cursor::new(Vec::new());
        let _ = DynamicImage::new_rgb8(1, 1).write_to(&mut out, image::ImageFormat::Png);
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, out.into_inner())
    }

    fn challenge(v: &Value) -> Solving {
        match parse(v, Instant::now()) {
            Twister::Challenge(c) => Solving::new(c),
            other => panic!("not a challenge: {other:?}"),
        }
    }

    #[test]
    fn slider_steps_answer_with_their_picks() {
        let img = png();
        // As 4chan sends them: the reference inline in the words, or on its own.
        let words = format!(r#"<img src="data:image/png;base64,{img}" style="float:right">Use the scroll bar below to find the image"#);
        let mut s = challenge(&json!({"challenge": "abc", "ttl": 120, "tasks": [
            {"str": words, "items": [img, img, img]},
            {"img": img, "items": [img, img]},
        ]}));
        let step = s.current().unwrap();
        assert_eq!(step.text, "Use the scroll bar below to find the image");
        assert!(step.reference.is_some());
        // Past the last item stays on it.
        s.slide(5);
        assert_eq!(s.slide, 2);
        assert!(!s.next_step());
        assert!(s.current().unwrap().reference.is_some());
        s.slide(-1);
        assert_eq!(s.answer(), None);
        assert!(s.next_step());
        assert_eq!((s.challenge.id.as_str(), s.answer().as_deref()), ("abc", Some("20")));
    }

    #[test]
    fn newer_tasks_text_and_grid() {
        let mut text = challenge(&json!({"challenge": "c", "ttl": 60, "extTask": {"mode": 1, "str": "Type it", "imgRoot": "r", "img": ["a.png", 300, 80]}}));
        assert!(matches!(&text.challenge.task, Task::Text { image: Some(u), .. } if u == "https://s.4cdn.org/image/temp/april2026/r/a.png"));
        text.typed = "Ab 3-x".into();
        assert_eq!(answer_fields(&text.challenge, &text.answer().unwrap()), [("t-challenge".to_string(), "c".to_string()), ("t-response".into(), "ab3x".into())]);
        let mut grid = challenge(&json!({"challenge": "c", "ttl": 60, "extTask": {"mode": 2, "str": "Pick one", "single": 1, "labels": ["a", "b", "c"]}}));
        assert_eq!(grid.answer(), None);
        grid.toggle(0);
        grid.toggle(2);
        assert_eq!(grid.answer().as_deref(), Some("001"));
        let none = challenge(&json!({"challenge": "c", "ttl": 60}));
        assert_eq!(none.answer().as_deref(), Some(""));
    }

    #[test]
    fn refusals_and_waits() {
        let now = Instant::now();
        assert!(matches!(parse(&json!({"error": "You have to wait <b>a bit</b>."}), now), Twister::Refused(e) if e == "You have to wait a bit."));
        assert!(matches!(parse(&json!({"pcd": 30, "pcd_msg": "Slow down."}), now), Twister::Wait { until, message } if until == now + std::time::Duration::from_secs(30) && message == "Slow down."));
    }

    #[test]
    fn a_post_waits_out_the_cooldown_then_answers_the_captcha() {
        use super::super::fake::{Person, Site, session};
        let site = Site::default()
            .twister(json!({"pcd": 30, "pcd_msg": "Slow down."}))
            .twister(json!({"challenge": "c1", "ttl": 60, "extTask": {"mode": 1, "str": "Type it"}}))
            .twister(json!({"challenge": "c2", "ttl": 60, "extTask": {"mode": 1, "str": "Type it"}}))
            .answers("sys.4chan.org/g/post", 200, r#"{"pid": 12, "tid": 7}"#);
        // The first captcha is passed over for another.
        let person = Person::says(&[None, Some("Ab 1")]);
        let draft = Draft { comment: "hi".into(), password: "pw".into(), ..Default::default() };
        let to = Where { site: "4chan".into(), board: "g".into(), thread: 7 };
        assert_eq!(Fourchan.post(&session(&site, &person), &to, &draft).unwrap(), Posted { thread: 7, no: 12 });
        assert_eq!(person.waited.borrow().as_slice(), ["Slow down."]);
        let sent = site.sent("/g/post");
        assert_eq!((sent.field("resto"), sent.field("com"), sent.field("t-challenge"), sent.field("t-response")), (Some("7"), Some("hi"), Some("c2"), Some("ab1")));
    }

    #[test]
    fn what_4chan_answers_a_post_with() {
        assert_eq!(posted(200, r#"{"pid": 5, "tid": 0}"#).unwrap(), (5, 5));
        assert_eq!(posted(200, r#"{"pid": 6, "tid": 2}"#).unwrap(), (2, 6));
        assert_eq!(posted(200, r#"{"error": "You must wait <b>longer</b>."}"#).unwrap_err().to_string(), "You must wait longer.");
        assert_eq!(posted(200, r#"<span id="errmsg" style="color:red">Error: no file.</span>"#).unwrap_err().to_string(), "Error: no file.");
        assert_eq!(posted(200, "<!-- thread:0,no:9 -->").unwrap(), (9, 9));
        assert!(posted(500, "?").is_err());
    }
}
