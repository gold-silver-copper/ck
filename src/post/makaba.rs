//! 2ch (makaba): posts go to `/user/posting`, in JSON. Its captcha is emoji: a picture with
//! a few icons in it and a keyboard of icons, picked one at a time (each pick a request,
//! answered with the next picture, or a token once it's done), plus a proof of work: the
//! number whose SHA-512, put in its template, is the hash it gives. Requests use paths of the
//! page's own host: 2ch.hk sends its pages on to 2ch.su.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha512};

use super::{Answered, Draft, Poster, Sent, Where, said};
use crate::captcha::{Captcha, Cell, Challenge, Task, decode};
use image::{DynamicImage, Rgba, RgbaImage, imageops};
use crate::http::encode_segment as enc;
use crate::web::{Fetch, Helper, Upload};

pub struct Makaba {
    pub root: String,
}

/// What a challenge's id carries: the captcha's token, and the proof of work's answer.
fn id(token: &str, work: u64) -> String {
    json!({"token": token, "work": work}).to_string()
}

impl Poster for Makaba {
    fn page(&self) -> String {
        format!("{}/robots.txt", self.root)
    }

    fn captcha(&self, web: &Helper, to: &Where) -> Result<Captcha> {
        let v = web.fetch(Fetch::get(&format!("/api/captcha/emoji/id?board={}", enc(&to.board))))?.json()?;
        if let Some(why) = v.get("banned").or_else(|| v.get("warning")).and_then(Value::as_str) {
            return Ok(Captcha::Refused(crate::markup::strip_tags(why)));
        }
        match v.get("result").and_then(Value::as_i64) {
            // A passcode's, or captchas are off.
            Some(2 | 3) => return Ok(super::no_captcha("")),
            Some(1) => {}
            _ => bail!("2ch didn't give a captcha now: try again in a moment"),
        }
        let token = v.get("id").and_then(Value::as_str).context("2ch's captcha came without its id")?;
        let work = v.get("challenge").map(proof_of_work).transpose()?.unwrap_or(0);
        let show = web.fetch(Fetch::get(&format!("/api/captcha/emoji/show?id={token}")))?.json()?;
        Ok(Captcha::Challenge(emoji(&id(token, work), &show, Instant::now())?))
    }

    fn answer(&self, web: &Helper, challenge: &Challenge, answer: &str) -> Result<Answered> {
        let data: Value = serde_json::from_str(&challenge.id).unwrap_or_default();
        let (token, work) = (data.get("token").and_then(Value::as_str).unwrap_or_default(), data.get("work").and_then(Value::as_u64).unwrap_or(0));
        if matches!(challenge.task, Task::None) {
            return Ok(Answered::Done(Vec::new()));
        }
        let pick = answer.find('1').context("Pick an icon")?;
        let body = json!({"captchaTokenID": token, "emojiNumber": pick}).to_string();
        let r = web.fetch(Fetch { method: "POST", url: "/api/captcha/emoji/click", body: Some(body), ..Default::default() }.header("Content-Type", "application/json"))?;
        let v = r.json()?;
        if let Some(done) = v.get("success").and_then(Value::as_str) {
            return Ok(Answered::Done(vec![("captcha_type".into(), "emoji_captcha".into()), ("emoji_captcha_id".into(), done.into()), ("2ch_challenge".into(), work.to_string())]));
        }
        if v.get("image").is_some() {
            return Ok(Answered::Next(emoji(&challenge.id, &v, Instant::now())?));
        }
        Err(error(&v))
    }

    fn post(&self, web: &Helper, to: &Where, draft: &Draft, captcha: &[(String, String)]) -> Result<Sent> {
        let mut fields: Vec<(String, String)> = [("task", "post"), ("usercode", ""), ("code", ""), ("makaka_id", ""), ("makaka_answer", "")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        fields.extend([("board".into(), to.board.clone()), ("thread".into(), to.thread.to_string()), ("email".into(), draft.email.clone()), ("name".into(), draft.name.clone()), ("subject".into(), draft.subject.clone()), ("comment".into(), draft.comment.clone())]);
        if draft.sage() {
            fields.push(("sage".into(), "on".into()));
        }
        fields.extend(captcha.iter().cloned());
        let referer = match to.thread {
            0 => format!("/{}/", enc(&to.board)),
            t => format!("/{}/res/{t}.html", enc(&to.board)),
        };
        let file = draft.file.as_ref().map(|p| Upload { field: "file[]".into(), path: p.display().to_string() });
        let v = web.fetch(Fetch { file, ..Fetch::post("/user/posting?nc=1", fields) }.header("Referer", &referer))?.json()?;
        let (thread, no) = posted(to, &v)?;
        Ok(Sent::Posted { thread, no })
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
fn emoji(id: &str, v: &Value, now: Instant) -> Result<Challenge> {
    let image = v.get("image").and_then(Value::as_str).and_then(decode).context("2ch's captcha picture didn't come")?;
    let keys: Vec<_> = v.get("keyboard").and_then(Value::as_array).into_iter().flatten().filter_map(|k| k.as_str().and_then(decode)).collect();
    let key = format!("captcha:makaba:{}", super::next_key());
    // The cells are the numbers in the picture: nothing to show in them but their number.
    let cells = keys.iter().map(|_| Cell::Label(String::new())).collect();
    let task = Task::Grid { prompt: "Pick each icon that's in the picture, one at a time".into(), image: Some(key.clone()), cells, single: true };
    Ok(Challenge { id: id.into(), expires: super::after(now, 300), task, pictures: vec![(key, sheet(&image, &keys))] })
}

/// The picture with the keys under it, four to a row, each in a white square with its number.
fn sheet(image: &DynamicImage, keys: &[DynamicImage]) -> DynamicImage {
    const SLOT: u32 = 80;
    const COLS: u32 = 4;
    let rows = u32::try_from(keys.len()).unwrap_or(0).div_ceil(COLS);
    let width = image.width().max(SLOT.saturating_mul(COLS));
    let top = image.height().saturating_add(8);
    let mut out = RgbaImage::from_pixel(width, top.saturating_add(rows.saturating_mul(SLOT)), Rgba([255, 255, 255, 255]));
    imageops::overlay(&mut out, &image.to_rgba8(), 0, 0);
    for (n, key) in (0u32..).zip(keys) {
        let (x, y) = ((n % COLS).saturating_mul(SLOT), top.saturating_add((n / COLS).saturating_mul(SLOT)));
        let icon = key.to_rgba8();
        let (dx, dy) = (SLOT.saturating_sub(icon.width()) / 2, SLOT.saturating_sub(icon.height()).saturating_add(10) / 2);
        imageops::overlay(&mut out, &icon, i64::from(x.saturating_add(dx)), i64::from(y.saturating_add(dy)));
        number(&mut out, n.saturating_add(1), x.saturating_add(3), y.saturating_add(3));
    }
    DynamicImage::ImageRgba8(out)
}

/// `n` written at `x`, `y` in a small blocky hand, three pixels to a dot.
fn number(img: &mut RgbaImage, n: u32, x: u32, y: u32) {
    // Each digit's 3x5 dots, a row a number (the top bit on the left).
    const DIGITS: [[u8; 5]; 10] = [
        [7, 5, 5, 5, 7],
        [2, 6, 2, 2, 7],
        [7, 1, 7, 4, 7],
        [7, 1, 7, 1, 7],
        [5, 5, 7, 1, 1],
        [7, 4, 7, 1, 7],
        [7, 4, 7, 5, 7],
        [7, 1, 1, 1, 1],
        [7, 5, 7, 5, 7],
        [7, 5, 7, 1, 7],
    ];
    const DOT: u32 = 3;
    for (i, d) in (0u32..).zip(n.to_string().bytes()) {
        let Some(rows) = DIGITS.get(usize::from(d.saturating_sub(b'0'))) else { continue };
        let left = x.saturating_add(i.saturating_mul(DOT.saturating_mul(4)));
        for (row, bits) in (0u32..).zip(rows) {
            for col in 0..3u32 {
                if bits & (4 >> col) == 0 {
                    continue;
                }
                for (px, py) in (0..DOT).flat_map(|a| (0..DOT).map(move |b| (a, b))) {
                    let (ix, iy) = (left.saturating_add(col.saturating_mul(DOT).saturating_add(px)), y.saturating_add(row.saturating_mul(DOT).saturating_add(py)));
                    if ix < img.width() && iy < img.height() {
                        img.put_pixel(ix, iy, Rgba([200, 20, 40, 255]));
                    }
                }
            }
        }
    }
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
        assert_eq!((s.width(), s.height()), (320, 268));
        // "1" in the first square's corner.
        assert_eq!(s.to_rgba8().get_pixel(6, 111), &Rgba([200, 20, 40, 255]));
    }

    #[test]
    fn what_2ch_answers_a_post_with() {
        let reply = Where { site: "2ch".into(), board: "test".into(), thread: 5 };
        assert_eq!(posted(&reply, &json!({"result": 1, "num": 8})).unwrap(), (5, 8));
        assert_eq!(posted(&Where { thread: 0, ..reply.clone() }, &json!({"result": 1, "thread": 9})).unwrap(), (9, 9));
        assert_eq!(posted(&reply, &json!({"result": 0, "error": {"code": -5, "message": "Капча невалидна"}})).unwrap_err().to_string(), "Капча невалидна");
    }
}
