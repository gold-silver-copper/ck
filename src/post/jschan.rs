//! jschan: posts go to `/forms/board/{board}/post`, answered in JSON when asked with
//! `x-using-xhr`. Its captcha (when the board wants one: `captchaMode`) is a grid of icons to
//! pick from, or text to read; its id rides in a cookie. A "block bypass" is the same captcha,
//! asked for first when the site wants it.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use serde_json::Value;

use super::{Draft, Posted, Poster, Session, Where, thread_in};
use crate::captcha::{Cell, Challenge, Task};
use crate::http::encode_segment as enc;
use crate::web::{Fetch, Fetched, Upload};

pub struct Jschan {
    pub root: String,
}

impl Jschan {
    /// The fields of the site's captcha, once it's done: its kind and question from the
    /// site's settings, its picture from `/captcha` (which sets its cookie), asked for again
    /// until there's an answer; or a service's widget, its key in the board's page.
    fn captcha(&self, s: &Session, board: &str) -> Result<Vec<(String, String)>> {
        let settings = s.web.fetch(Fetch::get(&format!("{}/settings.json", self.root)))?.json()?;
        let options = settings.get("captchaOptions").cloned().unwrap_or_default();
        if matches!(options.get("type").and_then(Value::as_str), Some("hcaptcha" | "google" | "yandex")) {
            let page = s.web.fetch(Fetch::get(&format!("{}/{}/index.html", self.root, enc(board))))?.body;
            let (provider, sitekey) = super::widget_in(&page).context("The site wants a captcha service's check, and ck couldn't find its key")?;
            return Ok(vec![("captcha".into(), s.web.widget(provider, &sitekey)?)]);
        }
        let (challenge, answer) = s.solve(|| challenge(&options, s.web.fetch(Fetch::get(&format!("{}/captcha", self.root)))?.image()?, Instant::now()))?;
        Ok(answer_fields(&challenge.task, &answer))
    }
}

impl Poster for Jschan {
    fn page(&self) -> String {
        format!("{}/robots.txt", self.root)
    }

    fn post(&self, s: &Session, to: &Where, draft: &Draft) -> Result<Posted> {
        let board = enc(&to.board);
        let settings = s.web.fetch(Fetch::get(&format!("{}/{board}/settings.json", self.root)))?.json()?;
        let mode = settings.get("captchaMode").and_then(Value::as_u64).unwrap_or(0);
        let mut fields = Vec::new();
        if !to.new_thread() {
            fields.push(("thread".to_string(), to.thread.to_string()));
        }
        fields.extend([("name".into(), draft.name.clone()), ("email".into(), draft.email.clone()), ("subject".into(), draft.subject.clone()), ("message".into(), draft.comment.clone()), ("postpassword".into(), draft.password.clone())]);
        if draft.spoiler && draft.file.is_some() {
            fields.push(("spoiler_all".into(), "true".into()));
        }
        let wants = mode == 2 || (mode == 1 && to.new_thread());
        let mut captcha = if wants { self.captcha(s, &to.board)? } else { Vec::new() };
        let referer = match to.thread {
            0 => format!("{}/{board}/index.html", self.root),
            t => format!("{}/{board}/thread/{t}.html", self.root),
        };
        let url = format!("{}/forms/board/{board}/post", self.root);
        let send = |captcha: &[(String, String)]| {
            let file = draft.file.as_ref().map(|p| Upload { field: "file".into(), path: p.display().to_string() });
            let fields = [fields.as_slice(), captcha].concat();
            s.web.fetch(Fetch { file, ..Fetch::post(&url, fields) }.header("x-using-xhr", "true").header("Referer", &referer))
        };
        let mut r = send(&captcha)?;
        // A block bypass first: the same captcha, to its own form (it sets a cookie the post
        // then carries), then the post again, with a new captcha if it wants one (each is
        // taken once).
        if r.status == 403 && r.body.contains("block bypass") {
            let bypass = self.captcha(s, &to.board)?;
            let b = s.web.fetch(Fetch::post(&format!("{}/forms/blockbypass", self.root), bypass).header("x-using-xhr", "true"))?;
            if b.status >= 400 {
                return Err(error(&b));
            }
            if wants {
                captcha = self.captcha(s, &to.board)?;
            }
            r = send(&captcha)?;
        }
        if r.status >= 400 {
            return Err(error(&r));
        }
        let (thread, no) = posted(to, &r.json()?)?;
        Ok(Posted { thread, no })
    }
}

/// The captcha's fields: the text, or each picked cell's index.
fn answer_fields(task: &Task, answer: &str) -> Vec<(String, String)> {
    match task {
        Task::None => Vec::new(),
        Task::Grid { .. } => answer.chars().enumerate().filter(|&(_, c)| c == '1').map(|(i, _)| ("captcha".to_string(), i.to_string())).collect(),
        // Its text is six lower-case letters and digits, compared as sent.
        _ => vec![("captcha".into(), answer.trim().to_lowercase())],
    }
}

/// jschan's captcha from its settings (`captchaOptions`) and picture: a grid (one picture of
/// `size` rows of `size` icons, picked by their order), a square grid ("grid2": arrows
/// pointing at an icon, picked as cells), or text to read.
fn challenge(options: &Value, picture: DynamicImage, now: Instant) -> Result<Challenge> {
    let expires = super::after(now, 300);
    let kind = options.get("type").and_then(Value::as_str).unwrap_or("text");
    let key = format!("captcha:jschan:{}", super::next_key());
    match kind {
        "grid" => {
            // Each row's icons are shifted along at random, so the cells go by the icons'
            // order (row by row, left to right), not by squares of the picture.
            let (size, question) = grid_options(options);
            let prompt = format!("{question}: the icons in order, row by row");
            let cells = (1..=u64::from(size)).flat_map(|row| (1..=u64::from(size)).map(move |n| Cell::Label(format!("row {row}, {}", ordinal(n))))).collect();
            Ok(Challenge { id: String::new(), expires, task: Task::Grid { prompt, image: Some(key.clone()), cells, single: false }, pictures: vec![(key, picture)] })
        }
        "grid2" => {
            // A square grid: its cells are squares of the picture, numbered in it (one picture
            // sits well in every terminal, where many small ones don't).
            let (size, prompt) = grid_options(options);
            let picture = picture.resize(450, 450, image::imageops::FilterType::Lanczos3);
            let mut numbered = picture.to_rgba8();
            let side = numbered.width().checked_div(size).unwrap_or(0);
            for (n, (row, col)) in (1u32..).zip((0..size).flat_map(|row| (0..size).map(move |col| (row, col)))) {
                crate::captcha::number(&mut numbered, n, col.saturating_mul(side).saturating_add(2), row.saturating_mul(side).saturating_add(2));
            }
            let cells = (0..size.saturating_mul(size)).map(|_| Cell::Label(String::new())).collect();
            Ok(Challenge { id: String::new(), expires, task: Task::Grid { prompt, image: Some(key.clone()), cells, single: false }, pictures: vec![(key, DynamicImage::ImageRgba8(numbered))] })
        }
        "text" => Ok(Challenge { id: String::new(), expires, task: Task::Text { prompt: "Type the text in the picture".into(), image: Some(key.clone()) }, pictures: vec![(key, picture)] }),
        other => bail!("This site's captcha ({other}) isn't one ck can show yet: post from the browser"),
    }
}

/// A grid's size (cells a side) and question, from `captchaOptions`.
fn grid_options(options: &Value) -> (u32, String) {
    let grid = options.get("grid").cloned().unwrap_or_default();
    let size = grid.get("size").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).filter(|&n| n > 0).unwrap_or(4);
    let question = grid.get("question").and_then(Value::as_str).unwrap_or("Pick the ones that fit").to_string();
    (size, question)
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

    fn png_b64() -> String {
        let mut png = std::io::Cursor::new(Vec::new());
        DynamicImage::new_rgb8(150, 150).write_to(&mut png, image::ImageFormat::Png).unwrap();
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png.into_inner())
    }

    #[test]
    fn a_reply_on_a_board_with_a_captcha_then_a_block_bypass() {
        use super::super::fake::{Person, Site, session};
        let picture = png_b64();
        let site = Site::default()
            .answers("/meta/settings.json", 200, r#"{"captchaMode":2,"maxFiles":5}"#)
            .answers("/settings.json", 200, "fixture:jschan_zzzchan_settings.json")
            .answers("/captcha", 200, &picture)
            .answers("/forms/board/meta/post", 403, r#"{"title":"Forbidden","message":"Please complete a block bypass to continue","frame":"/bypass_minimal.html"}"#)
            .answers("/settings.json", 200, "fixture:jschan_zzzchan_settings.json")
            .answers("/captcha", 200, &picture)
            .answers("/forms/blockbypass", 200, r#"{"title":"Success"}"#)
            .answers("/settings.json", 200, "fixture:jschan_zzzchan_settings.json")
            .answers("/captcha", 200, &picture)
            .answers("/forms/board/meta/post", 200, r#"{"postId":6307,"redirect":"/meta/thread/4378.html#6307"}"#);
        let person = Person::says(&[Some("0100000000000000"), Some("1000000000000000"), Some("0010000000000001")]);
        let to = Where { site: "zzzchan".into(), board: "meta".into(), thread: 4378 };
        let draft = Draft { comment: "hi".into(), password: "pw".into(), ..Default::default() };
        let jschan = Jschan { root: "https://zzzchan.xyz".into() };
        assert_eq!(jschan.post(&session(&site, &person), &to, &draft).unwrap(), Posted { thread: 4378, no: 6307 });
        let picks: Vec<_> = site.sent("/forms/board/meta/post").fields.into_iter().filter(|(k, _)| k == "captcha").map(|(_, v)| v).collect();
        assert_eq!(picks, ["2", "15"]);
        assert_eq!(site.sent("/forms/blockbypass").field("captcha"), Some("0"));
        assert!(matches!(person.shown.borrow().first(), Some(Task::Grid { image: Some(_), .. })));
    }

    #[test]
    fn a_grid_captcha_is_picked_by_the_icons_order() {
        let options = json!({"type": "grid", "grid": {"size": 4, "question": "Select the solid/filled icons"}});
        let c = challenge(&options, DynamicImage::new_rgb8(150, 150), Instant::now()).unwrap();
        let Task::Grid { cells, image: Some(_), .. } = &c.task else { panic!("not a grid with its picture") };
        assert_eq!((cells.len(), c.pictures.len()), (16, 1));
        assert_eq!(cells.get(5), Some(&Cell::Label("row 2, 2nd".into())));
        let picks = format!("1{}1", "0".repeat(14));
        assert_eq!(answer_fields(&c.task, &picks), [("captcha".to_string(), "0".to_string()), ("captcha".into(), "15".into())]);
        let to = Where { site: "z".into(), board: "meta".into(), thread: 7 };
        assert_eq!(posted(&to, &json!({"postId": 9, "redirect": "/meta/thread/7.html#9"})).unwrap(), (7, 9));
    }
}
