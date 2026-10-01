//! Background image loading and a bounded in-memory cache of decoded images.
//!
//! The UI asks for images every frame with `get`; anything it didn't ask for in a frame is
//! dropped from the fetch queue, so scrolling past a page of thumbnails doesn't fetch them.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

use image::DynamicImage;
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;

use crate::http;

const WORKERS: usize = 4;
/// Decoded images (plus their encoded protocols, estimated at the same size) kept in memory.
const BUDGET_BYTES: usize = 96 * 1024 * 1024;
const MAX_DOWNLOAD: u64 = 25 * 1024 * 1024;
/// Full-size images are scaled down to this before caching; no terminal shows more.
const MAX_DIM: u32 = 2048;

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

/// Fetch queue shared with the workers. `wanted` is what the UI asked for in the last frame.
#[derive(Default)]
struct Queue {
    state: Mutex<(VecDeque<String>, HashSet<String>)>,
    cv: Condvar,
}

type Done = (String, Option<Result<DynamicImage, String>>);

pub struct Images {
    picker: Option<Picker>,
    slots: HashMap<String, Slot>,
    queue: Arc<Queue>,
    rx: Receiver<Done>,
    frame: Vec<String>,
    tick: u64,
    bytes: usize,
}

impl Images {
    /// `None` disables images entirely: no workers, no requests.
    pub fn new(picker: Option<Picker>) -> Self {
        let queue = Arc::new(Queue::default());
        let (tx, rx) = channel();
        if picker.is_some() {
            for _ in 0..WORKERS {
                let (q, tx) = (queue.clone(), tx.clone());
                std::thread::spawn(move || worker(&q, &tx));
            }
        }
        Self { picker, slots: HashMap::new(), queue, rx, frame: Vec::new(), tick: 0, bytes: 0 }
    }

    pub fn enabled(&self) -> bool {
        self.picker.is_some()
    }

    pub fn protocol_name(&self) -> String {
        self.picker.as_ref().map_or("off".into(), |p| format!("{:?}", p.protocol_type()).to_lowercase())
    }

    /// The image at `url` fitted into `size` cells, starting a fetch if needed.
    pub fn get(&mut self, url: &str, size: Size) -> State<'_> {
        self.want(url);
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
    pub fn want(&mut self, url: &str) {
        if self.picker.is_some() {
            self.frame.push(url.to_string());
        }
    }

    /// Call once per frame after drawing: hand this frame's wishes to the workers.
    pub fn end_frame(&mut self) {
        if self.picker.is_none() {
            return;
        }
        let frame = std::mem::take(&mut self.frame);
        let mut st = self.queue.state.lock().unwrap();
        st.1 = frame.iter().cloned().collect();
        for url in frame {
            if !self.slots.contains_key(&url) {
                self.slots.insert(url.clone(), Slot::Loading);
                st.0.push_back(url);
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

fn worker(q: &Queue, tx: &Sender<Done>) {
    loop {
        let url = {
            let mut st = q.state.lock().unwrap();
            loop {
                if let Some(url) = st.0.pop_front() {
                    if st.1.contains(&url) {
                        break url;
                    }
                    if tx.send((url, None)).is_err() {
                        return;
                    }
                    continue;
                }
                st = q.cv.wait(st).unwrap();
            }
        };
        let res = fetch(&url);
        if tx.send((url, Some(res))).is_err() {
            return;
        }
    }
}

fn fetch(url: &str) -> Result<DynamicImage, String> {
    let bytes = http::get_bytes(url, MAX_DOWNLOAD).map_err(|e| format!("{e:#}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    Ok(if img.width() > MAX_DIM || img.height() > MAX_DIM { img.thumbnail(MAX_DIM, MAX_DIM) } else { img })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_makes_no_requests() {
        let mut im = Images::new(None);
        assert!(matches!(im.get("http://x/a.jpg", Size::new(4, 4)), State::Failed));
        im.end_frame();
        assert!(im.queue.state.lock().unwrap().0.is_empty());
    }

    #[test]
    fn eviction_respects_budget() {
        let mut im = Images::new(None);
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
