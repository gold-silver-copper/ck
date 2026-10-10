//! A site's captcha, as its engine turned it into something to answer in the terminal (see
//! `crate::post`), and answering it.

use std::time::Instant;

use image::{DynamicImage, Rgba, RgbaImage};

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

/// Pictures in one picture, `cols` to a row, each in a white square numbered from 1 in its
/// corner: a captcha's grid drawn whole (one picture sits well in every terminal, where many
/// small ones don't).
pub fn sheet(pictures: &[DynamicImage], cols: u32) -> DynamicImage {
    let cols = cols.max(1);
    let rows = u32::try_from(pictures.len()).unwrap_or(0).div_ceil(cols);
    // Room for the largest, and for its number above it.
    let slot = pictures.iter().map(|p| p.width().max(p.height())).max().unwrap_or(0).saturating_add(24);
    let mut out = RgbaImage::from_pixel(slot.saturating_mul(cols), slot.saturating_mul(rows), Rgba([255, 255, 255, 255]));
    for (n, picture) in (0u32..).zip(pictures) {
        let (x, y) = ((n % cols).saturating_mul(slot), (n / cols).saturating_mul(slot));
        let p = picture.to_rgba8();
        let (dx, dy) = (slot.saturating_sub(p.width()) / 2, slot.saturating_sub(p.height()).saturating_add(16) / 2);
        image::imageops::overlay(&mut out, &p, i64::from(x.saturating_add(dx)), i64::from(y.saturating_add(dy)));
        number(&mut out, n.saturating_add(1), x.saturating_add(3), y.saturating_add(3));
    }
    DynamicImage::ImageRgba8(out)
}

/// `n` written at `x`, `y` in a small blocky hand, three pixels to a dot.
pub fn number(img: &mut RgbaImage, n: u32, x: u32, y: u32) {
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
