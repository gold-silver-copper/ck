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
use crate::http;

const WORKERS: usize = 4;
/// Decoded images (plus their encoded protocols, estimated at the same size) kept in memory.
const BUDGET_BYTES: usize = 96 * 1024 * 1024;
const MAX_DOWNLOAD: u64 = 25 * 1024 * 1024;
/// Full-size images are scaled down to this before caching; no terminal shows more.
const MAX_DIM: u32 = 2048;
/// A new size for an image that's already shown (a resize) is encoded once it has held
/// this long, instead of on every step of the resize.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);

/// Thumbnails are cached on disk; full-size images only in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Thumb,
    Full,
}

pub enum State<'a> {
    Loading,
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
    Fetched(String, Result<Arc<DynamicImage>, String>, Option<Size>),
    Encoded(String, Size, Result<Protocol, String>),
}

type EncodeJob = (String, Size, Arc<DynamicImage>);

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
        Self { picker, slots: HashMap::new(), queue, rx, encode, frame: Vec::new(), tick: 0, bytes: 0 }
    }

    /// Images "on", but nothing is ever fetched or encoded: everything stays a placeholder.
    #[cfg(test)]
    pub fn offline() -> Self {
        let (_tx, rx) = channel();
        let picker = Some(Picker::halfblocks());
        let queue = Arc::new(Queue::default());
        Self { picker, slots: HashMap::new(), queue, rx, encode: None, frame: Vec::new(), tick: 0, bytes: 0 }
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
        let slot = Slot::Ready { img: Arc::new(img), protos: Vec::new(), pending: None, asked: None, bytes, used: 0 };
        self.slots.insert(url.to_string(), slot);
    }

    pub fn enabled(&self) -> bool {
        self.picker.is_some()
    }

    pub fn protocol_name(&self) -> String {
        self.picker.as_ref().map_or("off".into(), |p| format!("{:?}", p.protocol_type()).to_lowercase())
    }

    #[cfg(test)]
    pub fn queued(&self) -> usize {
        self.queue.state.lock().unwrap().jobs.len()
    }

    /// The image at `url` fitted into `size` cells, starting a fetch or an encoding if needed.
    /// While a new size is encoded, a previous encoding is returned if it still fits.
    pub fn get(&mut self, url: &str, size: Size, kind: Kind) -> State<'_> {
        if self.picker.is_none() {
            return State::Failed;
        }
        self.frame.push((url.to_string(), kind, Some(size)));
        self.tick += 1;
        let Some(Slot::Ready { img, protos, pending, asked, used, .. }) = self.slots.get_mut(url) else {
            return match self.slots.get(url) {
                Some(Slot::Failed) => State::Failed,
                _ => State::Loading,
            };
        };
        *used = self.tick;
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
                let _ = enc.send((url.to_string(), size, img.clone()));
                *pending = Some(size);
            }
        }
        match protos.iter().rev().find(|(_, p)| p.size().width <= size.width && p.size().height <= size.height) {
            Some((_, p)) => State::Ready(p),
            None => State::Rendering,
        }
    }

    /// Ask for an image without drawing it (prefetch for rows about to scroll into view).
    pub fn want(&mut self, url: &str, kind: Kind) {
        if self.picker.is_some() {
            self.frame.push((url.to_string(), kind, None));
        }
    }

    /// Call once per frame after drawing: hand this frame's wishes to the workers.
    pub fn end_frame(&mut self) {
        if self.picker.is_none() {
            return;
        }
        let frame = std::mem::take(&mut self.frame);
        let mut st = self.queue.state.lock().unwrap();
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
                Done::Fetched(url, Ok(img), pending) => {
                    let bytes = img.as_bytes().len() * 2;
                    self.bytes += bytes;
                    self.tick += 1;
                    let slot = Slot::Ready { img, protos: Vec::new(), pending, asked: None, bytes, used: self.tick };
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
            }
        }
    }

    fn evict(&mut self) {
        while self.bytes > BUDGET_BYTES {
            let oldest = self
                .slots
                .iter()
                .filter_map(|(k, s)| match s {
                    Slot::Ready { used, .. } => Some((*used, k)),
                    _ => None,
                })
                .min()
                .map(|(_, k)| k.clone());
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
            let mut st = q.state.lock().unwrap();
            loop {
                // Drop what the UI no longer wants.
                while let Some(i) = st.jobs.iter().position(|(u, ..)| !st.wanted.contains(u)) {
                    let (url, ..) = st.jobs.remove(i).unwrap();
                    if tx.send(Done::Skipped(url)).is_err() {
                        return;
                    }
                }
                if let Some(i) = st.jobs.iter().position(|(u, k, _)| runnable(&st.busy, u, *k, disk)) {
                    let (url, kind, size) = st.jobs.remove(i).unwrap();
                    let cached = kind == Kind::Thumb && disk.is_some_and(|d| d.contains(&url));
                    let host = (!cached && !http::is_media_host(&url)).then(|| http::host(&url).to_string());
                    if let Some(h) = &host {
                        st.busy.insert(h.clone());
                    }
                    break (url, kind, size, host);
                }
                st = q.cv.wait(st).unwrap();
            }
        };
        let res = fetch(&url, if kind == Kind::Thumb { disk } else { None }).map(Arc::new);
        if let Some(h) = host {
            q.state.lock().unwrap().busy.remove(&h);
            q.cv.notify_all();
        }
        // Encode right away for the size the UI asked for, saving a round trip.
        let pending = match (&res, size) {
            (Ok(img), Some(size)) => encode.send((url.clone(), size, img.clone())).is_ok().then_some(size),
            _ => None,
        };
        if tx.send(Done::Fetched(url, res, pending)).is_err() {
            return;
        }
        wake();
    }
}

fn encoder(picker: &Picker, jobs: &Receiver<EncodeJob>, tx: &Sender<Done>, wake: &dyn Fn()) {
    while let Ok((url, size, img)) = jobs.recv() {
        let res = encode(picker, &img, size);
        if tx.send(Done::Encoded(url, size, res)).is_err() {
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
    (kind == Kind::Thumb && disk.is_some_and(|d| d.contains(url)))
        || http::is_media_host(url)
        || !busy.contains(http::host(url))
}

/// Load an image: a thumbnail from the disk cache if it's there, else over HTTP (caching
/// thumbnails that decode). A cached file that won't decode is deleted and fetched again.
fn fetch(url: &str, disk: Option<&DiskCache>) -> Result<DynamicImage, String> {
    if let Some(bytes) = disk.and_then(|d| d.get(url)) {
        match decode(&bytes) {
            Ok(img) => return Ok(img),
            Err(_) => disk.unwrap().remove(url),
        }
    }
    let bytes = http::get_bytes(url, MAX_DOWNLOAD).map_err(|e| format!("{e:#}"))?;
    let img = decode(&bytes)?;
    if let Some(d) = disk {
        let _ = d.put(url, &bytes);
    }
    Ok(img)
}

fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
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
        assert!(im.queue.state.lock().unwrap().jobs.is_empty());
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
        assert_eq!(fetch(url, Some(&disk)).unwrap().width(), 4);
        // A corrupt entry is deleted (and the fetch then fails here, offline).
        disk.put(url, b"not an image").unwrap();
        assert!(fetch(url, Some(&disk)).is_err());
        assert!(!disk.contains(url));
    }

    #[test]
    fn eviction_respects_budget() {
        let mut im = Images::new(None, Arc::new(|| {}), None);
        let (tx, rx) = channel();
        im.rx = rx;
        // Each 2048x2048 RGBA image counts as 32 MiB (doubled for its protocol).
        for i in 0..4 {
            let img = Arc::new(DynamicImage::new_rgba8(2048, 2048));
            tx.send(Done::Fetched(format!("u{i}"), Ok(img), None)).unwrap();
        }
        im.poll();
        assert!(im.bytes <= BUDGET_BYTES);
        assert!(!im.slots.contains_key("u0"), "oldest image evicted first");
        assert!(im.slots.contains_key("u3"));
    }
}
