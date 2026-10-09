//! 4chan's captcha, as its captcha page hands it over (the "twister", read the way 4chan's
//! own `tcaptcha.js` reads it), and answering it.

use std::time::{Duration, Instant};

use base64::Engine;
use image::DynamicImage;
use serde_json::Value;

/// Where 4chan keeps the images of its newer captchas (as its script has it).
const IMAGES: &str = "https://s.4cdn.org/image/temp/april2026";

/// What the captcha page said.
#[derive(Debug)]
pub enum Twister {
    /// It won't give one now: why.
    Refused(String),
    /// Not for a while (posting too often): how long, and its message.
    Wait { until: Instant, message: String },
    Challenge(Challenge),
}

#[derive(Debug)]
pub struct Challenge {
    /// Sent back with the answer.
    pub id: String,
    /// When it stops being accepted.
    pub expires: Instant,
    pub task: Task,
}

/// What it asks.
#[derive(Debug)]
pub enum Task {
    /// Nothing: "verification not required".
    None,
    /// Steps, each a slider through pictures: pick the one that fits.
    Slider(Vec<Step>),
    /// Type what a picture shows.
    Text { prompt: String, image: Option<String> },
    /// Pick the pictures (or words) that fit: one, or any number.
    Grid { prompt: String, cells: Vec<Cell>, single: bool },
}

#[derive(Debug)]
pub struct Step {
    /// What to look for: words, or a picture.
    pub prompt: Prompt,
    pub items: Vec<DynamicImage>,
}

#[derive(Debug)]
pub enum Prompt {
    Text(String),
    Image(DynamicImage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    /// A picture, by its URL.
    Image(String),
    Label(String),
}

impl Twister {
    pub fn parse(v: &Value, now: Instant) -> Twister {
        let str_of = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let secs = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            return Twister::Refused(strip_tags(e));
        }
        if secs("pcd") > 0 {
            let message = v.get("pcd_msg").and_then(Value::as_str).map_or_else(|| "Please wait a while.".into(), strip_tags);
            return Twister::Wait { until: now + Duration::from_secs(secs("pcd")), message };
        }
        // Answers aren't taken in the last few seconds (as 4chan's script counts).
        let ttl = Duration::from_secs(secs("ttl").max(10)).saturating_sub(Duration::from_secs(3));
        let task = match v.get("extTask") {
            Some(ext) => ext_task(ext),
            None => match v.get("tasks").and_then(Value::as_array) {
                Some(tasks) => Task::Slider(tasks.iter().map(step).collect()),
                None => Task::None,
            },
        };
        Twister::Challenge(Challenge { id: str_of("challenge"), expires: now + ttl, task })
    }
}

fn step(t: &Value) -> Step {
    let prompt = match t.get("img").and_then(Value::as_str).and_then(decode) {
        Some(img) => Prompt::Image(img),
        None => Prompt::Text(t.get("str").and_then(Value::as_str).map(strip_tags).unwrap_or_default()),
    };
    let items = t.get("items").and_then(Value::as_array).map(|a| a.iter().filter_map(|i| i.as_str().and_then(decode)).collect()).unwrap_or_default();
    Step { prompt, items }
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
            Task::Grid { prompt, cells, single }
        }
        _ => Task::Text { prompt, image: ext.get("img").and_then(url) },
    }
}

fn decode(b64: &str) -> Option<DynamicImage> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    image::load_from_memory(&bytes).ok()
}

/// The text of a bit of HTML (4chan's messages have links and line breaks).
pub fn strip_tags(html: &str) -> String {
    let breaks = html.replace("<br>", "\n").replace("<br/>", "\n").replace("<br />", "\n");
    let mut out = String::new();
    let mut in_tag = false;
    for c in breaks.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    html_escape::decode_html_entities(&out).trim().to_string()
}

/// Answering a challenge: where the person is in it, and what they've picked.
#[derive(Debug)]
pub struct Solving {
    pub challenge: Challenge,
    /// The slider's step, and its position (0 shows the prompt; then the items, from 1).
    pub step: usize,
    pub slide: usize,
    /// The slider's picks so far.
    pub picks: String,
    /// The text typed.
    pub typed: String,
    /// The grid's cells picked, and the one the cursor is on.
    pub chosen: Vec<bool>,
    pub at: usize,
}

impl Solving {
    pub fn new(challenge: Challenge) -> Self {
        let cells = match &challenge.task {
            Task::Grid { cells, .. } => cells.len(),
            _ => 0,
        };
        Solving { challenge, step: 0, slide: 0, picks: String::new(), typed: String::new(), chosen: vec![false; cells], at: 0 }
    }

    /// The slider's step being answered.
    pub fn current(&self) -> Option<&Step> {
        match &self.challenge.task {
            Task::Slider(steps) => steps.get(self.step),
            _ => None,
        }
    }

    /// Move the slider by `by`, within its items (and the prompt, at 0).
    pub fn slide(&mut self, by: isize) {
        let n = self.current().map_or(0, |s| s.items.len());
        self.slide = self.slide.saturating_add_signed(by).min(n);
    }

    /// Take the slider's pick and go on; whether that was the last step.
    pub fn next_step(&mut self) -> bool {
        let Some(pick) = self.slide.checked_sub(1) else { return false };
        self.picks.push_str(&pick.to_string());
        self.step += 1;
        self.slide = 0;
        self.current().is_none()
    }

    pub fn toggle(&mut self, cell: usize) {
        let single = matches!(self.challenge.task, Task::Grid { single: true, .. });
        let was = self.chosen.get(cell).copied().unwrap_or(false);
        if single {
            self.chosen.iter_mut().for_each(|c| *c = false);
        }
        if let Some(c) = self.chosen.get_mut(cell) {
            *c = !was;
        }
    }

    /// The answer, as 4chan's form sends it; None while it isn't finished.
    pub fn answer(&self) -> Option<String> {
        match &self.challenge.task {
            Task::None => Some(String::new()),
            Task::Slider(steps) => (self.step >= steps.len()).then(|| self.picks.clone()),
            Task::Text { .. } => Some(self.typed.to_lowercase().chars().filter(char::is_ascii_alphanumeric).collect::<String>()).filter(|a| !a.is_empty()),
            Task::Grid { .. } => self.chosen.contains(&true).then(|| self.chosen.iter().map(|&c| if c { '1' } else { '0' }).collect()),
        }
    }

    pub fn expired(&self, now: Instant) -> bool {
        now >= self.challenge.expires
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A 1x1 PNG, base64.
    fn png() -> String {
        let mut out = std::io::Cursor::new(Vec::new());
        let _ = DynamicImage::new_rgb8(1, 1).write_to(&mut out, image::ImageFormat::Png);
        base64::engine::general_purpose::STANDARD.encode(out.into_inner())
    }

    fn challenge(v: &Value) -> Solving {
        match Twister::parse(v, Instant::now()) {
            Twister::Challenge(c) => Solving::new(c),
            other => panic!("not a challenge: {other:?}"),
        }
    }

    #[test]
    fn slider_steps_answer_with_their_picks() {
        let img = png();
        let mut s = challenge(&json!({"challenge": "abc", "ttl": 120, "tasks": [
            {"str": "Pick the <b>cat</b>", "items": [img, img, img]},
            {"img": img, "items": [img, img]},
        ]}));
        assert!(matches!(s.current().map(|c| &c.prompt), Some(Prompt::Text(t)) if t == "Pick the cat"));
        // The prompt can't be picked; past the last item stays on it.
        assert!(!s.next_step());
        s.slide(5);
        assert_eq!(s.slide, 3);
        assert!(!s.next_step());
        assert!(matches!(s.current().map(|c| &c.prompt), Some(Prompt::Image(_))));
        s.slide(1);
        assert_eq!(s.answer(), None);
        assert!(s.next_step());
        assert_eq!((s.challenge.id.as_str(), s.answer().as_deref()), ("abc", Some("20")));
    }

    #[test]
    fn newer_tasks_text_and_grid() {
        let mut text = challenge(&json!({"challenge": "c", "ttl": 60, "extTask": {"mode": 1, "str": "Type it", "imgRoot": "r", "img": ["a.png", 300, 80]}}));
        assert!(matches!(&text.challenge.task, Task::Text { image: Some(u), .. } if u == "https://s.4cdn.org/image/temp/april2026/r/a.png"));
        text.typed = "Ab 3-x".into();
        assert_eq!(text.answer().as_deref(), Some("ab3x"));
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
        assert!(matches!(Twister::parse(&json!({"error": "You have to wait <b>a bit</b>."}), now), Twister::Refused(e) if e == "You have to wait a bit."));
        assert!(matches!(Twister::parse(&json!({"pcd": 30, "pcd_msg": "Slow down."}), now), Twister::Wait { until, message } if until == now + Duration::from_secs(30) && message == "Slow down."));
    }
}
