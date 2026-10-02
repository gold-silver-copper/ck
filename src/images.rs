//! Background image loading and a bounded in-memory cache of decoded images.
//!
//! The UI asks for images every frame with `get`; anything it didn't ask for in a frame is
//! dropped from the fetch queue, so scrolling past a page of thumbnails doesn't fetch them.
//! Thumbnails are also kept on disk. On hosts that share the API's rate limit, one image is
//! fetched at a time, in the order the UI asked (top to bottom).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

use image::DynamicImage;
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

/// Thumbnails are cached on disk; full-size images only in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Thumb,
    Full,
}

pub enum State<'a> {
    Loading,
    Failed,
    Ready(&'a Protocol),
}

enum Slot {
    Loading,
    Failed,
    Ready { img: DynamicImage, protos: Vec<(Size, Protocol)>, bytes: usize, used: u64 },
}

/// Fetch queue shared with the workers.
#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    cv: Condvar,
}

#[derive(Default)]
struct QueueState {
    jobs: VecDeque<(String, Kind)>,
    /// What the UI asked for in the last frame.
    wanted: HashSet<String>,
    /// Rate-limited (non-media) hosts with a fetch in progress.
    busy: HashSet<String>,
}

type Done = (String, Option<Result<DynamicImage, String>>);

/// Called by workers after each result, to wake the UI's main loop.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub struct Images {
    picker: Option<Picker>,
    slots: HashMap<String, Slot>,
    queue: Arc<Queue>,
    rx: Receiver<Done>,
    frame: Vec<(String, Kind)>,
    tick: u64,
    bytes: usize,
}

impl Images {
    /// `None` disables images entirely: no workers, no requests.
    pub fn new(picker: Option<Picker>, wake: Waker, disk: Option<DiskCache>) -> Self {
        let queue = Arc::new(Queue::default());
        let (tx, rx) = channel();
        let disk = disk.map(Arc::new);
        if picker.is_some() {
            for _ in 0..WORKERS {
                let (q, tx, wake, disk) = (queue.clone(), tx.clone(), wake.clone(), disk.clone());
                std::thread::spawn(move || worker(&q, &tx, &*wake, disk.as_deref()));
            }
        }
        Self { picker, slots: HashMap::new(), queue, rx, frame: Vec::new(), tick: 0, bytes: 0 }
    }

    /// Images "on", but nothing is ever fetched: everything stays a placeholder.
    #[cfg(test)]
    pub fn offline() -> Self {
        let (_tx, rx) = channel();
        let picker = Some(Picker::halfblocks());
        Self { picker, slots: HashMap::new(), queue: Arc::new(Queue::default()), rx, frame: Vec::new(), tick: 0, bytes: 0 }
    }

    pub fn enabled(&self) -> bool {
        self.picker.is_some()
    }

    pub fn protocol_name(&self) -> String {
        self.picker.as_ref().map_or("off".into(), |p| format!("{:?}", p.protocol_type()).to_lowercase())
    }

    /// Put an already decoded image in the cache (benchmarks).
    #[cfg(test)]
    pub fn insert_decoded(&mut self, url: &str, img: DynamicImage) {
        let bytes = img.as_bytes().len() * 2;
        self.slots.insert(url.to_string(), Slot::Ready { img, protos: Vec::new(), bytes, used: 0 });
    }

    #[cfg(test)]
    pub fn with_picker(picker: Picker) -> Self {
        Self { picker: Some(picker), ..Self::offline() }
    }

    #[cfg(test)]
    pub fn queued(&self) -> usize {
        self.queue.state.lock().unwrap().jobs.len()
    }

    /// The image at `url` fitted into `size` cells, starting a fetch if needed.
    pub fn get(&mut self, url: &str, size: Size, kind: Kind) -> State<'_> {
        self.want(url, kind);
        let Some(picker) = &self.picker else { return State::Failed };
        self.tick += 1;
        match self.slots.get_mut(url) {
            None | Some(Slot::Loading) => State::Loading,
            Some(Slot::Failed) => State::Failed,
            Some(Slot::Ready { img, protos, used, .. }) => {
                *used = self.tick;
                let i = match protos.iter().position(|(s, _)| *s == size) {
                    Some(i) => i,
                    None => match picker.new_protocol(img.clone(), size, Resize::Fit(None)) {
                        Ok(p) => {
                            // Keep only a couple of sizes per image.
                            if protos.len() >= 2 {
                                protos.remove(0);
                            }
                            protos.push((size, p));
                            protos.len() - 1
                        }
                        Err(_) => return State::Failed,
                    },
                };
                State::Ready(&protos[i].1)
            }
        }
    }

    /// Ask for an image without drawing it (prefetch for rows about to scroll into view).
    pub fn want(&mut self, url: &str, kind: Kind) {
        if self.picker.is_some() {
            self.frame.push((url.to_string(), kind));
        }
    }

    /// Call once per frame after drawing: hand this frame's wishes to the workers.
    pub fn end_frame(&mut self) {
        if self.picker.is_none() {
            return;
        }
        let frame = std::mem::take(&mut self.frame);
        let mut st = self.queue.state.lock().unwrap();
        st.wanted = frame.iter().map(|(u, _)| u.clone()).collect();
        for (url, kind) in frame {
            if !self.slots.contains_key(&url) {
                self.slots.insert(url.clone(), Slot::Loading);
                st.jobs.push_back((url, kind));
            }
        }
        drop(st);
        self.queue.cv.notify_all();
    }

    /// Collect finished fetches.
    pub fn poll(&mut self) {
        while let Ok((url, res)) = self.rx.try_recv() {
            match res {
                // Skipped because it scrolled out of view: forget it so it can be asked again.
                None => {
                    self.slots.remove(&url);
                }
                Some(Err(_)) => {
                    self.slots.insert(url, Slot::Failed);
                }
                Some(Ok(img)) => {
                    let bytes = img.as_bytes().len() * 2;
                    self.bytes += bytes;
                    self.tick += 1;
                    let slot = Slot::Ready { img, protos: Vec::new(), bytes, used: self.tick };
                    self.slots.insert(url, slot);
                    self.evict();
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

fn worker(q: &Queue, tx: &Sender<Done>, wake: &dyn Fn(), disk: Option<&DiskCache>) {
    loop {
        let (url, kind, host) = {
            let mut st = q.state.lock().unwrap();
            loop {
                // Drop what the UI no longer wants.
                while let Some(i) = st.jobs.iter().position(|(u, _)| !st.wanted.contains(u)) {
                    let (url, _) = st.jobs.remove(i).unwrap();
                    if tx.send((url, None)).is_err() {
                        return;
                    }
                }
                if let Some(i) = st.jobs.iter().position(|(u, k)| runnable(&st.busy, u, *k, disk)) {
                    let (url, kind) = st.jobs.remove(i).unwrap();
                    let cached = kind == Kind::Thumb && disk.is_some_and(|d| d.contains(&url));
                    let host = (!cached && !http::is_media_host(&url)).then(|| http::host(&url).to_string());
                    if let Some(h) = &host {
                        st.busy.insert(h.clone());
                    }
                    break (url, kind, host);
                }
                st = q.cv.wait(st).unwrap();
            }
        };
        let res = fetch(&url, if kind == Kind::Thumb { disk } else { None });
        if let Some(h) = host {
            q.state.lock().unwrap().busy.remove(&h);
            q.cv.notify_all();
        }
        if tx.send((url, Some(res))).is_err() {
            return;
        }
        wake();
    }
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
            let img = DynamicImage::new_rgba8(2048, 2048);
            tx.send((format!("u{i}"), Some(Ok(img)))).unwrap();
        }
        im.poll();
        assert!(im.bytes <= BUDGET_BYTES);
        assert!(!im.slots.contains_key("u0"), "oldest image evicted first");
        assert!(im.slots.contains_key("u3"));
    }
}
