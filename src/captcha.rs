//! A site's captcha, as its engine turned it into something to answer in the terminal (see
//! `crate::post`), and answering it.

use std::time::Instant;

use image::DynamicImage;

/// What a site said when asked for a captcha.
#[derive(Debug)]
pub enum Captcha {
    /// It won't give one now: why.
    Refused(String),
    /// Not for a while (posting too often): how long, and its message.
    Wait { until: Instant, message: String },
    Challenge(Challenge),
}

#[derive(Debug, Clone)]
pub struct Challenge {
    /// Its engine's, to go with the answer.
    pub id: String,
    /// When it stops being accepted.
    pub expires: Instant,
    pub task: Task,
    /// Pictures that came with it rather than by a URL (by the key `task` uses for them), to be
    /// drawn like any other.
    pub pictures: Vec<(String, DynamicImage)>,
}

/// What it asks.
#[derive(Debug, Clone)]
pub enum Task {
    /// Nothing: "verification not required".
    None,
    /// Steps, each a slider through pictures: pick the one that fits.
    Slider(Vec<Step>),
    /// Type what a picture (by its URL or key) shows.
    Text { prompt: String, image: Option<String> },
    /// Pick the pictures (or words) that fit: one, or any number; under a picture they're
    /// about, if there's one.
    Grid { prompt: String, image: Option<String>, cells: Vec<Cell>, single: bool },
}

/// A slider step: pick the item (a strip of shapes) that has the reference (the shape to find)
/// in it.
#[derive(Debug, Clone)]
pub struct Step {
    pub text: String,
    pub reference: Option<DynamicImage>,
    pub items: Vec<DynamicImage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    /// A picture, by its URL or key.
    Image(String),
    Label(String),
}

/// A picture in base64.
pub fn decode(b64: &str) -> Option<DynamicImage> {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()?;
    image::load_from_memory(&bytes).ok()
}

/// The first picture in a bit of HTML that's inline (a `data:` URL, base64).
pub fn inline_image(html: &str) -> Option<DynamicImage> {
    let at = html.find("base64,")?.checked_add("base64,".len())?;
    let rest = html.get(at..)?;
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))).unwrap_or(rest.len());
    decode(rest.get(..end)?)
}

/// Answering a challenge: where the person is in it, and what they've picked.
#[derive(Debug)]
pub struct Solving {
    pub challenge: Challenge,
    /// The slider's step, and the item it's on.
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

    /// Move the slider by `by`, within its items.
    pub fn slide(&mut self, by: isize) {
        let last = self.current().map_or(0, |s| s.items.len().saturating_sub(1));
        self.slide = self.slide.saturating_add_signed(by).min(last);
    }

    /// Take the slider's pick and go on; whether that was the last step.
    pub fn next_step(&mut self) -> bool {
        if self.current().is_none_or(|s| s.items.is_empty()) {
            return false;
        }
        self.picks.push_str(&self.slide.to_string());
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

    /// The answer: the slider's picks, what's typed, or the grid's picks as 0s and 1s; None
    /// while it isn't finished.
    pub fn answer(&self) -> Option<String> {
        match &self.challenge.task {
            Task::None => Some(String::new()),
            Task::Slider(steps) => (self.step >= steps.len()).then(|| self.picks.clone()),
            Task::Text { .. } => Some(self.typed.trim().to_string()).filter(|a| !a.is_empty()),
            Task::Grid { .. } => self.chosen.contains(&true).then(|| self.chosen.iter().map(|&c| if c { '1' } else { '0' }).collect()),
        }
    }

    pub fn expired(&self, now: Instant) -> bool {
        now >= self.challenge.expires
    }
}
