//! Background image loading and a bounded in-memory cache of decoded images.
//!
//! The UI asks for images every frame with `get`; anything it didn't ask for in a frame is
//! dropped from the fetch queue, so scrolling past a page of thumbnails doesn't fetch them.
//! Thumbnails are also kept on disk. On hosts that share the API's rate limit, one image is
//! fetched at a time, in the order the UI asked (top to bottom).
//!
//! Turning an image into terminal output (sixel, kitty, half-blocks) takes milliseconds, so
//! it happens on an encoder thread too; `get` only ever hands out finished encodings.
#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use image::DynamicImage;
use image::imageops::FilterType;
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;

use crate::disk_cache::DiskCache;
use crate::http::{self, lock};

const WORKERS: usize = 4;
/// Decoded images (plus their encoded protocols, estimated at the same size) kept in memory.
const BUDGET_BYTES: usize = 96 * 1024 * 1024;
const MAX_DOWNLOAD: u64 = 25 * 1024 * 1024;
/// Full-size images are scaled down to this before caching; no terminal shows more.
const MAX_DIM: u32 = 2048;
/// A new size for an image that's already shown (a resize) is encoded once it has held
/// this long, instead of on every step of the resize.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);
/// Animations redraw at most this often (20 per second); frames due sooner are skipped,
/// so they keep their speed.
const MIN_FRAME: Duration = Duration::from_millis(50);
/// An animated GIF's decoded frames are scaled down to fit in this (well within the cache
/// budget); one that would need more than 4x less is shown still.
const ANIMATION_BYTES: usize = 40 * 1024 * 1024;
/// The most one decoded GIF frame (or the canvas it's drawn on) may take.
const GIF_FRAME_ALLOC: u64 = 128 * 1024 * 1024;
/// No real image is wider or taller; past this a file is broken or hostile.
const MAX_SIDE: u32 = 16384;
/// The most decoding one image may take: a full-size one, and a thumbnail.
const FULL_ALLOC: u64 = 256 * 1024 * 1024;
const THUMB_ALLOC: u64 = 64 * 1024 * 1024;

/// An animated GIF's frames and how long each shows.
type Frames = Arc<Vec<(DynamicImage, Duration)>>;

/// An animation encoded for one size, playing.
struct Animation {
    size: Size,
    frames: Vec<(Protocol, Duration)>,
    total: Duration,
    start: Instant,
    /// Paused at this point in the loop.
    paused: Option<Duration>,
}

impl Animation {
    fn at(&self, now: Instant) -> Duration {
        let t = self.paused.unwrap_or_else(|| now.duration_since(self.start));
        let nanos = t.as_nanos().checked_rem(self.total.as_nanos()).unwrap_or(0);
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    /// The frame showing now, and how long until the next one.
    fn frame(&self, now: Instant) -> (usize, Duration) {
        let mut t = self.at(now);
        for (i, (_, d)) in self.frames.iter().enumerate() {
            if t < *d {
                return (i, d.saturating_sub(t));
            }
            t = t.saturating_sub(*d);
        }
        (0, self.frames.first().map_or(Duration::ZERO, |f| f.1))
    }
}

/// Thumbnails are cached on disk; full-size images only in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Thumb,
    Full,
}

impl Kind {
    /// The most decoding one may take.
    fn alloc(self) -> u64 {
        match self {
            Kind::Thumb => THUMB_ALLOC,
            Kind::Full => FULL_ALLOC,
        }
    }
}

pub enum State<'a> {
    Loading,
    /// Offline (a saved copy), and not on disk: it won't be fetched.
    Unavailable,
    /// Downloaded, being encoded for this size.
    Rendering,
    Failed,
    Ready(&'a Protocol),
}

/// Part of an image, zoomed in: `zoom` percent (100: all of it, fitted), centered at
/// (`x`, `y`) in thousandths of its width and height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Crop {
    pub zoom: u16,
    pub x: u16,
    pub y: u16,
}

/// Zoom levels, in percent.
const ZOOMS: [u16; 7] = [100, 150, 200, 300, 400, 600, 800];

impl Crop {
    pub const FIT: Crop = Crop { zoom: 100, x: 500, y: 500 };

    pub fn is_fit(self) -> bool {
        self.zoom <= 100
    }

    /// One zoom level in (or out), keeping the center.
    pub fn zoomed(self, zoom_in: bool) -> Crop {
        let i = ZOOMS.iter().position(|&z| z >= self.zoom).unwrap_or(0);
        let zoom = if zoom_in { ZOOMS.get(i.saturating_add(1)).or(ZOOMS.last()) } else { ZOOMS.get(i.saturating_sub(1)) };
        let zoom = zoom.copied().unwrap_or(100);
        if zoom <= 100 {
            return Crop::FIT;
        }
        Crop { zoom, ..self }
    }

    /// Moved by a quarter of what's shown (`shown`: thousandths of the image's width and
    /// height on screen), in steps of `dx`, `dy`; never past the image's edges.
    pub fn moved(self, dx: i32, dy: i32, shown: (u16, u16)) -> Crop {
        let step = |s: u16| (i32::from(s) / 4).max(1);
        let at = |v: u16, d: i32, s: u16| u16::try_from(i32::from(v).saturating_add(d.saturating_mul(step(s))).clamp(0, 1000)).unwrap_or_default();
        Crop { x: at(self.x, dx, shown.0), y: at(self.y, dy, shown.1), ..self }.within(shown)
    }

    /// How much is shown before it's known (not drawn yet): the image's shape at this zoom.
    pub fn guess_shown(self) -> (u16, u16) {
        let s = u16::try_from(100_000u32.checked_div(u32::from(self.zoom.max(100))).unwrap_or(0)).unwrap_or(u16::MAX);
        (s, s)
    }

    /// The center kept where what's shown stays inside the image.
    pub fn within(self, shown: (u16, u16)) -> Crop {
        let keep = |v: u16, s: u16| {
            let half = s.min(1000) / 2;
            v.clamp(half, 1000u16.saturating_sub(half))
        };
        Crop { x: keep(self.x, shown.0), y: keep(self.y, shown.1), ..self }
    }

    /// How much of a `w` x `h` image is shown zoomed into `view` (pixels on screen): in
    /// thousandths of its width and height. Zoomed in, that's as much as the screen holds
    /// at that zoom (its shape, not the image's), up to all of it.
    pub fn shown(self, w: u32, h: u32, view: (u32, u32)) -> (u16, u16) {
        let (_, _, rw, rh) = self.region(w, h, view);
        let part = |r: u32, of: u32| u16::try_from(u64::from(r).saturating_mul(1000).checked_div(u64::from(of.max(1))).unwrap_or(0)).unwrap_or(u16::MAX);
        (part(rw, w), part(rh, h))
    }

    /// The part of a `w` x `h` image it shows on a screen area of `view` pixels: left, top,
    /// width, height. At 100% the whole image fits the area; zoomed by `z`, the image is `z`
    /// times that size, and the part is what the area holds of it.
    pub fn region(self, w: u32, h: u32, view: (u32, u32)) -> (u32, u32, u32, u32) {
        let (w, h) = (w.max(1), h.max(1));
        let (aw, ah) = (f64::from(view.0.max(1)), f64::from(view.1.max(1)));
        let scale = f64::min(aw / f64::from(w), ah / f64::from(h)) * f64::from(self.zoom.max(100)) / 100.0;
        #[allow(clippy::cast_possible_truncation)] // f64 as u32 saturates
        let (rw, rh) = (((aw / scale).round() as u32).clamp(1, w), ((ah / scale).round() as u32).clamp(1, h));
        let at = |side: u32, thousandths: u16| u32::try_from(u64::from(side).saturating_mul(u64::from(thousandths)) / 1000).unwrap_or(u32::MAX);
        let (left, top) = (at(w, self.x), at(h, self.y));
        (left.saturating_sub(rw / 2).min(w.saturating_sub(rw)), top.saturating_sub(rh / 2).min(h.saturating_sub(rh)), rw, rh)
    }
}

/// What an encoding is for: a size in cells, and the part of the image.
type View = (Size, Crop);

enum Slot {
    Loading,
    Failed,
    Ready {
        img: Arc<DynamicImage>,
        protos: Vec<(View, Protocol)>,
        /// The view being encoded, if any.
        pending: Option<View>,
        /// A view asked for since, and when (for the resize debounce).
        asked: Option<(View, Instant)>,
        /// The last view that couldn't be encoded, not asked for again.
        failed: Option<View>,
        bytes: usize,
        used: u64,
        /// An animated GIF's frames, their encoding, and the size being encoded.
        frames: Option<Frames>,
        animation: Option<Animation>,
        animating: Option<Size>,
    },
}

/// Fetch queue shared with the workers.
#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    cv: Condvar,
}

#[derive(Default)]
struct QueueState {
    /// URL, kind, and the size to encode for once it's decoded (if the UI said).
    jobs: VecDeque<(String, Kind, Option<Size>)>,
    /// What the UI asked for in the last frame.
    wanted: HashSet<String>,
    /// Rate-limited (non-media) hosts with a fetch in progress.
    busy: HashSet<String>,
}

enum Done {
    /// Not fetched: it scrolled out of view before a worker got to it.
    Skipped(String),
    /// Decoded, and already sent to be encoded at this size if the UI had given one.
    Fetched(String, Result<(Arc<DynamicImage>, Option<Frames>), String>, Option<View>),
    Encoded(String, View, Result<Protocol, String>),
    EncodedFrames(String, Size, Result<Vec<(Protocol, Duration)>, String>),
}

enum EncodeJob {
    One(String, View, Arc<DynamicImage>),
    Frames(String, Size, Frames),
}

/// The image protocol to use: the one `images` names, or what the terminal said it can
/// show. Zellij answers for the terminal it runs in, and says sixel whatever that terminal
/// is (most can't show it, and then nothing would show at all): there, half-blocks, unless
/// `images = "sixel"` says otherwise.
pub fn choose_protocol(mode: crate::config::ImagesMode, detected: ProtocolType, in_zellij: bool) -> ProtocolType {
    use crate::config::ImagesMode;
    match mode {
        ImagesMode::Halfblocks => ProtocolType::Halfblocks,
        ImagesMode::Sixel => ProtocolType::Sixel,
        ImagesMode::Kitty => ProtocolType::Kitty,
        ImagesMode::Iterm2 => ProtocolType::Iterm2,
        ImagesMode::Auto | ImagesMode::Off if in_zellij && detected == ProtocolType::Sixel => ProtocolType::Halfblocks,
        ImagesMode::Auto | ImagesMode::Off => detected,
    }
}

/// Called by workers after each result, to wake the UI's main loop.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub struct Images {
    picker: Option<Picker>,
    slots: HashMap<String, Slot>,
    queue: Arc<Queue>,
    rx: Receiver<Done>,
    encode: Option<Sender<EncodeJob>>,
    frame: Vec<(String, Kind, Option<Size>)>,
    tick: u64,
    bytes: usize,
    /// When the animation drawn this frame shows its next frame (as of the last finished
    /// frame, and in the one being drawn).
    next_frame: Option<Instant>,
    drawing_next: Option<Instant>,
    /// Reading offline: only images already on disk (cached thumbnails, `file://` paths)
    /// load; nothing is fetched.
    pub offline: bool,
    disk: Option<Arc<DiskCache>>,
}

impl Images {
    /// `None` disables images entirely: no workers, no requests.
    pub fn new(picker: Option<Picker>, wake: Waker, disk: Option<DiskCache>) -> Self {
        Self::start(picker, wake, disk, WORKERS)
    }

    fn start(picker: Option<Picker>, wake: Waker, disk: Option<DiskCache>, workers: usize) -> Self {
        let queue = Arc::new(Queue::default());
        let (tx, rx) = channel();
        let disk = disk.map(Arc::new);
        let mut encode = None;
        if let Some(p) = &picker {
            let (enc_tx, enc_rx) = channel::<EncodeJob>();
            for _ in 0..workers {
                let (q, tx, enc_tx, wake, disk) = (queue.clone(), tx.clone(), enc_tx.clone(), wake.clone(), disk.clone());
                std::thread::spawn(move || worker(&q, &tx, &enc_tx, &*wake, disk.as_deref()));
            }
            // Last, as it takes `tx` and `wake` themselves.
            let p = p.clone();
            std::thread::spawn(move || encoder(&p, &enc_rx, &tx, &*wake));
            encode = Some(enc_tx);
        }
        let (offline, frame) = (false, Vec::new());
        Self { picker, slots: HashMap::new(), queue, rx, encode, frame, tick: 0, bytes: 0, next_frame: None, drawing_next: None, offline, disk }
    }

    /// Images "on", but nothing is ever fetched or encoded: everything stays a placeholder.
    #[cfg(test)]
    pub fn offline() -> Self {
        let (_tx, rx) = channel();
        let picker = Some(Picker::halfblocks());
        let queue = Arc::new(Queue::default());
        let (offline, disk) = (false, None);
        Self { picker, slots: HashMap::new(), queue, rx, encode: None, frame: Vec::new(), tick: 0, bytes: 0, next_frame: None, drawing_next: None, offline, disk }
    }

    /// An encoder thread but no fetch workers (tests and benchmarks).
    #[cfg(test)]
    pub fn with_picker(picker: Picker) -> Self {
        Self::start(Some(picker), Arc::new(|| {}), None, 0)
    }

    /// Put an already decoded image in the cache (tests and benchmarks).
    #[cfg(test)]
    pub fn insert_decoded(&mut self, url: &str, img: DynamicImage) {
        let bytes = img.as_bytes().len().saturating_mul(2);
        let slot = Slot::Ready { img: Arc::new(img), protos: Vec::new(), pending: None, asked: None, failed: None, bytes, used: 0, frames: None, animation: None, animating: None };
        self.slots.insert(url.to_string(), slot);
    }

    /// Put an animation's decoded frames in the cache (tests).
    #[cfg(test)]
    pub fn insert_frames(&mut self, url: &str, frames: Vec<(DynamicImage, Duration)>) {
        let Some((first, _)) = frames.first() else { return };
        self.insert_decoded(url, first.clone());
        if let Some(Slot::Ready { frames: f, .. }) = self.slots.get_mut(url) {
            *f = Some(Arc::new(frames));
        }
    }

    pub fn enabled(&self) -> bool {
        self.picker.is_some()
    }

    /// A loaded image's size in pixels.
    pub fn dims(&self, url: &str) -> Option<(u32, u32)> {
        match self.slots.get(url)? {
            Slot::Ready { img, .. } => Some((img.width(), img.height())),
            _ => None,
        }
    }

    /// The terminal's cell size in pixels, as images are encoded for.
    pub fn cell_size(&self) -> Option<(u32, u32)> {
        self.picker.as_ref().map(|p| (u32::from(p.font_size().width), u32::from(p.font_size().height)))
    }

    pub fn protocol_name(&self) -> String {
        self.picker.as_ref().map_or_else(|| "off".into(), |p| format!("{:?}", p.protocol_type()).to_lowercase())
    }

    #[cfg(test)]
    pub fn queued(&self) -> usize {
        lock(&self.queue.state).jobs.len()
    }

    /// What's been asked for and not fetched yet (tests).
    #[cfg(test)]
    pub fn queued_urls(&self) -> Vec<String> {
        lock(&self.queue.state).jobs.iter().map(|(u, ..)| u.clone()).collect()
    }

    /// The image at `url` fitted into `size` cells, starting a fetch or an encoding if needed.
    /// While a new size is encoded, a previous encoding is returned if it still fits.
    pub fn get(&mut self, url: &str, size: Size, kind: Kind) -> State<'_> {
        self.get_crop(url, size, kind, Crop::FIT)
    }

    /// `get`, for part of the image (zoomed in). Animations play only fitted.
    pub fn get_crop(&mut self, url: &str, size: Size, kind: Kind, crop: Crop) -> State<'_> {
        let view = (size, crop);
        if self.picker.is_none() {
            return State::Failed;
        }
        if !self.may_load(url) {
            return State::Unavailable;
        }
        self.frame.push((url.to_string(), kind, Some(size)));
        self.tick = self.tick.saturating_add(1);
        // Looked at before borrowing it to change (stable's borrow checker needs the order).
        match self.slots.get(url) {
            Some(Slot::Ready { .. }) => {}
            Some(Slot::Failed) => return State::Failed,
            _ => return State::Loading,
        }
        let Some(Slot::Ready { img, protos, pending, asked, failed, used, frames, animation, animating, .. }) = self.slots.get_mut(url) else {
            return State::Loading;
        };
        *used = self.tick;
        // Animated: the frame due now, once the frames are encoded for this size.
        if let Some(f) = frames.as_ref().filter(|_| kind == Kind::Full && crop.is_fit()) {
            match animation {
                Some(a) if a.size == size => {
                    let now = Instant::now();
                    let (i, left) = a.frame(now);
                    if a.paused.is_none()
                        && let Some(next) = now.checked_add(left.max(MIN_FRAME))
                    {
                        self.drawing_next = Some(self.drawing_next.map_or(next, |t| t.min(next)));
                    }
                    return a.frames.get(i).map_or(State::Loading, |(p, _)| State::Ready(p));
                }
                _ if *animating != Some(size) => {
                    if let Some(enc) = &self.encode {
                        let _ = enc.send(EncodeJob::Frames(url.to_string(), size, f.clone()));
                        *animating = Some(size);
                    }
                }
                _ => {}
            }
        }
        if let Some(i) = protos.iter().position(|(v, _)| *v == view) {
            return protos.get(i).map_or(State::Loading, |(_, p)| State::Ready(p));
        }
        if *failed == Some(view) {
            // Shown as it is at another size if one fits, else it failed.
            return protos
                .iter()
                .rev()
                .find(|(_, p)| p.size().width <= size.width && p.size().height <= size.height)
                .map_or(State::Failed, |(_, p)| State::Ready(p));
        }
        if *pending != Some(view) {
            // The first encoding starts at once, and so does a zoom or a move (a key, once);
            // a resize waits until the size settles.
            let now = Instant::now();
            let new_part = protos.iter().all(|((s, _), _)| *s == size);
            let settled = match asked {
                Some((v, since)) if *v == view => now.duration_since(*since) >= RESIZE_DEBOUNCE,
                _ => {
                    *asked = Some((view, now));
                    false
                }
            };
            if (protos.is_empty() || settled || new_part)
                && let Some(enc) = &self.encode
            {
                let _ = enc.send(EncodeJob::One(url.to_string(), view, img.clone()));
                *pending = Some(view);
            }
        }
        match protos.iter().rev().find(|(_, p)| p.size().width <= size.width && p.size().height <= size.height) {
            Some((_, p)) => State::Ready(p),
            None => State::Rendering,
        }
    }

    /// When the animation on screen next changes, if one is playing.
    pub fn next_frame(&self) -> Option<Instant> {
        self.next_frame
    }

    /// Pause or resume an animated image. Returns false if it isn't one (yet).
    pub fn toggle_pause(&mut self, url: &str) -> bool {
        let Some(Slot::Ready { animation: Some(a), .. }) = self.slots.get_mut(url) else { return false };
        let now = Instant::now();
        match a.paused.take() {
            Some(at) => a.start = now.checked_sub(at).unwrap_or(now),
            None => a.paused = Some(a.at(now)),
        }
        true
    }

    pub fn is_paused(&self, url: &str) -> bool {
        matches!(self.slots.get(url), Some(Slot::Ready { animation: Some(a), .. }) if a.paused.is_some())
    }

    /// Ask for an image without drawing it (prefetch for rows about to scroll into view).
    pub fn want(&mut self, url: &str, kind: Kind) {
        if self.picker.is_some() && self.may_load(url) {
            self.frame.push((url.to_string(), kind, None));
        }
    }

    /// Whether an image may be loaded: always, unless offline, where only what's in memory
    /// or on disk is.
    fn may_load(&self, url: &str) -> bool {
        !self.offline || self.slots.contains_key(url) || url.starts_with("file://") || self.disk.as_ref().is_some_and(|d| d.contains(url))
    }

    /// Call once per frame after drawing: hand this frame's wishes to the workers.
    pub fn end_frame(&mut self) {
        if self.picker.is_none() {
            return;
        }
        let frame = std::mem::take(&mut self.frame);
        // Only an animation drawn this frame keeps the loop waking.
        self.next_frame = self.drawing_next.take();
        let mut st = lock(&self.queue.state);
        st.wanted = frame.iter().map(|(u, ..)| u.clone()).collect();
        for (url, kind, size) in frame {
            if !self.slots.contains_key(&url) {
                self.slots.insert(url.clone(), Slot::Loading);
                st.jobs.push_back((url, kind, size));
            }
        }
        drop(st);
        self.queue.cv.notify_all();
    }

    /// Collect finished fetches and encodings.
    pub fn poll(&mut self) {
        while let Ok(done) = self.rx.try_recv() {
            match done {
                // Forget it so it can be asked for again.
                Done::Skipped(url) => {
                    self.slots.remove(&url);
                }
                Done::Fetched(url, Err(_), _) => {
                    self.slots.insert(url, Slot::Failed);
                }
                Done::Fetched(url, Ok((img, frames)), pending) => {
                    let frame_bytes: usize = frames.iter().flat_map(|f| f.iter()).map(|(f, _)| f.as_bytes().len()).sum();
                    let bytes = img.as_bytes().len().saturating_mul(2).saturating_add(frame_bytes);
                    self.bytes = self.bytes.saturating_add(bytes);
                    self.tick = self.tick.saturating_add(1);
                    let slot = Slot::Ready { img, protos: Vec::new(), pending, asked: None, failed: None, bytes, used: self.tick, frames, animation: None, animating: None };
                    self.slots.insert(url, slot);
                    self.evict();
                }
                Done::Encoded(url, view, res) => {
                    let Some(Slot::Ready { protos, pending, failed, .. }) = self.slots.get_mut(&url) else { continue };
                    if *pending == Some(view) {
                        *pending = None;
                    }
                    match res {
                        Ok(p) => {
                            // Keep only a couple of views per image.
                            if protos.len() >= 2 {
                                protos.remove(0);
                            }
                            protos.push((view, p));
                        }
                        // Only this view failed: the image and its other views stay (and stay
                        // counted), and it isn't encoded again and again.
                        Err(_) => *failed = Some(view),
                    }
                }
                Done::EncodedFrames(url, size, res) => {
                    let Some(Slot::Ready { animation, animating, .. }) = self.slots.get_mut(&url) else { continue };
                    if *animating == Some(size) {
                        *animating = None;
                    }
                    // If the frames can't be encoded, the first frame stays up, still.
                    if let Some(frames) = res.ok().filter(|f| !f.is_empty()) {
                        let total = frames.iter().map(|(_, d)| *d).sum();
                        *animation = Some(Animation { size, frames, total, start: Instant::now(), paused: None });
                    }
                }
            }
        }
    }

    /// Drop the least recently used images while over budget, but never the newest one:
    /// dropping an image just loaded would only have it fetched again, and again.
    fn evict(&mut self) {
        while self.bytes > BUDGET_BYTES {
            let ready: Vec<(u64, &String)> =
                self.slots.iter().filter_map(|(k, s)| if let Slot::Ready { used, .. } = s { Some((*used, k)) } else { None }).collect();
            let oldest = ready.iter().min().filter(|_| ready.len() > 1).map(|(_, k)| (*k).clone());
            let Some(k) = oldest else { break };
            if let Some(Slot::Ready { bytes, .. }) = self.slots.remove(&k) {
                self.bytes = self.bytes.saturating_sub(bytes);
            }
        }
    }
}

fn worker(q: &Queue, tx: &Sender<Done>, encode: &Sender<EncodeJob>, wake: &dyn Fn(), disk: Option<&DiskCache>) {
    loop {
        let (url, kind, size, host) = {
            let mut st = lock(&q.state);
            loop {
                // Drop what the UI no longer wants.
                while let Some(i) = st.jobs.iter().position(|(u, ..)| !st.wanted.contains(u)) {
                    if let Some((url, ..)) = st.jobs.remove(i)
                        && tx.send(Done::Skipped(url)).is_err()
                    {
                        return;
                    }
                }
                if let Some(i) = st.jobs.iter().position(|(u, k, _)| runnable(&st.busy, u, *k, disk))
                    && let Some((url, kind, size)) = st.jobs.remove(i)
                {
                    let cached = url.starts_with("file://") || (kind == Kind::Thumb && disk.is_some_and(|d| d.contains(&url)));
                    let host = (!cached && !http::is_media_host(&url)).then(|| http::host(&url).to_string());
                    if let Some(h) = &host {
                        st.busy.insert(h.clone());
                    }
                    break (url, kind, size, host);
                }
                st = q.cv.wait(st).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        let res = crate::guard::catching(|| fetch(&url, kind, if kind == Kind::Thumb { disk } else { None }))
            .unwrap_or_else(bug)
            .map(|(img, frames)| (Arc::new(img), frames));
        if let Some(h) = host {
            lock(&q.state).busy.remove(&h);
            q.cv.notify_all();
        }
        // Encode right away for the size the UI asked for, saving a round trip.
        let pending = match (&res, size) {
            (Ok((img, _)), Some(size)) => encode.send(EncodeJob::One(url.clone(), (size, Crop::FIT), img.clone())).is_ok().then_some((size, Crop::FIT)),
            _ => None,
        };
        if tx.send(Done::Fetched(url, res, pending)).is_err() {
            return;
        }
        wake();
    }
}

/// A panic in a job, as the job's failure.
#[allow(clippy::needless_pass_by_value)] // what guard::catching gives unwrap_or_else
fn bug<T>(what: String) -> Result<T, String> {
    Err(format!("ck hit a bug: {what}"))
}

fn encoder(picker: &Picker, jobs: &Receiver<EncodeJob>, tx: &Sender<Done>, wake: &dyn Fn()) {
    while let Ok(job) = jobs.recv() {
        let done = match job {
            EncodeJob::One(url, (size, crop), img) => {
                let res = crate::guard::catching(|| encode_crop(picker, &img, size, crop)).unwrap_or_else(bug);
                Done::Encoded(url, (size, crop), res)
            }
            EncodeJob::Frames(url, size, frames) => {
                let res = crate::guard::catching(|| frames.iter().map(|(f, d)| encode(picker, f, size).map(|p| (p, *d))).collect()).unwrap_or_else(bug);
                Done::EncodedFrames(url, size, res)
            }
        };
        if tx.send(done).is_err() {
            return;
        }
        wake();
    }
}

/// Encode the part `crop` of `img` to fit in `size` cells: scaled up to fill them (that's
/// zooming in), unless it's the whole image.
pub(crate) fn encode_crop(picker: &Picker, img: &DynamicImage, size: Size, crop: Crop) -> Result<Protocol, String> {
    if crop.is_fit() {
        return encode(picker, img, size);
    }
    let font = picker.font_size();
    let (w, h) = pixels(size, font);
    let (x, y, cw, ch) = crop.region(img.width(), img.height(), (w, h));
    let part = img.crop_imm(x, y, cw, ch);
    if picker.protocol_type() == ProtocolType::Halfblocks {
        return halfblocks(&part, size, font, true);
    }
    picker.new_protocol(shrink(&part, w, h, true), size, Resize::Fit(None)).map_err(|e| e.to_string())
}

/// Encode `img` to fit in `size` cells. Large images are scaled down from the shared copy
/// first, so the full-size image is never cloned.
fn encode(picker: &Picker, img: &DynamicImage, size: Size) -> Result<Protocol, String> {
    if picker.protocol_type() == ProtocolType::Halfblocks {
        return halfblocks(img, size, picker.font_size(), false);
    }
    let (w, h) = pixels(size, picker.font_size());
    picker.new_protocol(shrink(img, w, h, false), size, Resize::Fit(None)).map_err(|e| e.to_string())
}

/// `size` cells in pixels, at the terminal's cell size `font`.
fn pixels(size: Size, font: ratatui_image::FontSize) -> (u32, u32) {
    (u32::from(size.width).saturating_mul(u32::from(font.width)), u32::from(size.height).saturating_mul(u32::from(font.height)))
}

/// `img` fitted into `w` x `h` pixels (keeping its shape): scaled down with the fast
/// area-averaging resize when it's much bigger (a high-quality filter over a large image costs
/// tens of milliseconds, and the terminal shows no difference), and only scaled up with
/// `grow` (a zoomed part fills the area).
fn shrink(img: &DynamicImage, w: u32, h: u32, grow: bool) -> DynamicImage {
    let (w, h) = (w.max(1), h.max(1));
    if img.width() > w || img.height() > h {
        img.thumbnail(w, h)
    } else if grow {
        img.resize(w, h, FilterType::Triangle)
    } else {
        img.clone()
    }
}

/// Half-blocks (two "pixels" per cell, one above the other) for `img` in `size` cells. The
/// image takes the cells it would at the terminal's cell size (`font`): as many as its pixels
/// cover, fewer if they don't fit, or (with `fill`) all it can. It's scaled once, straight to
/// two pixels per cell, rather than to the screen's full pixel size first (which, at a large
/// cell size, made the viewer cost a tenth of a second per image).
fn halfblocks(img: &DynamicImage, size: Size, font: ratatui_image::FontSize, fill: bool) -> Result<Protocol, String> {
    use ratatui_image::protocol::halfblocks::Halfblocks;
    let (fw, fh) = (u32::from(font.width).max(1), u32::from(font.height).max(1));
    let (area_w, area_h) = (u32::from(size.width).saturating_mul(fw), u32::from(size.height).saturating_mul(fh));
    let (iw, ih) = (img.width().max(1), img.height().max(1));
    // Its size on screen, in pixels: as it is if it fits (and isn't to fill), else fitted.
    #[allow(clippy::cast_possible_truncation)] // f64 as u32 saturates
    let (pw, ph) = if !fill && iw <= area_w && ih <= area_h {
        (iw, ih)
    } else {
        let ratio = f64::min(f64::from(area_w) / f64::from(iw), f64::from(area_h) / f64::from(ih));
        (((f64::from(iw) * ratio).round() as u32).clamp(1, area_w.max(1)), ((f64::from(ih) * ratio).round() as u32).clamp(1, area_h.max(1)))
    };
    let cells_in = |px: u32, f: u32| u16::try_from(px.div_ceil(f).max(1)).unwrap_or(u16::MAX);
    let cells = Size::new(cells_in(pw, fw), cells_in(ph, fh));
    // The image in half-block pixels: a cell's width, half its height.
    // (Rounded: pw / fw wide, ph * 2 / fh tall.)
    let cw = pw.saturating_mul(2).saturating_add(fw).checked_div(fw.saturating_mul(2)).unwrap_or(0).clamp(1, u32::from(cells.width));
    let ch = ph.saturating_mul(4).saturating_add(fh).checked_div(fh.saturating_mul(2)).unwrap_or(0).clamp(1, u32::from(cells.height) * 2);
    let scaled = if iw > cw.saturating_mul(2) || ih > ch.saturating_mul(2) { img.thumbnail_exact(cw, ch) } else { img.resize_exact(cw, ch, FilterType::Triangle) };
    // Padded to whole cells like the library pads (transparent, shown black).
    let (gw, gh) = (u32::from(cells.width), u32::from(cells.height) * 2);
    let image = if (cw, ch) == (gw, gh) {
        scaled
    } else {
        let mut bg = DynamicImage::ImageRgba8(image::RgbaImage::new(gw, gh));
        image::imageops::overlay(&mut bg, &scaled, 0, 0);
        bg
    };
    Halfblocks::new(image, cells).map(Protocol::Halfblocks).map_err(|e| e.to_string())
}

/// Whether a job can start now: cached thumbnails and media hosts always; hosts that share
/// the API's rate limit one at a time, so the queue's order (top to bottom) is kept.
fn runnable(busy: &HashSet<String>, url: &str, kind: Kind, disk: Option<&DiskCache>) -> bool {
    url.starts_with("file://")
        || (kind == Kind::Thumb && disk.is_some_and(|d| d.contains(url)))
        || http::is_media_host(url)
        || !busy.contains(http::host(url))
}

/// Load an image: a thumbnail from the disk cache if it's there, else over HTTP (caching
/// thumbnails that decode). A cached file that won't decode is deleted and fetched again.
/// Full-size animated GIFs come with their frames.
fn fetch(url: &str, kind: Kind, disk: Option<&DiskCache>) -> Result<(DynamicImage, Option<Frames>), String> {
    // A downloaded file (a saved thread's, read offline).
    if let Some(path) = url.strip_prefix("file://") {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let frames = if kind == Kind::Full { gif_frames(&bytes) } else { None };
        return Ok((decode_within(&bytes, kind.alloc())?, frames));
    }
    if let Some(d) = disk
        && let Some(bytes) = d.get(url)
    {
        match decode_within(&bytes, kind.alloc()) {
            Ok(img) => return Ok((img, None)),
            Err(_) => d.remove(url),
        }
    }
    let bytes = http::get_bytes(url, MAX_DOWNLOAD).map_err(|e| format!("{e:#}"))?;
    let img = decode_within(&bytes, kind.alloc())?;
    if let Some(d) = disk {
        let _ = d.put(url, &bytes);
    }
    let frames = if kind == Kind::Full { gif_frames(&bytes) } else { None };
    Ok((img, frames))
}

/// An animated GIF's frames with their delays, if it has more than one. Frames are scaled
/// down as far as needed (up to 4x) to fit in `ANIMATION_BYTES`; past that it's shown still.
fn gif_frames(bytes: &[u8]) -> Option<Frames> {
    gif_frames_within(bytes, ANIMATION_BYTES)
}

pub(crate) fn gif_frames_within(bytes: &[u8], budget: usize) -> Option<Frames> {
    use image::AnimationDecoder;
    if !bytes.starts_with(b"GIF8") {
        return None;
    }
    // The canvas's width and height, after the 6-byte signature.
    let Some(&[w0, w1, h0, h1]) = bytes.get(6..10) else { return None };
    // Frames, from their graphic control blocks (a slight overcount at worst), and the
    // canvas size from the header: what the frames will take decoded.
    let count = bytes.windows(3).filter(|w| *w == [0x21, 0xf9, 0x04]).count().max(1);
    let (w, h) = (f64::from(u16::from_le_bytes([w0, w1])), f64::from(u16::from_le_bytes([h0, h1])));
    let need = count as f64 * w * h * 4.0;
    let scale = (budget as f64 / need).sqrt().min(1.0);
    if scale < 0.25 {
        return None;
    }
    // The header's size says nothing about the frames': a later one can claim 65535x65535,
    // which unlimited would be allocated (and abort) before anything checks it.
    let mut decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    image::ImageDecoder::set_limits(&mut decoder, limits(GIF_FRAME_ALLOC)).ok()?;
    let mut frames = Vec::new();
    let mut size = 0usize;
    for frame in decoder.into_frames() {
        let frame = frame.ok()?;
        let (num, den) = frame.delay().numer_denom_ms();
        // Like browsers: no delay (or almost none) means 100ms.
        let ms = num.checked_div(den).unwrap_or(0);
        let delay = Duration::from_millis(if ms < 20 { 100 } else { ms as u64 });
        let mut img = DynamicImage::ImageRgba8(frame.into_buffer());
        if scale < 1.0 {
            #[allow(clippy::cast_possible_truncation)] // f64 as u32 saturates
            let (fw, fh) = ((f64::from(img.width()) * scale) as u32, (f64::from(img.height()) * scale) as u32);
            img = img.resize_exact(fw.max(1), fh.max(1), FilterType::Triangle);
        }
        size = size.saturating_add(img.as_bytes().len());
        if size > budget {
            return None;
        }
        frames.push((img, delay));
    }
    (frames.len() > 1).then(|| Arc::new(frames))
}

/// Decoding limits: no side over `MAX_SIDE`, and at most `alloc` bytes allocated at once.
fn limits(alloc: u64) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(alloc);
    limits
}

/// Decode a full-size image (within `FULL_ALLOC`).
pub(crate) fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    decode_within(bytes, FULL_ALLOC)
}

/// Decode an image, refusing one that would take more than `alloc` bytes (a small file can
/// claim a huge image) or is over `MAX_SIDE` either way. Big ones are scaled to `MAX_DIM`.
fn decode_within(bytes: &[u8], alloc: u64) -> Result<DynamicImage, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    reader.limits(limits(alloc));
    let img = reader.decode().map_err(|e| e.to_string())?;
    Ok(if img.width() > MAX_DIM || img.height() > MAX_DIM { img.thumbnail(MAX_DIM, MAX_DIM) } else { img })
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn protocols_forced_or_detected_and_zellij_isnt_believed_about_sixel() {
        use crate::config::ImagesMode;
        assert_eq!(choose_protocol(ImagesMode::Auto, ProtocolType::Sixel, false), ProtocolType::Sixel);
        assert_eq!(choose_protocol(ImagesMode::Auto, ProtocolType::Sixel, true), ProtocolType::Halfblocks);
        assert_eq!(choose_protocol(ImagesMode::Auto, ProtocolType::Kitty, true), ProtocolType::Kitty);
        assert_eq!(choose_protocol(ImagesMode::Sixel, ProtocolType::Halfblocks, true), ProtocolType::Sixel);
        assert_eq!(choose_protocol(ImagesMode::Halfblocks, ProtocolType::Kitty, false), ProtocolType::Halfblocks);
        // Each is a setting the config takes.
        for (text, mode) in [("halfblocks", ImagesMode::Halfblocks), ("sixel", ImagesMode::Sixel), ("kitty", ImagesMode::Kitty), ("iterm2", ImagesMode::Iterm2)] {
            let cfg: crate::config::Config = toml::from_str(&format!("images = \"{text}\"\n{}", crate::config::DEFAULT_CONFIG.replace("images = \"auto\"", ""))).unwrap();
            assert_eq!((cfg.images, mode.as_str()), (mode, text));
        }
    }

    /// How the library alone would size an image (the old way): fitted at the cell size.
    fn library_size(picker: &Picker, img: &DynamicImage, size: Size, fill: bool) -> Size {
        let font = picker.font_size();
        let (w, h) = (size.width as u32 * font.width as u32, size.height as u32 * font.height as u32);
        let img = if fill || img.width() > w || img.height() > h { img.resize(w, h, FilterType::Triangle) } else { img.clone() };
        picker.new_protocol(img, size, Resize::Fit(None)).unwrap().size()
    }

    #[test]
    fn halfblocks_take_the_cells_the_library_would_give_them() {
        let zoom = Crop::FIT.zoomed(true).zoomed(true);
        for font in [(10u16, 20u16), (25, 51), (8, 17), (1, 2)] {
            #[allow(deprecated)]
            let mut picker = Picker::from_fontsize(font.into());
            picker.set_protocol_type(ProtocolType::Halfblocks);
            for (iw, ih) in [(1200, 1200), (1200, 350), (300, 1300), (250, 250), (40, 30), (1, 1), (2500, 3)] {
                let img = DynamicImage::new_rgb8(iw, ih);
                for size in [Size::new(117, 30), Size::new(16, 8), Size::new(3, 1)] {
                    let ours = encode_crop(&picker, &img, size, Crop::FIT).unwrap().size();
                    assert_eq!(ours, library_size(&picker, &img, size, false), "font {font:?}, image {iw}x{ih}, area {size:?}");
                }
                // Zoomed: the part fills the area as it did (at small cell sizes: the
                // library's own way is slow in a debug build at large ones).
                if font.0 > 10 {
                    continue;
                }
                let size = Size::new(40, 12);
                let font_px = picker.font_size();
                let view = (u32::from(size.width) * u32::from(font_px.width), u32::from(size.height) * u32::from(font_px.height));
                let (x, y, cw, ch) = zoom.region(iw, ih, view);
                let part = img.crop_imm(x, y, cw, ch);
                let ours = encode_crop(&picker, &img, size, zoom).unwrap().size();
                assert_eq!(ours, library_size(&picker, &part, size, true), "zoomed: font {font:?}, image {iw}x{ih}");
            }
        }
    }

    #[test]
    fn a_zoomed_image_fills_a_wide_viewer() {
        // A square image on a wide viewer: fitted, a square in the middle; zoomed, the whole
        // width (the report: zooming cut it off at the sides).
        let img = DynamicImage::new_rgb8(1200, 1200);
        for proto in [ProtocolType::Halfblocks, ProtocolType::Kitty] {
            #[allow(deprecated)]
            let mut picker = Picker::from_fontsize((10, 20).into());
            picker.set_protocol_type(proto);
            let size = Size::new(117, 30);
            let fitted = encode_crop(&picker, &img, size, Crop::FIT).unwrap().size();
            assert_eq!(fitted.width, 60, "{proto:?}");
            let zoomed = encode_crop(&picker, &img, size, Crop::FIT.zoomed(true).zoomed(true)).unwrap().size();
            assert!(zoomed.width >= 115 && zoomed.height == 30, "{proto:?}: {zoomed:?}");
        }
    }

    #[test]
    fn halfblocks_show_the_image() {
        // Left half red, right half blue, top white: on screen as such.
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(800, 400, |x, y| {
            if y < 100 { image::Rgb([255, 255, 255]) } else if x < 400 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) }
        }));
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize((25, 51).into());
        picker.set_protocol_type(ProtocolType::Halfblocks);
        let p = encode_crop(&picker, &img, Size::new(20, 10), Crop::FIT).unwrap();
        let s = p.size();
        let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, s.width, s.height));
        ratatui::widgets::Widget::render(ratatui_image::Image::new(&p), buf.area, &mut buf);
        let cell = |x: u16, y: u16| buf[(x, y)].clone();
        let colors = |c: &ratatui::buffer::Cell| [c.fg, c.bg];
        assert!(colors(&cell(1, s.height - 1)).contains(&ratatui::style::Color::Rgb(255, 0, 0)), "{:?}", cell(1, s.height - 1));
        assert!(colors(&cell(s.width - 2, s.height - 1)).contains(&ratatui::style::Color::Rgb(0, 0, 255)));
        assert!(colors(&cell(s.width / 2 - 3, 0)).contains(&ratatui::style::Color::Rgb(255, 255, 255)));
    }

    #[test]
    fn off_makes_no_requests() {
        let mut im = Images::new(None, Arc::new(|| {}), None);
        assert!(matches!(im.get("http://x/a.jpg", Size::new(4, 4), Kind::Thumb), State::Failed));
        im.end_frame();
        assert!(lock(&im.queue.state).jobs.is_empty());
    }

    /// Poll until `get` stops saying the image is being rendered (up to 10s: a debug build
    /// on a busy machine is slow).
    fn settle(im: &mut Images, url: &str, size: Size) -> bool {
        for _ in 0..1000 {
            im.poll();
            if !matches!(im.get(url, size, Kind::Full), State::Rendering) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn encoding_happens_off_the_ui_thread() {
        let mut im = Images::with_picker(Picker::halfblocks());
        im.insert_decoded("u", DynamicImage::new_rgb8(2048, 1536));
        // The first ask only queues the encoding.
        let start = Instant::now();
        assert!(matches!(im.get("u", Size::new(100, 30), Kind::Full), State::Rendering));
        assert!(start.elapsed() < Duration::from_millis(5), "{:?}", start.elapsed());
        assert!(settle(&mut im, "u", Size::new(100, 30)));
        let State::Ready(p) = im.get("u", Size::new(100, 30), Kind::Full) else { panic!("not ready") };
        assert!(p.size().width <= 100 && p.size().height <= 30);

        // A resize to a bigger area keeps showing the current encoding, and only encodes
        // the new size once it has held for the debounce time.
        assert!(matches!(im.get("u", Size::new(110, 32), Kind::Full), State::Ready(_)));
        let Some(Slot::Ready { pending, .. }) = im.slots.get("u") else { panic!() };
        assert_eq!(*pending, None);
        std::thread::sleep(RESIZE_DEBOUNCE);
        im.get("u", Size::new(110, 32), Kind::Full);
        let Some(Slot::Ready { pending, .. }) = im.slots.get("u") else { panic!() };
        assert_eq!(*pending, Some((Size::new(110, 32), Crop::FIT)));
    }

    #[test]
    fn zooming_crops_and_moves_inside_the_image() {
        let c = Crop::FIT.zoomed(true);
        assert_eq!((c.zoom, c.x, c.y), (150, 500, 500));
        // 200% of an image the same shape as the screen: half of each side, centered.
        let c = c.zoomed(true);
        assert_eq!(c.region(1000, 600, (2000, 1200)), (250, 150, 500, 300));
        // A square image on a wide screen: zoomed in, the part shown is the screen's shape
        // (the 100% view is 600x600 of 1200x600; at 200% the screen holds 600x300 of the
        // image), not a square in the middle.
        let wide = (1200, 600);
        assert_eq!(c.region(1000, 1000, wide), (0, 250, 1000, 500));
        let c4 = c.zoomed(true).zoomed(true);
        assert_eq!(c4.zoom, 400);
        assert_eq!(c4.region(1000, 1000, wide), (250, 375, 500, 250));
        assert_eq!(c4.shown(1000, 1000, wide), (500, 250));
        // Moving steps a quarter of what's shown and stops at the edges, with no presses
        // wasted coming back.
        let shown = c4.shown(1000, 1000, wide);
        let left = (0..10).fold(c4, |c, _| c.moved(-1, 0, shown));
        assert_eq!(left.region(1000, 1000, wide).0, 0);
        assert_eq!(left.moved(1, 0, shown).region(1000, 1000, wide).0, 125);
        let down = (0..10).fold(c4, |c, _| c.moved(0, 1, shown));
        assert_eq!(down.region(1000, 1000, wide).1, 750);
        // Zooming out goes back to the whole image; past the last level it stays.
        assert_eq!(c.zoomed(false).zoomed(false), Crop::FIT);
        assert_eq!(Crop::FIT.zoomed(false), Crop::FIT);
        let most = (0..20).fold(Crop::FIT, |c, _| c.zoomed(true));
        assert_eq!(most.zoom, 800);
        assert!(most.region(7, 3, (100, 100)).2 >= 1 && most.region(7, 3, (100, 100)).3 >= 1);
    }

    #[test]
    fn a_zoomed_view_is_encoded_at_once_and_fills_the_area() {
        let mut im = Images::with_picker(Picker::halfblocks());
        im.insert_decoded("u", DynamicImage::new_rgb8(400, 300));
        let size = Size::new(40, 20);
        assert!(settle(&mut im, "u", size));
        let fitted = match im.get("u", size, Kind::Full) {
            State::Ready(p) => p.size(),
            _ => panic!("not ready"),
        };
        // No wait for a zoom (as there is for a resize).
        let zoom = Crop::FIT.zoomed(true).zoomed(true);
        im.get_crop("u", size, Kind::Full, zoom);
        let Some(Slot::Ready { pending, .. }) = im.slots.get("u") else { panic!() };
        assert_eq!(*pending, Some((size, zoom)));
        let mut zoomed = None;
        for _ in 0..1000 {
            im.poll();
            let done = im.slots.get("u").is_some_and(|s| matches!(s, Slot::Ready { pending: None, .. }));
            if let State::Ready(p) = im.get_crop("u", size, Kind::Full, zoom)
                && done
            {
                zoomed = Some(p.size());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let zoomed = zoomed.expect("the zoomed view wasn't encoded");
        assert!(zoomed.width <= size.width && zoomed.height <= size.height);
        assert!(zoomed.width >= fitted.width && zoomed.height >= fitted.height, "{zoomed:?} {fitted:?}");
    }

    #[test]
    fn one_fetch_at_a_time_per_rate_limited_host() {
        let dir = tempfile::tempdir().unwrap();
        let disk = DiskCache::new(dir.path().to_path_buf(), 1 << 20);
        crate::http::register_media_host("https://media.example");
        let busy: HashSet<String> = ["chan.example".to_string()].into();
        // The site's host is busy: its next thumbnail waits...
        assert!(!runnable(&busy, "https://chan.example/b/thumb/1.png", Kind::Thumb, Some(&disk)));
        // ...unless it's already on disk.
        disk.put("https://chan.example/b/thumb/2.png", b"x").unwrap();
        assert!(runnable(&busy, "https://chan.example/b/thumb/2.png", Kind::Thumb, Some(&disk)));
        // Other hosts and media hosts aren't held up.
        assert!(runnable(&busy, "https://other.example/1.png", Kind::Thumb, Some(&disk)));
        assert!(runnable(&busy, "https://media.example/1.png", Kind::Full, None));
    }

    #[test]
    fn thumbnails_come_from_disk_without_requests() {
        let dir = tempfile::tempdir().unwrap();
        let disk = DiskCache::new(dir.path().to_path_buf(), 1 << 20);
        let mut png = Vec::new();
        DynamicImage::new_rgb8(4, 3).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        // An unroutable URL: any request would fail, so success means it came from disk.
        let url = "http://127.0.0.1:9/thumb.png";
        disk.put(url, &png).unwrap();
        assert_eq!(fetch(url, Kind::Thumb, Some(&disk)).unwrap().0.width(), 4);
        // A corrupt entry is deleted (and the fetch then fails here, offline).
        disk.put(url, b"not an image").unwrap();
        assert!(fetch(url, Kind::Thumb, Some(&disk)).is_err());
        assert!(!disk.contains(url));
    }

    #[test]
    fn offline_loads_only_what_is_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let disk = DiskCache::new(dir.path().join("cache"), 1 << 20);
        let mut png = Vec::new();
        DynamicImage::new_rgb8(4, 3).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let cached = "http://127.0.0.1:9/cached.png";
        disk.put(cached, &png).unwrap();
        std::fs::write(dir.path().join("1_cat.png"), &png).unwrap();
        let local = format!("file://{}", dir.path().join("1_cat.png").display());
        let mut im = Images::start(Some(Picker::halfblocks()), Arc::new(|| {}), Some(disk), 0);
        im.offline = true;
        let size = Size::new(4, 4);
        assert!(matches!(im.get("http://127.0.0.1:9/other.png", size, Kind::Thumb), State::Unavailable));
        im.want("http://127.0.0.1:9/next.png", Kind::Thumb);
        assert!(matches!(im.get(cached, size, Kind::Thumb), State::Loading));
        assert!(matches!(im.get(&local, size, Kind::Full), State::Loading));
        im.end_frame();
        let jobs: Vec<String> = lock(&im.queue.state).jobs.iter().map(|(u, ..)| u.clone()).collect();
        assert_eq!(jobs, [cached.to_string(), local.clone()]);
        // A downloaded file loads from its path, and never waits on a host.
        assert_eq!(fetch(&local, Kind::Full, None).unwrap().0.width(), 4);
        assert!(runnable(&["".to_string()].into(), &local, Kind::Full, None));
    }

    fn gif(frames: &[(u8, u16)]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
            for &(shade, ms) in frames {
                let buf = image::RgbaImage::from_pixel(8, 6, image::Rgba([shade, shade, shade, 255]));
                let delay = image::Delay::from_numer_denom_ms(ms as u32, 1);
                enc.encode_frame(image::Frame::from_parts(buf, 0, 0, delay)).unwrap();
            }
        }
        out
    }

    /// A GIF whose header says 1x1, with two 1x1 frames and a third that claims 65535x65535.
    fn gif_bomb() -> Vec<u8> {
        let frame = |w: u16, h: u16| {
            let mut v = vec![0x21, 0xf9, 0x04, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x2c, 0, 0, 0, 0];
            v.extend(w.to_le_bytes());
            v.extend(h.to_le_bytes());
            v.extend([0x00, 0x02, 0x02, 0x44, 0x01, 0x00]);
            v
        };
        let mut g = b"GIF89a".to_vec();
        g.extend([1, 0, 1, 0, 0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
        g.extend(frame(1, 1));
        g.extend(frame(1, 1));
        g.extend(frame(65535, 65535));
        g.push(0x3b);
        g
    }

    #[test]
    fn images_too_big_to_decode_are_refused() {
        let png = |w, h| {
            let mut out = std::io::Cursor::new(Vec::new());
            DynamicImage::new_luma8(w, h).write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        assert!(decode(&png(100, 100)).is_ok());
        // Wider than any real image.
        assert!(decode(&png(MAX_SIDE + 1, 1)).is_err());
        // More than it may take: 10000 bytes, with 1000 allowed.
        assert!(decode_within(&png(100, 100), 1000).is_err());
        assert!(Kind::Thumb.alloc() < Kind::Full.alloc());
    }

    #[test]
    fn a_frame_bigger_than_its_gif_isnt_allocated() {
        let bomb = gif_bomb();
        assert!(decode(&bomb).is_ok(), "the first frame is fine");
        assert!(gif_frames(&bomb).is_none());
    }

    #[test]
    fn gif_frames_and_delays() {
        let frames = gif_frames(&gif(&[(0, 50), (128, 0), (255, 200)])).unwrap();
        let delays: Vec<u128> = frames.iter().map(|(_, d)| d.as_millis()).collect();
        // A missing delay plays as 100ms, like in browsers.
        assert_eq!(delays, [50, 100, 200]);
        assert!(gif_frames(&gif(&[(0, 50)])).is_none());
        // Too big for the budget: scaled down to fit (4 frames of 8x6 need 768 bytes).
        let bytes = gif(&[(0, 40), (60, 40), (120, 40), (180, 40)]);
        let frames = gif_frames_within(&bytes, 400).unwrap();
        assert_eq!((frames[0].0.width(), frames[0].0.height()), (5, 4));
        // More than 4x too big: still.
        assert!(gif_frames_within(&bytes, 40).is_none());
        assert!(gif_frames(b"\x89PNG").is_none());
    }

    #[test]
    fn animations_play_pause_and_wake_the_loop() {
        let mut im = Images::with_picker(Picker::halfblocks());
        let frames = gif_frames(&gif(&[(0, 50), (255, 50)])).unwrap();
        im.insert_frames("g", frames.iter().cloned().collect());
        let size = Size::new(20, 6);
        // The first frame shows still while the frames are encoded; then it plays.
        let mut playing = false;
        for _ in 0..1000 {
            im.poll();
            let _ = im.get("g", size, Kind::Full);
            im.end_frame();
            if im.next_frame().is_some() {
                playing = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(playing);
        let next = im.next_frame().unwrap();
        assert!(next <= Instant::now() + MIN_FRAME);
        // Paused: nothing to wake for.
        assert!(im.toggle_pause("g"));
        let _ = im.get("g", size, Kind::Full);
        im.end_frame();
        assert!(im.next_frame().is_none());
        // A frame without the viewer: nothing either.
        assert!(im.toggle_pause("g"));
        im.end_frame();
        assert!(im.next_frame().is_none());
        // Still images can't be paused.
        im.insert_decoded("still", DynamicImage::new_rgb8(4, 4));
        assert!(!im.toggle_pause("still"));
    }

    #[test]
    fn the_newest_image_is_never_evicted() {
        let mut im = Images::new(None, Arc::new(|| {}), None);
        let (tx, rx) = channel();
        im.rx = rx;
        // One image over the whole budget on its own: kept, not dropped and fetched again.
        let img = Arc::new(DynamicImage::new_rgba8(4096, 4096));
        tx.send(Done::Fetched("big".into(), Ok((img, None)), None)).unwrap();
        im.poll();
        assert!(matches!(im.slots.get("big"), Some(Slot::Ready { .. })));
    }

    #[test]
    fn a_size_that_fails_to_encode_fails_alone() {
        let mut im = Images::offline();
        let (tx, rx) = channel();
        im.rx = rx;
        let img = DynamicImage::new_rgb8(64, 48);
        let (big, small) = (Size::new(20, 10), Size::new(4, 2));
        let p = encode(&Picker::halfblocks(), &img, big).unwrap();
        tx.send(Done::Fetched("u".into(), Ok((Arc::new(img), None)), None)).unwrap();
        tx.send(Done::Encoded("u".into(), (big, Crop::FIT), Ok(p))).unwrap();
        tx.send(Done::Encoded("u".into(), (small, Crop::FIT), Err("no".into()))).unwrap();
        im.poll();
        // The size that was encoded still shows; the one that failed says so.
        assert!(matches!(im.get("u", big, Kind::Full), State::Ready(_)));
        assert!(matches!(im.get("u", small, Kind::Full), State::Failed));
        // Whatever is kept is what's counted.
        let kept: usize = im.slots.values().map(|s| if let Slot::Ready { bytes, .. } = s { *bytes } else { 0 }).sum();
        assert_eq!(im.bytes, kept);
        assert!(kept > 0);
    }

    #[test]
    fn eviction_respects_budget() {
        let mut im = Images::new(None, Arc::new(|| {}), None);
        let (tx, rx) = channel();
        im.rx = rx;
        // Each 2048x2048 RGBA image counts as 32 MiB (doubled for its protocol).
        for i in 0..4 {
            let img = Arc::new(DynamicImage::new_rgba8(2048, 2048));
            tx.send(Done::Fetched(format!("u{i}"), Ok((img, None)), None)).unwrap();
        }
        im.poll();
        assert!(im.bytes <= BUDGET_BYTES);
        assert!(!im.slots.contains_key("u0"), "oldest image evicted first");
        assert!(im.slots.contains_key("u3"));
    }
}

