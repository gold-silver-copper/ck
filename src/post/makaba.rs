//! 2ch (makaba): posts go to `/user/posting`, in JSON. Its captcha is emoji: a picture with
//! a few icons in it and a keyboard of icons, picked one at a time (each pick a request,
//! answered with the next picture, or a token once it's done), plus a proof of work: the
//! number whose SHA-512, put in its template, is the hash it gives. Requests use paths of the
//! page's own host: 2ch.hk sends its pages on to 2ch.su.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, Rgba, RgbaImage, imageops};
use serde_json::{Value, json};
use sha2::{Digest, Sha512};

use super::{LoginField, Draft, Login, Posted, Poster, Session, Web, Where, said, text_of_class};
use crate::captcha::{Cell, Challenge, Task, decode};
use crate::http::encode_segment as enc;
use crate::web::{Fetch, Upload};

pub struct Makaba {
    pub root: String,
}

impl Makaba {
    /// The captcha's fields once it's done: its proof of work, and its rounds picked one at a
    /// time (a new captcha when they ask for one); none for a passcode, or when captchas are
    /// off.
    fn captcha(&self, s: &Session, board: &str) -> Result<Vec<(String, String)>> {
        'fresh: loop {
            let r = s.web.fetch(Fetch::get(&format!("/api/captcha/emoji/id?board={}", enc(board))))?;
            let v = r.json()?;
            if let Some(why) = v.get("banned").or_else(|| v.get("warning")).and_then(Value::as_str) {
                return Err(said(why));
            }
            match v.get("result").and_then(Value::as_i64) {
                // The passcode goes in the form too, as 2ch's own script puts it there.
                Some(2 | 3) => return Ok(r.cookie(PASSCODE).map(|code| vec![("usercode".into(), code.into())]).unwrap_or_default()),
                Some(1) => {}
                _ => bail!("2ch didn't give a captcha now: try again in a moment"),
            }
            let token = v.get("id").and_then(Value::as_str).context("2ch's captcha came without its id")?;
            let work = v.get("challenge").map(proof_of_work).transpose()?.unwrap_or(0);
            let mut round = s.web.fetch(Fetch::get(&format!("/api/captcha/emoji/show?id={token}")))?.json()?;
            loop {
                let Some(answer) = s.ask.solve(&emoji(&round, Instant::now())?)? else { continue 'fresh };
                let pick = answer.find('1').context("Pick an icon")?;
                let body = json!({"captchaTokenID": token, "emojiNumber": pick}).to_string();
                round = s.web.fetch(Fetch { method: "POST", url: "/api/captcha/emoji/click", body: Some(body), ..Default::default() }.header("Content-Type", "application/json"))?.json()?;
                if let Some(done) = round.get("success").and_then(Value::as_str) {
                    return Ok(vec![("captcha_type".into(), "emoji_captcha".into()), ("emoji_captcha_id".into(), done.into()), ("2ch_challenge".into(), work.to_string())]);
                }
                if round.get("image").is_none() {
                    return Err(error(&round));
                }
            }
        }
    }
}

impl Poster for Makaba {
    fn page(&self) -> String {
        format!("{}/robots.txt", self.root)
    }

    fn login(&self) -> Option<Arc<dyn Login>> {
        Some(Arc::new(Passcode { root: self.root.clone() }))
    }

    fn post(&self, s: &Session, to: &Where, draft: &Draft) -> Result<Posted> {
        let mut fields: Vec<(String, String)> = [("task", "post"), ("usercode", ""), ("code", ""), ("makaka_id", ""), ("makaka_answer", "")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        fields.extend([("board".into(), to.board.clone()), ("thread".into(), to.thread.to_string()), ("email".into(), draft.email.clone()), ("name".into(), draft.name.clone()), ("subject".into(), draft.subject.clone()), ("comment".into(), draft.comment.clone())]);
        if draft.sage() {
            fields.push(("sage".into(), "on".into()));
        }
        for (k, v) in self.captcha(s, &to.board)? {
            match fields.iter_mut().find(|(name, _)| *name == k) {
                Some(field) => field.1 = v,
                None => fields.push((k, v)),
            }
        }
        let referer = match to.thread {
            0 => format!("/{}/", enc(&to.board)),
            t => format!("/{}/res/{t}.html", enc(&to.board)),
        };
        let file = draft.file.as_ref().map(|p| Upload { field: "file[]".into(), path: p.display().to_string() });
        let v = s.web.fetch(Fetch { file, ..Fetch::post("/user/posting?nc=1", fields) }.header("Referer", &referer))?.json()?;
        let (thread, no) = posted(to, &v)?;
        Ok(Posted { thread, no })
    }
}

/// 2ch's passcode: logged in to at `/user/passlogin`, which sets this cookie.
const PASSCODE: &str = "passcode_auth";

pub struct Passcode {
    pub root: String,
}

impl Login for Passcode {
    fn name(&self) -> String {
        "2ch passcode".into()
    }

    fn asks(&self) -> &'static [LoginField] {
        &[LoginField { label: "Passcode", optional: false }]
    }

    fn page(&self) -> String {
        format!("{}/robots.txt", self.root)
    }

    fn logged_in(&self, web: &dyn Web) -> Result<bool> {
        Ok(web.fetch(Fetch::get("/robots.txt"))?.cookie(PASSCODE).is_some_and(|c| !c.is_empty()))
    }

    fn log_in(&self, web: &dyn Web, answers: &[String]) -> Result<()> {
        let [code] = answers else { bail!("A 2ch passcode is one code") };
        let r = web.fetch(Fetch::post("/user/passlogin", vec![("passcode".into(), code.trim().into())]).header("Referer", "/user/passlogin"))?;
        if r.cookie(PASSCODE).is_some_and(|c| !c.is_empty()) {
            return Ok(());
        }
        // Its error page: "Ошибка!" (error), then why.
        match text_of_class(&r.body, "msg__text") {
            Some(why) => bail!("{}", why.trim_start_matches("Ошибка!").trim()),
            None => bail!("2ch didn't log you in (it answered {})", r.status),
        }
    }

    fn log_out(&self, web: &dyn Web) -> Result<()> {
        if web.fetch(Fetch::post("/user/passlogout", Vec::new()))?.cookie(PASSCODE).is_some_and(|c| !c.is_empty()) {
            bail!("2ch didn't log you out");
        }
        Ok(())
    }
}

/// The thread and number of a post 2ch took: `num` for a reply, `thread` for a new thread.
fn posted(to: &Where, v: &Value) -> Result<(u64, u64)> {
    if v.get("result").and_then(Value::as_i64) != Some(1) {
        return Err(error(v));
    }
    match (v.get("num").and_then(crate::http::as_u64), v.get("thread").and_then(crate::http::as_u64)) {
        (Some(no), _) => Ok((to.thread_of(no), no)),
        (None, Some(thread)) => Ok((thread, thread)),
        (None, None) => bail!("2ch didn't say the post's number"),
    }
}

/// 2ch's error: `{error: {code, message}}`.
fn error(v: &Value) -> anyhow::Error {
    let e = v.get("error");
    match e.and_then(|e| e.get("message")).and_then(Value::as_str) {
        Some(m) => said(m),
        None => anyhow::anyhow!("2ch answered {}", v),
    }
}

/// The emoji captcha's round: its picture, with the keyboard of icons to pick from under it
/// in one picture, each icon numbered as its cell is (many small pictures don't sit well in
/// every terminal).
fn emoji(v: &Value, now: Instant) -> Result<Challenge> {
    let image = v.get("image").and_then(Value::as_str).and_then(decode).context("2ch's captcha picture didn't come")?;
    let keys: Vec<_> = v.get("keyboard").and_then(Value::as_array).into_iter().flatten().filter_map(|k| k.as_str().and_then(decode)).collect();
    let key = format!("captcha:makaba:{}", super::next_key());
    // The cells are the numbers in the picture: nothing to show in them but their number.
    let cells = keys.iter().map(|_| Cell::Label(String::new())).collect();
    let task = Task::Grid { prompt: "Pick each icon that's in the picture, one at a time".into(), image: Some(key.clone()), cells, single: true };
    Ok(Challenge { id: String::new(), expires: super::after(now, 300), task, pictures: vec![(key, sheet(&image, &keys))] })
}

/// The picture with the keys under it, four to a row, numbered.
fn sheet(image: &DynamicImage, keys: &[DynamicImage]) -> DynamicImage {
    let keys = crate::captcha::sheet(keys, 4);
    let mut out = RgbaImage::from_pixel(image.width().max(keys.width()), image.height().saturating_add(8).saturating_add(keys.height()), Rgba([255, 255, 255, 255]));
    imageops::overlay(&mut out, &image.to_rgba8(), 0, 0);
    imageops::overlay(&mut out, &keys.to_rgba8(), 0, i64::from(image.height().saturating_add(8)));
    DynamicImage::ImageRgba8(out)
}

/// The proof of work: the number under `limit` whose SHA-512 (in hex), put in `template` for
/// its `%d`, is `hash`.
fn proof_of_work(challenge: &Value) -> Result<u64> {
    let hash = challenge.get("hash").and_then(Value::as_str).context("no hash")?;
    let template = challenge.get("template").and_then(Value::as_str).context("no template")?;
    let limit = challenge.get("limit").and_then(Value::as_u64).unwrap_or(20_000).min(10_000_000);
    (0..limit).find(|i| format!("{:x}", Sha512::digest(template.replace("%d", &i.to_string()))) == hash).context("2ch's proof of work has no answer")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_proof_of_work_finds_its_number() {
        let hash = format!("{:x}", Sha512::digest("abc1234xyz"));
        assert_eq!(proof_of_work(&json!({"hash": hash, "limit": 20000, "template": "abc%dxyz"})).unwrap(), 1234);
        assert!(proof_of_work(&json!({"hash": "0", "limit": 10, "template": "%d"})).is_err());
    }

    #[test]
    fn the_keys_go_under_the_picture_numbered() {
        let keys = vec![DynamicImage::new_rgba8(50, 43); 8];
        let s = sheet(&DynamicImage::new_rgb8(300, 100), &keys);
        // Squares of 74 (the largest key and its number), four to a row, under the picture.
        assert_eq!((s.width(), s.height()), (300, 256));
        // "1" in the first square's corner.
        assert_eq!(s.to_rgba8().get_pixel(6, 111), &Rgba([200, 20, 40, 255]));
    }

    #[test]
    fn a_post_through_the_emoji_rounds_and_proof_of_work() {
        use super::super::fake::{Person, Site, session};
        let site = Site::default()
            .answers("/api/captcha/emoji/id", 200, "fixture:makaba_emoji_id.json")
            .answers("/api/captcha/emoji/show", 200, "fixture:makaba_emoji_show.json")
            .answers("/api/captcha/emoji/click", 200, "fixture:makaba_emoji_show.json")
            .answers("/api/captcha/emoji/click", 200, r#"{"success":"done-token"}"#)
            .answers("/user/posting", 200, r#"{"result":1,"num":252321}"#);
        let person = Person::says(&[Some("00100000"), Some("00000001")]);
        let to = Where { site: "2ch".into(), board: "test".into(), thread: 252013 };
        let draft = Draft { comment: "hi".into(), ..Default::default() };
        let posted = Makaba { root: "https://2ch.hk".into() }.post(&session(&site, &person), &to, &draft).unwrap();
        assert_eq!(posted, Posted { thread: 252013, no: 252321 });
        assert_eq!(site.sent("/emoji/click").body.as_deref().map(|b| b.contains("\"emojiNumber\":7")), Some(true));
        let sent = site.sent("/user/posting");
        let id: Value = serde_json::from_str(&std::fs::read_to_string("tests/fixtures/makaba_emoji_id.json").unwrap()).unwrap();
        let work = proof_of_work(id.get("challenge").unwrap()).unwrap().to_string();
        assert_eq!((sent.field("emoji_captcha_id"), sent.field("2ch_challenge"), sent.field("comment")), (Some("done-token"), Some(work.as_str()), Some("hi")));
        assert!(matches!(person.shown.borrow().first(), Some(Task::Grid { single: true, .. })));
    }

    #[test]
    fn with_a_passcode_a_post_has_no_captcha_and_sends_the_code() {
        use super::super::fake::{Person, Site, session};
        let site = Site::default().answers("/api/captcha/emoji/id", 200, r#"{"result":2}"#).sets("passcode_auth=abc").answers("/user/posting", 200, r#"{"result":1,"num":8}"#);
        let person = Person::default();
        let to = Where { site: "2ch".into(), board: "test".into(), thread: 5 };
        Makaba { root: "https://2ch.hk".into() }.post(&session(&site, &person), &to, &Draft::default()).unwrap();
        let sent = site.sent("/user/posting");
        assert_eq!((sent.field("usercode"), sent.fields.iter().filter(|(k, _)| k == "usercode").count()), (Some("abc"), 1));
        assert!(person.shown.borrow().is_empty());
    }

    #[test]
    fn a_passcode_logs_in_and_out() {
        use super::super::fake::Site;
        let pass = Passcode { root: "https://2ch.hk".into() };
        let site = Site::default().answers("/user/passlogin", 200, "").sets("passcode_auth=abc");
        pass.log_in(&site, &[" code ".into()]).unwrap();
        assert_eq!(site.sent("/user/passlogin").field("passcode"), Some("code"));
        // 2ch's answer to a wrong one, as it came: "the passcode doesn't exist".
        let site = Site::default().answers("/user/passlogin", 500, "fixture:makaba_passlogin_wrong.html");
        assert_eq!(pass.log_in(&site, &["code".into()]).unwrap_err().to_string(), "Пасскод не существует.");
        let site = Site::default().answers("/robots.txt", 200, "").answers("/user/passlogout", 200, "");
        assert!(!pass.logged_in(&site).unwrap());
        pass.log_out(&site).unwrap();
    }

    #[test]
    fn what_2ch_answers_a_post_with() {
        let reply = Where { site: "2ch".into(), board: "test".into(), thread: 5 };
        assert_eq!(posted(&reply, &json!({"result": 1, "num": 8})).unwrap(), (5, 8));
        assert_eq!(posted(&Where { thread: 0, ..reply.clone() }, &json!({"result": 1, "thread": 9})).unwrap(), (9, 9));
        assert_eq!(posted(&reply, &json!({"result": 0, "error": {"code": -5, "message": "Капча невалидна"}})).unwrap_err().to_string(), "Капча невалидна");
    }
}
