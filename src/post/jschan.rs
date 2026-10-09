//! jschan: posts go to `/forms/board/{board}/post`, answered in JSON when asked with
//! `x-using-xhr`. Its captcha (when the board wants one: `captchaMode`) is a grid of icons to
//! pick from, or text to read; its id rides in a cookie. A "block bypass" is the same captcha,
//! asked for first when the site wants it.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use serde_json::Value;

use super::{Answered, Draft, Poster, Sent, Where, thread_in};
use crate::captcha::{Captcha, Cell, Challenge, Task};
use crate::http::encode_segment as enc;
use crate::web::{Fetch, Fetched, Helper, Upload};

pub struct Jschan {
    pub root: String,
}

/// The id of the captcha asked for a block bypass rather than a post.
const BYPASS: &str = "bypass";

impl Jschan {
    /// A captcha: its kind and question from the site's settings, its picture from
    /// `/captcha` (which sets its cookie). `id` says what it's for.
    fn fresh_captcha(&self, web: &Helper, id: &str) -> Result<Captcha> {
        let settings = web.fetch(Fetch::get(&format!("{}/settings.json", self.root)))?.json()?;
        let options = settings.get("captchaOptions").cloned().unwrap_or_default();
        let picture = web.fetch(Fetch::get(&format!("{}/captcha", self.root)))?;
        Ok(Captcha::Challenge(challenge(id, &options, picture.image()?, Instant::now())?))
    }
}

impl Poster for Jschan {
    fn page(&self) -> String {
        format!("{}/robots.txt", self.root)
    }

    fn captcha(&self, web: &Helper, to: &Where) -> Result<Captcha> {
        let board = web.fetch(Fetch::get(&format!("{}/{}/settings.json", self.root, enc(&to.board))))?.json()?;
        let mode = board.get("captchaMode").and_then(Value::as_u64).unwrap_or(0);
        if mode == 2 || (mode == 1 && to.new_thread()) {
            return self.fresh_captcha(web, "post");
        }
        Ok(super::no_captcha("post"))
    }

    fn answer(&self, web: &Helper, challenge: &Challenge, answer: &str) -> Result<Answered> {
        let fields = answer_fields(&challenge.task, answer);
        if challenge.id != BYPASS {
            return Ok(Answered::Done(fields));
        }
        // The bypass is its own form; it sets a cookie the post then carries.
        let r = web.fetch(Fetch::post(&format!("{}/forms/blockbypass", self.root), fields).header("x-using-xhr", "true"))?;
        if r.status >= 400 {
            return Err(error(&r));
        }
        Ok(Answered::Done(Vec::new()))
    }

    fn post(&self, web: &Helper, to: &Where, draft: &Draft, captcha: &[(String, String)]) -> Result<Sent> {
        let board = enc(&to.board);
        let mut fields = Vec::new();
        if !to.new_thread() {
            fields.push(("thread".to_string(), to.thread.to_string()));
        }
        fields.extend([("name".into(), draft.name.clone()), ("email".into(), draft.email.clone()), ("subject".into(), draft.subject.clone()), ("message".into(), draft.comment.clone()), ("postpassword".into(), draft.password.clone())]);
        if draft.spoiler && draft.file.is_some() {
            fields.push(("spoiler_all".into(), "true".into()));
        }
        fields.extend(captcha.iter().cloned());
        let referer = match to.thread {
            0 => format!("{}/{board}/index.html", self.root),
            t => format!("{}/{board}/thread/{t}.html", self.root),
        };
        let file = draft.file.as_ref().map(|p| Upload { field: "file".into(), path: p.display().to_string() });
        let url = format!("{}/forms/board/{board}/post", self.root);
        let r = web.fetch(Fetch { file, ..Fetch::post(&url, fields) }.header("x-using-xhr", "true").header("Referer", &referer))?;
        if r.status == 403 && r.body.contains("block bypass") {
            return Ok(Sent::Again(self.fresh_captcha(web, BYPASS)?));
        }
        if r.status >= 400 {
            return Err(error(&r));
        }
        let (thread, no) = posted(to, &r.json()?)?;
        Ok(Sent::Posted { thread, no })
    }
}

/// The captcha's fields: the text, or each picked cell's index.
fn answer_fields(task: &Task, answer: &str) -> Vec<(String, String)> {
    match task {
        Task::None => Vec::new(),
        Task::Grid { .. } => answer.chars().enumerate().filter(|&(_, c)| c == '1').map(|(i, _)| ("captcha".to_string(), i.to_string())).collect(),
        _ => vec![("captcha".into(), answer.to_string())],
    }
}

/// jschan's captcha from its settings (`captchaOptions`) and picture: a grid (one picture of
/// `size` rows of `size` icons, picked by their order), or text to read.
fn challenge(id: &str, options: &Value, picture: DynamicImage, now: Instant) -> Result<Challenge> {
    let expires = super::after(now, 300);
    let kind = options.get("type").and_then(Value::as_str).unwrap_or("text");
    let key = format!("captcha:jschan:{}", super::next_key());
    match kind {
        "grid" | "grid2" => {
            // Each row's icons are shifted along at random, so the cells go by the icons'
            // order (row by row, left to right), not by squares of the picture.
            let grid = options.get("grid").cloned().unwrap_or_default();
            let size = grid.get("size").and_then(Value::as_u64).filter(|&n| n > 0).unwrap_or(4);
            let question = grid.get("question").and_then(Value::as_str).unwrap_or("Pick the ones that fit");
            let prompt = format!("{question}: the icons in order, row by row");
            let cells = (1..=size).flat_map(|row| (1..=size).map(move |n| Cell::Label(format!("row {row}, {}", ordinal(n))))).collect();
            Ok(Challenge { id: id.into(), expires, task: Task::Grid { prompt, image: Some(key.clone()), cells, single: false }, pictures: vec![(key, picture)] })
        }
        "text" => Ok(Challenge { id: id.into(), expires, task: Task::Text { prompt: "Type the text in the picture".into(), image: Some(key.clone()) }, pictures: vec![(key, picture)] }),
        other => bail!("This site's captcha ({other}) isn't one ck can show yet: post from the browser"),
    }
}

/// "1st", "2nd", … for an icon's place in its row.
fn ordinal(n: u64) -> String {
    let suffix = match n {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// The thread and number of a post jschan took (`{postId, redirect}`).
fn posted(to: &Where, v: &Value) -> Result<(u64, u64)> {
    let no = v.get("postId").and_then(crate::http::as_u64).context("The site didn't say the post's number")?;
    let thread = v.get("redirect").and_then(Value::as_str).and_then(thread_in).unwrap_or_else(|| to.thread_of(no));
    Ok((thread, no))
}

/// jschan's error: its message, or its list of them.
fn error(r: &Fetched) -> anyhow::Error {
    let Ok(v) = r.json() else { return anyhow::anyhow!("The site answered {}", r.status) };
    let errors = v.get("errors").and_then(Value::as_array).map(|e| e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; "));
    let message = v.get("message").and_then(Value::as_str).map(str::to_string).or(errors).or_else(|| v.get("title").and_then(Value::as_str).map(str::to_string));
    anyhow::anyhow!(message.unwrap_or_else(|| format!("The site answered {}", r.status)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_grid_captcha_is_picked_by_the_icons_order() {
        let options = json!({"type": "grid", "grid": {"size": 4, "question": "Select the solid/filled icons"}});
        let c = challenge("post", &options, DynamicImage::new_rgb8(150, 150), Instant::now()).unwrap();
        let Task::Grid { cells, image: Some(_), .. } = &c.task else { panic!("not a grid with its picture") };
        assert_eq!((cells.len(), c.pictures.len()), (16, 1));
        assert_eq!(cells.get(5), Some(&Cell::Label("row 2, 2nd".into())));
        let picks = format!("1{}1", "0".repeat(14));
        assert_eq!(answer_fields(&c.task, &picks), [("captcha".to_string(), "0".to_string()), ("captcha".into(), "15".into())]);
        let to = Where { site: "z".into(), board: "meta".into(), thread: 7 };
        assert_eq!(posted(&to, &json!({"postId": 9, "redirect": "/meta/thread/7.html#9"})).unwrap(), (7, 9));
    }
}
