//! Background image loading and a bounded in-memory cache of decoded images.
//!
//! The UI asks for images every frame with `get`; anything it didn't ask for in a frame is
//! dropped from the fetch queue, so scrolling past a page of thumbnails doesn't fetch them.
//! Thumbnails are also kept on disk. On hosts that share the API's rate limit, one image is
//! fetched at a time, in the order the UI asked (top to bottom).
//!
//! Turning an image into terminal output (sixel, kitty, half-blocks) takes milliseconds, so
//! it happens on an encoder thread too; `get` only ever hands out finished encodings.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use image::DynamicImage;
use image::imageops::FilterType;
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
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
        Duration::from_nanos((t.as_nanos() % self.total.as_nanos().max(1)) as u64)
    }

    /// The frame showing now, and how long until the next one.
    fn frame(&self, now: Instant) -> (usize, Duration) {
        let mut t = self.at(now);
        for (i, (_, d)) in self.frames.iter().enumerate() {
            if t < *d {
                return (i, *d - t);
            }
            t -= *d;
        }
        (0, self.frames[0].1)
    }
}

/// Thumbnails are cached on disk; full-size images only in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Thumb,
    Full,
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

enum Slot {
    Loading,
    Failed,
    Ready {
        img: Arc<DynamicImage>,
        protos: Vec<(Size, Protocol)>,
        /// The size being encoded, if any.
        pending: Option<Size>,
        /// A size asked for since, and when (for the resize debounce).
        asked: Option<(Size, Instant)>,
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
    Fetched(String, Result<(Arc<DynamicImage>, Option<Frames>), String>, Option<Size>),
    Encoded(String, Size, Result<Protocol, String>),
    EncodedFrames(String, Size, Result<Vec<(Protocol, Duration)>, String>),
}

enum EncodeJob {
    One(String, Size, Arc<DynamicImage>),
    Frames(String, Size, Frames),
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
            let (p, etx, ewake) = (p.clone(), tx.clone(), wake.clone());
            std::thread::spawn(move || encoder(&p, &enc_rx, &etx, &*ewake));
            for _ in 0..workers {
                let (q, tx, enc_tx, wake, disk) = (queue.clone(), tx.clone(), enc_tx.clone(), wake.clone(), disk.clone());
                std::thread::spawn(move || worker(&q, &tx, &enc_tx, &*wake, disk.as_deref()));
            }
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
        let bytes = img.as_bytes().len() * 2;
        let slot = Slot::Ready { img: Arc::new(img), protos: Vec::new(), pending: None, asked: None, bytes, used: 0, frames: None, animation: None, animating: None };
        self.slots.insert(url.to_string(), slot);
    }

    /// Put an animation's decoded frames in the cache (tests).
    #[cfg(test)]
    pub fn insert_frames(&mut self, url: &str, frames: Vec<(DynamicImage, Duration)>) {
        self.insert_decoded(url, frames[0].0.clone());
        if let Some(Slot::Ready { frames: f, .. }) = self.slots.get_mut(url) {
            *f = Some(Arc::new(frames));
        }
    }

    pub fn enabled(&self) -> bool {
        self.picker.is_some()
    }

    pub fn protocol_name(&self) -> String {
        self.picker.as_ref().map_or("off".into(), |p| format!("{:?}", p.protocol_type()).to_lowercase())
    }

    #[cfg(test)]
    pub fn queued(&self) -> usize {
        lock(&self.queue.state).jobs.len()
    }

    /// The image at `url` fitted into `size` cells, starting a fetch or an encoding if needed.
    /// While a new size is encoded, a previous encoding is returned if it still fits.
    pub fn get(&mut self, url: &str, size: Size, kind: Kind) -> State<'_> {
        if self.picker.is_none() {
            return State::Failed;
        }
        if !self.may_load(url) {
            return State::Unavailable;
        }
        self.frame.push((url.to_string(), kind, Some(size)));
        self.tick += 1;
        // Looked at before borrowing it to change (stable's borrow checker needs the order).
        match self.slots.get(url) {
            Some(Slot::Ready { .. }) => {}
            Some(Slot::Failed) => return State::Failed,
            _ => return State::Loading,
        }
        let Some(Slot::Ready { img, protos, pending, asked, used, frames, animation, animating, .. }) = self.slots.get_mut(url) else {
            return State::Loading;
        };
        *used = self.tick;
        // Animated: the frame due now, once the frames are encoded for this size.
        if let Some(f) = frames.as_ref().filter(|_| kind == Kind::Full) {
            match animation {
                Some(a) if a.size == size => {
                    let now = Instant::now();
                    let (i, left) = a.frame(now);
                    if a.paused.is_none() {
                        let next = now + left.max(MIN_FRAME);
                        self.drawing_next = Some(self.drawing_next.map_or(next, |t| t.min(next)));
                    }
                    return State::Ready(&a.frames[i].0);
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
        if let Some(i) = protos.iter().position(|(s, _)| *s == size) {
            return State::Ready(&protos[i].1);
        }
        if *pending != Some(size) {
            // The first encoding starts at once; a resize waits until the size settles.
            let now = Instant::now();
            let settled = match asked {
                Some((s, since)) if *s == size => now.duration_since(*since) >= RESIZE_DEBOUNCE,
                _ => {
                    *asked = Some((size, now));
                    false
                }
            };
            if (protos.is_empty() || settled)
                && let Some(enc) = &self.encode
            {
                let _ = enc.send(EncodeJob::One(url.to_string(), size, img.clone()));
                *pending = Some(size);
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
            Some(at) => a.start = now - at,
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
                    let bytes = img.as_bytes().len() * 2 + frame_bytes;
                    self.bytes += bytes;
                    self.tick += 1;
                    let slot = Slot::Ready { img, protos: Vec::new(), pending, asked: None, bytes, used: self.tick, frames, animation: None, animating: None };
                    self.slots.insert(url, slot);
                    self.evict();
                }
                Done::Encoded(url, size, res) => {
                    let Some(Slot::Ready { protos, pending, .. }) = self.slots.get_mut(&url) else { continue };
                    if *pending == Some(size) {
                        *pending = None;
                    }
                    match res {
                        Ok(p) => {
                            // Keep only a couple of sizes per image.
                            if protos.len() >= 2 {
                                protos.remove(0);
                            }
                            protos.push((size, p));
                        }
                        Err(_) => {
                            self.slots.insert(url, Slot::Failed);
                        }
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
                self.bytes -= bytes;
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
        let res = fetch(&url, kind, if kind == Kind::Thumb { disk } else { None }).map(|(img, frames)| (Arc::new(img), frames));
        if let Some(h) = host {
            lock(&q.state).busy.remove(&h);
            q.cv.notify_all();
        }
        // Encode right away for the size the UI asked for, saving a round trip.
        let pending = match (&res, size) {
            (Ok((img, _)), Some(size)) => encode.send(EncodeJob::One(url.clone(), size, img.clone())).is_ok().then_some(size),
            _ => None,
        };
        if tx.send(Done::Fetched(url, res, pending)).is_err() {
            return;
        }
        wake();
    }
}

fn encoder(picker: &Picker, jobs: &Receiver<EncodeJob>, tx: &Sender<Done>, wake: &dyn Fn()) {
    while let Ok(job) = jobs.recv() {
        let done = match job {
            EncodeJob::One(url, size, img) => Done::Encoded(url, size, encode(picker, &img, size)),
            EncodeJob::Frames(url, size, frames) => {
                let res = frames.iter().map(|(f, d)| encode(picker, f, size).map(|p| (p, *d))).collect();
                Done::EncodedFrames(url, size, res)
            }
        };
        if tx.send(done).is_err() {
            return;
        }
        wake();
    }
}

/// Encode `img` to fit in `size` cells. Large images are scaled down from the shared copy
/// first, so the full-size image is never cloned.
fn encode(picker: &Picker, img: &DynamicImage, size: Size) -> Result<Protocol, String> {
    let font = picker.font_size();
    let (w, h) = (size.width as u32 * font.width as u32, size.height as u32 * font.height as u32);
    let small = if img.width() > w || img.height() > h { img.resize(w, h, FilterType::Triangle) } else { img.clone() };
    picker.new_protocol(small, size, Resize::Fit(None)).map_err(|e| e.to_string())
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
        return Ok((decode(&bytes)?, frames));
    }
    if let Some(d) = disk
        && let Some(bytes) = d.get(url)
    {
        match decode(&bytes) {
            Ok(img) => return Ok((img, None)),
            Err(_) => d.remove(url),
        }
    }
    let bytes = http::get_bytes(url, MAX_DOWNLOAD).map_err(|e| format!("{e:#}"))?;
    let img = decode(&bytes)?;
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
    if !bytes.starts_with(b"GIF8") || bytes.len() < 10 {
        return None;
    }
    // Frames, from their graphic control blocks (a slight overcount at worst), and the
    // canvas size from the header: what the frames will take decoded.
    let count = bytes.windows(3).filter(|w| *w == [0x21, 0xf9, 0x04]).count().max(1);
    let (w, h) = (u16::from_le_bytes([bytes[6], bytes[7]]) as f64, u16::from_le_bytes([bytes[8], bytes[9]]) as f64);
    let need = count as f64 * w * h * 4.0;
    let scale = (budget as f64 / need).sqrt().min(1.0);
    if scale < 0.25 {
        return None;
    }
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    let mut frames = Vec::new();
    let mut size = 0;
    for frame in decoder.into_frames() {
        let frame = frame.ok()?;
        let (num, den) = frame.delay().numer_denom_ms();
        // Like browsers: no delay (or almost none) means 100ms.
        let ms = num.checked_div(den).unwrap_or(0);
        let delay = Duration::from_millis(if ms < 20 { 100 } else { ms as u64 });
        let mut img = DynamicImage::ImageRgba8(frame.into_buffer());
        if scale < 1.0 {
            let (fw, fh) = ((img.width() as f64 * scale) as u32, (img.height() as f64 * scale) as u32);
            img = img.resize_exact(fw.max(1), fh.max(1), FilterType::Triangle);
        }
        size += img.as_bytes().len();
        if size > budget {
            return None;
        }
        frames.push((img, delay));
    }
    (frames.len() > 1).then(|| Arc::new(frames))
}

pub(crate) fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    Ok(if img.width() > MAX_DIM || img.height() > MAX_DIM { img.thumbnail(MAX_DIM, MAX_DIM) } else { img })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(*pending, Some(Size::new(110, 32)));
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

