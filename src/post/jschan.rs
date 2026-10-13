//! jschan: posts go to `/forms/board/{board}/post`, answered in JSON when asked with
//! `x-using-xhr`. Its captcha (when the board wants one: `captchaMode`) is a grid of icons to
//! pick from, or text to read; its id rides in a cookie. A "block bypass" is the same captcha,
//! asked for first when the site wants it. Accounts log in at `/forms/login`.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use serde_json::Value;

use super::{Draft, Login, LoginField, Posted, Poster, Session, Web, Where, thread_in};
use crate::captcha::{Cell, Challenge, Task};
use crate::http::encode_segment as enc;
use crate::web::{Fetch, Fetched, Upload};

pub struct Jschan {
    pub root: String,
    /// The site's name, as the config has it.
    pub site: String,
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

    fn login(&self) -> Option<Arc<dyn Login>> {
        Some(Arc::new(Account { root: self.root.clone(), site: self.site.clone() }))
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
        // An account may be let off the captcha (jschan's "bypass captcha" permission: ptchan's
        // gold accounts have it), which only the site knows: logged in, the post is sent
        // without one first, and the captcha comes if it's still wanted. A refused post isn't
        // kept, so it can't land twice.
        let account = wants && logged_in(s.web, &self.root)?;
        let mut captcha = if wants && !account { self.captcha(s, &to.board)? } else { Vec::new() };
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
        if account && r.status == 403 && !r.body.contains("block bypass") {
            captcha = self.captcha(s, &to.board)?;
            r = send(&captcha)?;
        }
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

/// A jschan account, logged in to at `/forms/login`. The login is the site's session cookie,
/// which the page can't read (it's HttpOnly): whether it's there is asked of `/csrf.json`,
/// which only a logged-in account gets.
pub struct Account {
    pub root: String,
    pub site: String,
}

/// What jschan says to any login it won't take: a wrong name or password, or a two-factor
/// code missing or wrong.
const REFUSED: &str = "Incorrect login credentials";

impl Login for Account {
    fn name(&self) -> String {
        format!("{} account", self.site)
    }

    fn skips_captcha(&self) -> bool {
        false
    }

    fn asks(&self) -> &'static [LoginField] {
        &[LoginField { label: "Username", optional: false }, LoginField { label: "Password", optional: false }, LoginField { label: "2FA code", optional: true }]
    }

    fn page(&self) -> String {
        format!("{}/robots.txt", self.root)
    }

    fn logged_in(&self, web: &dyn Web) -> Result<bool> {
        logged_in(web, &self.root)
    }

    fn log_in(&self, web: &dyn Web, answers: &[String]) -> Result<()> {
        let [username, password, code] = answers else { bail!("A jschan account needs its name, password and two-factor code") };
        let mut fields = vec![("username".to_string(), username.trim().to_string()), ("password".into(), password.clone())];
        if !code.trim().is_empty() {
            fields.push(("twofactor".into(), code.trim().into()));
        }
        // URL-encoded, as its login form sends it: it reads no other kind.
        let body = fields.iter().map(|(k, v)| format!("{k}={}", enc(v))).collect::<Vec<_>>().join("&");
        let (url, referer) = (format!("{}/forms/login", self.root), format!("{}/login.html", self.root));
        let login = Fetch { body: Some(body), ..Fetch::post(&url, Vec::new()) };
        let r = web.fetch(login.header("Content-Type", "application/x-www-form-urlencoded").header("x-using-xhr", "true").header("Referer", &referer))?;
        if r.status >= 400 {
            let e = error(&r);
            // The same words for all of them: the code may be what's missing.
            if e.to_string() == REFUSED && code.trim().is_empty() {
                bail!("{e}: check the name and password, and give the 2FA code if the account has two-factor on");
            }
            return Err(e);
        }
        if !logged_in(web, &self.root)? {
            bail!("{} didn't log you in (it answered {})", self.site, r.status);
        }
        Ok(())
    }

    fn log_out(&self, web: &dyn Web) -> Result<()> {
        // It sends you to the front page, whatever it answers there.
        web.fetch(Fetch::post(&format!("{}/forms/logout", self.root), Vec::new()).header("x-using-xhr", "true"))?;
        if logged_in(web, &self.root)? {
            bail!("{} didn't log you out", self.site);
        }
        Ok(())
    }
}

/// Whether ck-web is logged in to the jschan site at `root`: `/csrf.json` answers its token to
/// an account, and sends anyone else to the login page.
fn logged_in(web: &dyn Web, root: &str) -> Result<bool> {
    let r = web.fetch(Fetch::get(&format!("{root}/csrf.json")))?;
    Ok(r.json().ok().is_some_and(|v| v.get("token").is_some()))
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
            // Not logged in: sent to the login page.
            .answers("/csrf.json", 200, "<!DOCTYPE html><title>Login</title>")
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
        let jschan = Jschan { root: "https://zzzchan.xyz".into(), site: "zzzchan".into() };
        assert_eq!(jschan.post(&session(&site, &person), &to, &draft).unwrap(), Posted { thread: 4378, no: 6307 });
        let picks: Vec<_> = site.sent("/forms/board/meta/post").fields.into_iter().filter(|(k, _)| k == "captcha").map(|(_, v)| v).collect();
        assert_eq!(picks, ["2", "15"]);
        assert_eq!(site.sent("/forms/blockbypass").field("captcha"), Some("0"));
        assert!(matches!(person.shown.borrow().first(), Some(Task::Grid { image: Some(_), .. })));
    }

    // Answers recorded from jschan c0d50eb (e2e/jschan/servers.sh), redirects followed as
    // ck-web's fetch follows them; pages cut to what's read of them.
    const LOGIN_PAGE: &str = "<!DOCTYPE html><html><head><title>Login</title></head></html>";
    const REFUSED_JSON: &str = r#"{"title":"Forbidden","message":"Incorrect login credentials","redirect":"/login.html?goto=%2Faccount.html"}"#;

    #[test]
    fn an_account_logs_in_is_checked_and_logs_out() {
        use super::super::fake::Site;
        let account = Account { root: "http://127.0.0.1:7100".into(), site: "ptchan".into() };
        let answers = |code: &str| ["ck user".to_string(), "p&ss=w0rd".into(), code.into()];
        // Refused, without a code: the code may be what's missing.
        let site = Site::default().answers("/forms/login", 403, REFUSED_JSON).answers("/forms/login", 403, REFUSED_JSON);
        let why = account.log_in(&site, &answers("")).unwrap_err().to_string();
        assert!(why.starts_with("Incorrect login credentials: ") && why.contains("2FA code"), "{why}");
        assert_eq!(site.sent("/forms/login").body.as_deref(), Some("username=ck%20user&password=p%26ss%3Dw0rd"));
        assert_eq!(account.log_in(&site, &answers(" 123456 ")).unwrap_err().to_string(), "Incorrect login credentials");
        assert_eq!(site.sent("/forms/login").body.as_deref(), Some("username=ck%20user&password=p%26ss%3Dw0rd&twofactor=123456"));
        // In: its account page, and /csrf.json answers its token.
        let site = Site::default()
            .answers("/forms/login", 200, "<!DOCTYPE html><html><head><title>Account</title></head></html>")
            .answers("/csrf.json", 200, r#"{"token":"QLfCu5WV-FM4YjvJTtMDen_UnSBrOfGFEVlY"}"#)
            .answers("/csrf.json", 200, r#"{"token":"QLfCu5WV-FM4YjvJTtMDen_UnSBrOfGFEVlY"}"#)
            // Out: sent to the front page (none without nginx), then to the login page.
            .answers("/forms/logout", 404, "<!DOCTYPE html><html><head><title>Error</title></head></html>")
            .answers("/csrf.json", 200, LOGIN_PAGE)
            .answers("/csrf.json", 200, LOGIN_PAGE);
        account.log_in(&site, &answers("")).unwrap();
        assert!(account.logged_in(&site).unwrap());
        account.log_out(&site).unwrap();
        assert!(!account.logged_in(&site).unwrap());
        assert_eq!((account.name(), account.asks().iter().filter(|f| f.optional).count()), ("ptchan account".to_string(), 1));
    }

    #[test]
    fn logged_in_a_post_goes_without_the_captcha_first() {
        use super::super::fake::{Person, Site, session};
        let site = Site::default()
            .answers("/capt/settings.json", 200, r#"{"captchaMode":2,"maxFiles":5}"#)
            .answers("/csrf.json", 200, r#"{"token":"F8Lo7LFF-isTp5Y_nsdEaePkm7SfANouBM84"}"#)
            // A plain account: the site still wants it.
            .answers("/forms/board/capt/post", 403, r#"{"title":"Forbidden","message":"Incorrect captcha answer"}"#)
            .answers("/settings.json", 200, r#"{"captchaOptions":{"language":"en-GB","type":"text"}}"#)
            .answers("/captcha", 200, &png_b64())
            .answers("/forms/board/capt/post", 200, r#"{"postId":1,"redirect":"/capt/thread/1.html#1"}"#);
        let person = Person::says(&[Some("abc123")]);
        let to = Where { site: "ptchan".into(), board: "capt".into(), thread: 0 };
        let draft = Draft { comment: "hi".into(), password: "pw".into(), ..Default::default() };
        let jschan = Jschan { root: "http://localhost:7101".into(), site: "ptchan".into() };
        assert_eq!(jschan.post(&session(&site, &person), &to, &draft).unwrap(), Posted { thread: 1, no: 1 });
        assert_eq!(site.sent("/forms/board/capt/post").field("captcha"), Some("abc123"));
        assert_eq!(person.shown.borrow().len(), 1);
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
