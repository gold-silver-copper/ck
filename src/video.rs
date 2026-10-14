//! Videos (webm, mp4) played in the viewer. ck downloads the file with its own HTTP client,
//! as it does images (the same rate limits, into ck's cache folder), and plays it from
//! there. `ffmpeg` decodes it: scaled to the viewer, padded to whole cells and 20 frames a
//! second, raw RGBA on a pipe. A thread turns each frame into what the terminal shows
//! (kitty, sixel, half-blocks) as images are, a couple of frames ahead, and drops the ones
//! it's too late for. The sound plays in a second program (`ffplay`, else mpv), started and
//! stopped with the picture. Nothing is linked: without `ffmpeg` and `ffprobe` on PATH,
//! videos show their thumbnail as before.
#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, channel, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use image::{DynamicImage, RgbaImage};
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::cap_parser::Parser;
use ratatui_image::picker::{Capability, Picker, ProtocolType};
use ratatui_image::protocol::Protocol;

use crate::http::lock;

/// Frames a second: at most what the viewer redraws (as for GIFs), and at least this many
/// before a big video is made smaller.
const FPS: u32 = 20;
const MIN_FPS: u32 = 10;
/// What frames may take a second in kitty, sixel or iTerm2 (before compression, which kitty
/// may do), so a terminal (or a link to it) isn't swamped: a 640x360 video plays at 20
/// frames a second, a bigger one at fewer, and past 10 it's made smaller.
const PIXEL_BYTES_PER_SECOND: u64 = 24 * 1024 * 1024;
/// Frames decoded and encoded ahead of the one on screen.
const AHEAD: usize = 2;
/// Threads turning frames into terminal output (sixel especially takes its time).
const ENCODERS: usize = 2;
/// The biggest video downloaded to play.
const MAX_VIDEO: u64 = 256 * 1024 * 1024;
/// Videos played are kept in the cache folder up to this; the least recently played go.
const CACHE_BUDGET: u64 = 512 * 1024 * 1024;
/// The containers ffmpeg may read: webm and mp4 and their kin, not playlists or other
/// formats that read further files or URLs.
const FORMATS: &str = "matroska,webm,mov,mp4,m4a,3gp,3g2,mj2";
/// No real video is wider or taller.
const MAX_SIDE: u32 = 16384;
/// A seek short of the end goes this far before it, so there's something to show.
const END_MARGIN: Duration = Duration::from_millis(500);
/// A new size waits this long to settle before decoding starts over for it (as images).
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);

/// Whether videos can play here: `ffmpeg` and `ffprobe` on PATH (looked for once). Never in
/// tests, which mustn't start programs.
pub fn available() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    !crate::sandboxed() && *FOUND.get_or_init(|| crate::app::on_path("ffmpeg") && crate::app::on_path("ffprobe"))
}

/// The program the sound plays in: ffplay (it comes with ffmpeg), else mpv (looked for once).
fn sound_program() -> Option<&'static str> {
    static FOUND: OnceLock<Option<&'static str>> = OnceLock::new();
    *FOUND.get_or_init(|| ["ffplay", "mpv"].into_iter().find(|p| crate::app::on_path(p)))
}

/// What ffprobe says about a video: its size as shown (rotated, and with its pixels'
/// shape), how long it is, and whether it has sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub width: u32,
    pub height: u32,
    pub duration: Option<Duration>,
    pub audio: bool,
}

/// `ffprobe -of json`'s answer (see `probe_args`).
fn parse_probe(json: &str) -> Option<Probe> {
    use serde_json::Value;
    let v: Value = serde_json::from_str(json).ok()?;
    let streams = v.get("streams")?.as_array()?;
    let kind = |s: &Value| s.get("codec_type").and_then(Value::as_str).map(str::to_string);
    let video = streams.iter().find(|s| kind(s).as_deref() == Some("video"))?;
    let side = |k: &str| video.get(k).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).filter(|n| (1..=MAX_SIDE).contains(n));
    let (w, h) = (side("width")?, side("height")?);
    // Pixels that aren't square (anamorphic video) are shown wider or narrower.
    let sar = video.get("sample_aspect_ratio").and_then(Value::as_str).and_then(|s| s.split_once(':'));
    let sar = sar.and_then(|(n, d)| Some((n.parse::<u64>().ok()?, d.parse::<u64>().ok()?))).filter(|&(n, d)| n > 0 && d > 0);
    let w = sar.map_or(w, |(n, d)| u32::try_from(u64::from(w).saturating_mul(n).checked_div(d).unwrap_or(0)).unwrap_or(MAX_SIDE).clamp(1, MAX_SIDE));
    // A phone's video, filmed upright, is stored on its side and turned as it plays.
    let rotation = video.get("side_data_list").and_then(Value::as_array).into_iter().flatten().find_map(|d| d.get("rotation").and_then(Value::as_f64)).unwrap_or(0.0);
    #[allow(clippy::cast_possible_truncation)] // f64 as i64 saturates
    let quarter = (rotation / 90.0).round() as i64;
    let (width, height) = if quarter.rem_euclid(2) == 1 { (h, w) } else { (w, h) };
    let duration = v.get("format").and_then(|f| f.get("duration")).and_then(Value::as_str).and_then(|d| d.parse::<f64>().ok());
    let duration = duration.filter(|d| *d > 0.0).and_then(|d| Duration::try_from_secs_f64(d).ok());
    let audio = streams.iter().any(|s| kind(s).as_deref() == Some("audio"));
    Some(Probe { width, height, duration, audio })
}

fn probe_args(path: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = ["-v", "error", "-protocol_whitelist", "file", "-format_whitelist", FORMATS]
        .into_iter()
        .chain(["-show_entries", "stream=codec_type,width,height,sample_aspect_ratio:stream_side_data=rotation:format=duration", "-of", "json"])
        .map(Into::into)
        .collect();
    args.push(path.into());
    args
}

fn probe(path: &Path) -> Result<Probe, String> {
    let out = crate::quiet_command("ffprobe", probe_args(path)).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| format!("couldn't run ffprobe: {e}"))?;
    if !out.status.success() {
        // "<path>: Invalid data found when processing input": what's wrong, without the path.
        let why = String::from_utf8_lossy(&out.stderr);
        let path = format!("{}: ", path.display());
        let why = why.lines().last().map_or("", |l| l.trim().trim_start_matches(&path));
        return Err(if why.is_empty() { "ffprobe couldn't read it".into() } else { why.to_lowercase() });
    }
    parse_probe(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| "it has no picture ffmpeg can show".into())
}

/// Seconds, as ffmpeg and the players take them.
fn secs(d: Duration) -> String {
    format!("{:.3}", d.as_secs_f64())
}

/// Where a video is drawn: `part` of it (zoomed in: left, top, width and height, in
/// thousandths of its own), scaled to `scaled` pixels, padded (transparent) to `padded`,
/// which fill `cells` (for half-blocks, a pixel is half a cell); `fps` frames a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub part: Option<[u32; 4]>,
    pub scaled: (u32, u32),
    pub padded: (u32, u32),
    pub cells: Size,
    pub fps: u32,
}

impl Geometry {
    /// How long each frame shows.
    fn frame(self) -> Duration {
        Duration::from_secs(1).checked_div(self.fps.max(1)).unwrap_or(Duration::from_millis(50))
    }
}

/// How a `w` x `h` video is drawn in `area` cells, at the terminal's cell size `font`
/// (pixels): fitted to the area, scaled up too (as players do: a small video isn't left
/// small), or the part `crop` zooms into; in kitty, sixel or iTerm2, within
/// `PIXEL_BYTES_PER_SECOND` (fewer frames, then smaller).
pub fn geometry((w, h): (u32, u32), area: Size, font: (u32, u32), halfblocks: bool, crop: crate::images::Crop) -> Geometry {
    let (fw, fh) = (font.0.max(1), font.1.max(1));
    let (aw, ah) = (u32::from(area.width).saturating_mul(fw).max(1), u32::from(area.height).saturating_mul(fh).max(1));
    // Zoomed in: what's drawn is that part, at its size in the video.
    let (part, (w, h)) = if crop.is_fit() {
        (None, (w.max(1), h.max(1)))
    } else {
        let (x, y, rw, rh) = crop.region(w, h, (aw, ah));
        let of = |v: u32, side: u32| u32::try_from(u64::from(v).saturating_mul(1000).checked_div(u64::from(side.max(1))).unwrap_or(0)).unwrap_or(1000).min(1000);
        (Some([of(x, w), of(y, h), of(rw, w).max(1), of(rh, h).max(1)]), (rw.max(1), rh.max(1)))
    };
    if halfblocks {
        let (cells, cw, ch) = crate::images::halfblock_grid((w, h), area, font, true);
        return Geometry { part, scaled: (cw, ch), padded: (u32::from(cells.width), u32::from(cells.height).saturating_mul(2)), cells, fps: FPS };
    }
    let fitted = f64::min(f64::from(aw) / f64::from(w), f64::from(ah) / f64::from(h));
    // Smaller still if even `MIN_FPS` frames a second would be too much.
    let most = PIXEL_BYTES_PER_SECOND as f64 / f64::from(MIN_FPS) / 4.0;
    let ratio = fitted * f64::min(1.0, (most / (f64::from(w) * f64::from(h) * fitted * fitted)).sqrt());
    #[allow(clippy::cast_possible_truncation)] // f64 as u32 saturates
    let (sw, sh) = (((f64::from(w) * ratio).round() as u32).clamp(1, aw), ((f64::from(h) * ratio).round() as u32).clamp(1, ah));
    let cells_in = |px: u32, f: u32, max: u16| u16::try_from(px.div_ceil(f)).unwrap_or(u16::MAX).clamp(1, max.max(1));
    let cells = Size::new(cells_in(sw, fw, area.width), cells_in(sh, fh, area.height));
    let padded = (u32::from(cells.width).saturating_mul(fw), u32::from(cells.height).saturating_mul(fh));
    let bytes = u64::from(padded.0).saturating_mul(u64::from(padded.1)).saturating_mul(4).max(1);
    let fps = u32::try_from(PIXEL_BYTES_PER_SECOND.checked_div(bytes).unwrap_or(0)).unwrap_or(FPS).clamp(MIN_FPS, FPS);
    Geometry { part, scaled: (sw, sh), padded, cells, fps }
}

/// ffmpeg's arguments to decode `path` from `from` on, as `g` says, to raw RGBA frames on
/// its stdout, `g.fps` a second.
fn decoder_args(path: &Path, from: Duration, g: Geometry) -> Vec<std::ffi::OsString> {
    let ((sw, sh), (pw, ph)) = (g.scaled, g.padded);
    // A part, zoomed in: cut from the picture as it's shown (turned upright, though its
    // pixels may not be square: so in proportions of its width and height).
    let part = g.part.map_or_else(String::new, |[x, y, w, h]| {
        let k = |v: u32| f64::from(v) / 1000.0;
        format!("crop=iw*{}:ih*{}:iw*{}:ih*{},", k(w), k(h), k(x), k(y))
    });
    let filters = format!("fps={},{part}scale={sw}:{sh}:flags=bilinear,pad={pw}:{ph}:0:0:color=black@0,format=rgba", g.fps);
    let mut args: Vec<std::ffi::OsString> =
        ["-nostdin", "-hide_banner", "-loglevel", "error", "-protocol_whitelist", "file", "-format_whitelist", FORMATS, "-ss", &secs(from), "-i"].into_iter().map(Into::into).collect();
    args.push(path.into());
    args.extend(["-an", "-sn", "-dn", "-vf", &filters, "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"].into_iter().map(Into::into));
    args
}

/// The program and arguments that play `path`'s sound from `from` on, without a window.
fn sound_args(program: &str, path: &Path, from: Duration) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = if program == "mpv" {
        ["--no-video", "--no-terminal", "--really-quiet", "--loop-file=no"].into_iter().map(Into::into).chain([format!("--start={}", secs(from)).into(), "--".into()]).collect()
    } else {
        ["-nodisp", "-autoexit", "-loglevel", "quiet", "-protocol_whitelist", "file", "-format_whitelist", FORMATS, "-ss", &secs(from)].into_iter().map(Into::into).collect()
    };
    args.push(path.into());
    args
}

/// A program ck started, ended when it's dropped (and waited on, so it doesn't linger).
struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Where playback is: at `at` when `since` was, running since then (`None`: stopped).
#[derive(Debug, Clone, Copy)]
struct Clock {
    at: Duration,
    since: Option<Instant>,
}

impl Clock {
    fn now(&self, t: Instant) -> Duration {
        self.since.map_or(self.at, |s| self.at.saturating_add(t.saturating_duration_since(s)))
    }

    fn stop(&mut self, t: Instant) {
        *self = Clock { at: self.now(t), since: None };
    }
}

/// What the decoding thread hands the viewer.
enum Frame {
    /// The picture for this point in the video, the decoder's `n`th (see `Encode`).
    Picture(Duration, u64, Protocol),
    /// No more: the end, or ffmpeg stopped.
    End,
}

/// ffmpeg decoding, and the frames it's made ready.
struct Decoder {
    rx: Receiver<Frame>,
    /// Set by the viewer when it has no frame waiting: the next one wakes it.
    starving: Arc<AtomicBool>,
    geometry: Geometry,
    /// The area it decodes for, in cells.
    area: Size,
    /// Ended when dropped: the thread then gets an error sending (`rx` is gone) or the end
    /// of the pipe reading.
    ffmpeg: Process,
}

/// Turns a frame (`padded` pixels) into what the terminal shows in `cells`: the decoder's
/// `n`th shown (0: the first, after a start or a new size).
pub type Encode = Arc<dyn Fn(DynamicImage, Size, u64) -> Result<Protocol, String> + Send + Sync>;

/// Where frames go: to the viewer, waking it (`wake`) when it's `starving` for one.
struct Sink<'a> {
    tx: &'a SyncSender<Frame>,
    starving: &'a AtomicBool,
    wake: &'a (dyn Fn() + Sync),
}

/// Read raw frames of `g.padded` pixels from `out` (the first at `from`, numbered from
/// `first` on as they're encoded), and send the ones
/// still due to the viewer as pictures, then `End`. A frame the clock has already passed is
/// dropped unencoded, and so is one just like the last (a still scene, a video slower than
/// `g.fps`). `ENCODERS` threads encode them, taking turns, and they're sent in order.
fn pump(mut out: impl Read, g: Geometry, (from, first): (Duration, u64), clock: &Mutex<Clock>, encode: &Encode, sink: &Sink) {
    let (pw, ph) = g.padded;
    let len = usize::try_from(u64::from(pw).saturating_mul(u64::from(ph)).saturating_mul(4)).unwrap_or(usize::MAX);
    let send = |f: Frame| {
        let sent = sink.tx.send(f).is_ok();
        if sink.starving.swap(false, Ordering::SeqCst) {
            (sink.wake)();
        }
        sent
    };
    std::thread::scope(|scope| {
        let (mut work, mut done) = (Vec::new(), Vec::new());
        for _ in 0..ENCODERS {
            let (in_tx, in_rx) = sync_channel::<(u64, Duration, Vec<u8>)>(1);
            let (out_tx, out_rx) = sync_channel(1);
            scope.spawn(move || {
                for (n, pts, bytes) in in_rx {
                    let img = RgbaImage::from_raw(pw, ph, bytes).ok_or_else(|| "a frame of the wrong size".to_string());
                    let res = crate::guard::catching(|| img.and_then(|img| encode(DynamicImage::ImageRgba8(img), g.cells, n))).unwrap_or_else(Err);
                    if out_tx.send((pts, n, res)).is_err() {
                        return;
                    }
                }
            });
            work.push(in_tx);
            done.push(out_rx);
        }
        // Back in order (each encoder's next is the next frame), to the viewer.
        scope.spawn(move || {
            for i in 0usize.. {
                let Some(rx) = done.get(i.checked_rem(ENCODERS).unwrap_or(0)) else { break };
                match rx.recv() {
                    Ok((pts, n, Ok(p))) => {
                        if !send(Frame::Picture(pts, n, p)) {
                            return;
                        }
                    }
                    // A frame that couldn't be encoded is left out.
                    Ok((_, _, Err(_))) => {}
                    Err(_) => break,
                }
            }
            send(Frame::End);
        });
        let mut buf = vec![0u8; len];
        let mut last: Vec<u8> = Vec::new();
        let (mut at, mut n) = (from, first);
        while out.read_exact(&mut buf).is_ok() {
            let pts = at;
            at = at.saturating_add(g.frame());
            let late = pts.saturating_add(g.frame()) < lock(clock).now(Instant::now());
            if late || buf == last {
                continue;
            }
            last.clone_from(&buf);
            let to = work.get(usize::try_from(n.wrapping_sub(first)).unwrap_or(0).checked_rem(ENCODERS).unwrap_or(0));
            if to.is_none_or(|w| w.send((n, pts, buf.clone())).is_err()) {
                break;
            }
            n = n.saturating_add(1);
        }
        // Done: the encoders finish what they have, then the frames end.
        drop(work);
    });
}

/// The encoder for one video's frames, as images are, but so that a frame never leaves the
/// screen before the next is all there (images are cleared first, which played 20 times a
/// second is a flicker):
/// - kitty: frames take turns between two image numbers, so a new one goes up beside the
///   one showing, and replaces it once it's there; the one before is replaced in turn;
/// - sixel, iTerm2: the area is cleared only for the first frame (after a start or a new
///   size); the ones after cover it exactly. Sixel without dithering, which redone on each
///   frame made still parts shimmer (and is slower).
pub fn encoder(picker: &Picker) -> Encode {
    let picker = picker.clone();
    // Numbers of its own (kitty's image ids are the program's to pick).
    let id = u32::try_from(std::hash::BuildHasher::hash_one(&std::collections::hash_map::RandomState::new(), Instant::now()) >> 40).unwrap_or(1) | 1;
    let compress = picker.capabilities().contains(&Capability::KittyCompression);
    let tmux = picker.tmux_detected();
    Arc::new(move |img, cells, n| match picker.protocol_type() {
        ProtocolType::Halfblocks => crate::images::halfblocks_of(img, cells),
        ProtocolType::Kitty => {
            let id = if n & 1 == 0 { id } else { id ^ 2 };
            ratatui_image::protocol::kitty::Kitty::new(img, cells, id, tmux, compress).map(Protocol::Kitty).map_err(|e| e.to_string())
        }
        ProtocolType::Sixel => sixel(&img, cells, tmux, n == 0).map(Protocol::Sixel),
        ProtocolType::Iterm2 => match picker.new_protocol(img, cells, Resize::Fit(None)).map_err(|e| e.to_string())? {
            Protocol::ITerm2(mut p) if n > 0 => {
                p.data = p.data.replacen(&clear(cells, tmux), "", 1);
                Ok(Protocol::ITerm2(p))
            }
            p => Ok(p),
        },
    })
}

/// What clears `cells` before an image is drawn there (as ratatui-image does it): each row
/// erased, then back up.
fn clear(cells: Size, tmux: bool) -> String {
    let (_, esc, _) = Parser::tmux_start_escape_end(tmux);
    if cells.height == 1 {
        return format!("{esc}[{}X", cells.width);
    }
    let row = format!("{esc}[{}X{esc}[1B", cells.width);
    format!("{}{esc}[{}A", row.repeat(usize::from(cells.height)), cells.height)
}

/// A frame in sixel, clearing `cells` first if `fresh`.
fn sixel(img: &DynamicImage, cells: Size, tmux: bool, fresh: bool) -> Result<ratatui_image::protocol::sixel::Sixel, String> {
    let rgba = img.as_rgba8().ok_or("not RGBA")?;
    let (w, h) = (usize::try_from(rgba.width()).unwrap_or(0), usize::try_from(rgba.height()).unwrap_or(0));
    let options = icy_sixel::EncodeOptions { diffusion: 0.0, ..icy_sixel::EncodeOptions::default() };
    let six = icy_sixel::sixel_encode(rgba.as_raw(), w, h, &options).map_err(|e| format!("sixel: {e}"))?;
    let (start, esc, end) = Parser::tmux_start_escape_end(tmux);
    let mut data = String::from(start);
    if fresh {
        data.push_str(&clear(cells, tmux));
    }
    // Inside tmux's passthrough its escape is doubled.
    data.push_str(esc);
    data.push_str(six.strip_prefix('\x1b').ok_or("sixel without its escape")?);
    data.push_str(end);
    Ok(ratatui_image::protocol::sixel::Sixel { data, size: cells, is_tmux: tmux })
}

/// Where a video at `url` is kept once downloaded.
fn cache_dir() -> Option<PathBuf> {
    Some(crate::config::folder(crate::config::Folder::Cache)?.join("videos"))
}

fn cached_path(url: &str) -> Option<PathBuf> {
    Some(cache_dir()?.join(format!("{:032x}", crate::disk_cache::fnv1a128(url.as_bytes()))))
}

/// Whether the video at `url` can be had without the network.
pub fn on_disk(url: &str) -> bool {
    url.starts_with("file://") || cached_path(url).is_some_and(|p| p.is_file())
}

/// The video at `url` on disk: a saved copy's (`file://`), the cache's, or downloaded into
/// the cache now (telling `progress` the percent done), then what ffprobe says of it.
fn fetch(url: &str, progress: &AtomicU8) -> Result<(PathBuf, Probe), String> {
    let path = match url.strip_prefix("file://") {
        Some(p) => PathBuf::from(p),
        None => {
            let path = cached_path(url).ok_or("there's no cache folder to download it to")?;
            if path.is_file() {
                // Played again: the most recently used, last to be trimmed.
                #[allow(clippy::disallowed_methods)] // opened to touch its time, never written
                if let Ok(f) = std::fs::File::options().write(true).open(&path) {
                    let _ = f.set_modified(std::time::SystemTime::now());
                }
            } else {
                let pct = |got: u64, size: Option<u64>| {
                    let p = size.filter(|s| *s > 0).map_or(u8::MAX, |s| u8::try_from(got.saturating_mul(100).checked_div(s).unwrap_or(0).min(100)).unwrap_or(100));
                    progress.store(p, Ordering::Relaxed);
                };
                crate::http::download_with_progress(url, &path, MAX_VIDEO, pct).map_err(|e| crate::http::plain(&e))?;
                if let Some(dir) = cache_dir() {
                    crate::atomic::trim(&dir, 0, CACHE_BUDGET);
                }
            }
            path
        }
    };
    Ok((path.clone(), probe(&path)?))
}

/// What the viewer shows of a video.
pub enum Shown<'a> {
    /// Downloading (the percent done, if known), or waiting for the first frame.
    Loading(Option<u8>),
    /// Offline, and not on disk: it won't be fetched.
    Unavailable,
    Failed(&'a str),
    Ready(&'a Protocol),
}

/// The video in the viewer: downloading, failed, or playing.
pub struct Player {
    pub url: String,
    stage: Stage,
}

enum Stage {
    Fetching { rx: Receiver<Result<(PathBuf, Probe), String>>, progress: Arc<AtomicU8>, from: Duration, muted: bool, encode: Encode, wake: crate::images::Waker },
    Failed(String),
    Playing(Box<Playing>),
}

struct Playing {
    path: PathBuf,
    probe: Probe,
    clock: Arc<Mutex<Clock>>,
    /// Started (or restarted, after a seek or resize), and no frame shown since.
    waiting: bool,
    paused: bool,
    decoder: Option<Decoder>,
    /// A new size for the viewer, and since when (it's decoded for once it's held).
    asked: Option<(Size, Instant)>,
    shown: Option<(Duration, Protocol)>,
    next: Option<(Duration, u64, Protocol)>,
    /// The size the frame on screen was decoded for, and its number: a restart for the same
    /// size goes on numbering (no clearing; kitty's turns go on), one for another starts over.
    last: Option<(Geometry, u64)>,
    ended: bool,
    sound: Option<Process>,
    muted: bool,
    encode: Encode,
    wake: crate::images::Waker,
}

impl Player {
    /// Start getting the video at `url` ready, to play from `from` (without sound if
    /// `muted`), its frames made with `encode`; `wake` the viewer when one comes that it's
    /// waiting for.
    pub fn new(url: &str, from: Duration, muted: bool, encode: Encode, wake: crate::images::Waker) -> Self {
        let (tx, rx) = channel();
        let progress = Arc::new(AtomicU8::new(u8::MAX));
        let (u, p) = (url.to_string(), progress.clone());
        std::thread::spawn(move || {
            let res = crate::guard::catching(|| fetch(&u, &p)).unwrap_or_else(|what| Err(format!("ck hit a bug: {what}")));
            let _ = tx.send(res);
        });
        Player { url: url.to_string(), stage: Stage::Fetching { rx, progress, from, muted, encode, wake } }
    }

    /// A player that failed (tests).
    #[cfg(test)]
    pub fn failed(url: &str, why: &str) -> Self {
        Player { url: url.to_string(), stage: Stage::Failed(why.to_string()) }
    }

    /// The frame due now, for `area` cells (the part `crop` zooms into); and when the viewer
    /// should draw again (the next frame, or the clock's next second), if it's playing.
    pub fn frame(&mut self, (area, crop): (Size, crate::images::Crop), font: (u32, u32), halfblocks: bool, now: Instant) -> (Shown<'_>, Option<Instant>) {
        if let Stage::Fetching { rx, progress, from, muted, encode, wake } = &self.stage {
            match rx.try_recv() {
                Ok(Ok((path, probe))) => {
                    let (encode, wake, muted) = (encode.clone(), wake.clone(), *muted);
                    let clock = Arc::new(Mutex::new(Clock { at: *from, since: None }));
                    let p = Playing { path, probe, clock, waiting: true, paused: false, decoder: None, asked: None, shown: None, next: None, last: None, ended: false, sound: None, muted, encode, wake };
                    self.stage = Stage::Playing(Box::new(p));
                }
                Ok(Err(e)) => self.stage = Stage::Failed(e),
                Err(TryRecvError::Disconnected) => self.stage = Stage::Failed("ck hit a bug getting it".into()),
                Err(TryRecvError::Empty) => {
                    let pct = progress.load(Ordering::Relaxed);
                    return (Shown::Loading((pct <= 100).then_some(pct)), None);
                }
            }
        }
        match &mut self.stage {
            Stage::Fetching { .. } => (Shown::Loading(None), None),
            Stage::Failed(why) => (Shown::Failed(why), None),
            Stage::Playing(p) => p.frame((area, crop), font, halfblocks, now),
        }
    }

    fn playing(&mut self) -> Option<&mut Playing> {
        match &mut self.stage {
            Stage::Playing(p) => Some(p),
            _ => None,
        }
    }

    /// Pause or play. False if it isn't playing (yet).
    pub fn toggle_pause(&mut self) -> bool {
        let Some(p) = self.playing() else { return false };
        let now = Instant::now();
        p.paused = !p.paused;
        if p.paused {
            lock(&p.clock).stop(now);
            p.sound = None;
        } else if !p.waiting {
            let at = {
                let mut c = lock(&p.clock);
                c.since = Some(now);
                c.at
            };
            p.start_sound(at);
        }
        true
    }

    /// It couldn't be played (see `Shown::Failed`).
    pub fn has_failed(&self) -> bool {
        matches!(&self.stage, Stage::Failed(_)) || matches!(&self.stage, Stage::Playing(p) if p.ended && p.shown.is_none() && p.next.is_none())
    }

    pub fn is_paused(&self) -> bool {
        matches!(&self.stage, Stage::Playing(p) if p.paused)
    }

    /// Its size as shown, once it's known.
    pub fn dims(&self) -> Option<(u32, u32)> {
        let Stage::Playing(p) = &self.stage else { return None };
        Some((p.probe.width, p.probe.height))
    }

    /// Where it is, and how long it is.
    pub fn position(&self) -> Option<(Duration, Option<Duration>)> {
        let Stage::Playing(p) = &self.stage else { return None };
        Some((lock(&p.clock).now(Instant::now()), p.probe.duration))
    }

    /// Where it is now, or where it would start.
    pub fn at(&self) -> Duration {
        match &self.stage {
            Stage::Fetching { from, .. } => *from,
            Stage::Failed(_) => Duration::ZERO,
            Stage::Playing(p) => lock(&p.clock).now(Instant::now()),
        }
    }

    /// Whether it has sound, and nothing to play it in.
    pub fn soundless(&self) -> bool {
        matches!(&self.stage, Stage::Playing(p) if p.probe.audio) && sound_program().is_none()
    }

    /// Jump `by` seconds (back, if negative), within the video.
    pub fn seek(&mut self, by: i64) {
        let Some(p) = self.playing() else { return };
        let now = Instant::now();
        let at = lock(&p.clock).now(now);
        let to = if by < 0 { at.saturating_sub(Duration::from_secs(by.unsigned_abs())) } else { at.saturating_add(Duration::from_secs(by.unsigned_abs())) };
        let to = p.probe.duration.map_or(to, |d| to.min(d.saturating_sub(END_MARGIN)));
        p.restart(to, p.decoder.as_ref().map(|d| (d.geometry, d.area)));
    }

    /// Sound off, or on again.
    pub fn set_muted(&mut self, muted: bool) {
        match &mut self.stage {
            Stage::Fetching { muted: m, .. } => *m = muted,
            Stage::Failed(_) => {}
            Stage::Playing(p) => {
                p.muted = muted;
                p.sound = None;
                if !p.paused && !p.waiting {
                    let at = lock(&p.clock).now(Instant::now());
                    p.start_sound(at);
                }
            }
        }
    }
}

impl Playing {
    fn frame(&mut self, (area, crop): (Size, crate::images::Crop), font: (u32, u32), halfblocks: bool, now: Instant) -> (Shown<'_>, Option<Instant>) {
        let g = geometry((self.probe.width, self.probe.height), area, font, halfblocks, crop);
        let pos = lock(&self.clock).now(now);
        match &self.decoder {
            None if !self.ended => self.restart(pos, Some((g, area))),
            // A zoom or a move (a key, once): decoded for it at once.
            Some(d) if d.geometry != g && d.area == area => self.restart(pos, Some((g, area))),
            // A resize: decoded for the new size once it's held.
            Some(d) if d.geometry != g => match self.asked {
                Some((s, since)) if s == area && now.saturating_duration_since(since) >= RESIZE_DEBOUNCE => {
                    self.asked = None;
                    self.restart(pos, Some((g, area)));
                }
                Some((s, _)) if s == area => {}
                _ => self.asked = Some((area, now)),
            },
            _ => self.asked = None,
        }
        self.take_frames(now);
        // The sound program gone already (it ended, or found no sound device): let it go. And
        // ffmpeg done before its last frames are shown: let it go too (they're kept).
        if self.sound.as_mut().is_some_and(|s| !matches!(s.0.try_wait(), Ok(None))) {
            self.sound = None;
        }
        if let Some(d) = &mut self.decoder {
            let _ = d.ffmpeg.0.try_wait();
        }
        // The end: round again from the start.
        let pos = lock(&self.clock).now(now);
        let end = self.probe.duration.unwrap_or_else(|| self.shown.as_ref().map_or(Duration::ZERO, |(t, _)| t.saturating_add(g.frame())));
        if self.ended && self.next.is_none() && !self.paused && !self.waiting && pos >= end {
            self.restart(Duration::ZERO, Some((g, area)));
        }
        if self.ended && self.shown.is_none() && self.next.is_none() {
            return (Shown::Failed("ffmpeg couldn't decode it"), None);
        }
        let mut wake = None;
        if !self.paused && !self.waiting {
            // The next frame, the end, or the clock's next second, whichever comes first.
            let next_second = Duration::from_secs(pos.as_secs().saturating_add(1));
            let due = self.next.as_ref().map_or(next_second, |(t, ..)| (*t).min(next_second));
            let due = if self.ended { due.min(end) } else { due };
            wake = now.checked_add(due.saturating_sub(pos));
        }
        if let Some((_, after)) = self.asked {
            wake = after.checked_add(RESIZE_DEBOUNCE).map(|t| wake.map_or(t, |w: Instant| w.min(t)));
        }
        let shown = self.shown.as_ref().map_or(Shown::Loading(None), |(_, p)| Shown::Ready(p));
        (shown, wake)
    }

    /// Take the frames that have come: show the next one if it's due, and keep the one
    /// after. One at a time, never skipping one (the decoder already drops those it's late
    /// for): kitty's frames take turns between two images, and the one showing mustn't be
    /// replaced.
    fn take_frames(&mut self, now: Instant) {
        let mut took = false;
        loop {
            if self.next.is_none() {
                let Some(d) = &self.decoder else { return };
                // Waiting for a frame, said before looking: one sent meanwhile wakes the viewer.
                d.starving.store(true, Ordering::SeqCst);
                match d.rx.try_recv() {
                    Ok(Frame::Picture(t, n, p)) => {
                        d.starving.store(false, Ordering::SeqCst);
                        self.next = Some((t, n, p));
                    }
                    Ok(Frame::End) | Err(TryRecvError::Disconnected) => {
                        self.ended = true;
                        self.decoder = None;
                        return;
                    }
                    Err(TryRecvError::Empty) => return,
                }
            }
            let Some((t, ..)) = &self.next else { return };
            let t = *t;
            if took {
                return;
            }
            if self.waiting {
                // The first frame after a start: the clock (and the sound) start with it.
                self.waiting = false;
                let mut c = lock(&self.clock);
                *c = Clock { at: t, since: (!self.paused).then_some(now) };
                drop(c);
                if !self.paused {
                    self.start_sound(t);
                }
            } else if t > lock(&self.clock).now(now) {
                return;
            }
            if let Some((t, n, p)) = self.next.take() {
                self.last = self.decoder.as_ref().map(|d| (d.geometry, n));
                self.shown = Some((t, p));
            }
            took = true;
        }
    }

    /// Decode from `at` on (for `g`, if it's known; else for the size last decoded for).
    fn restart(&mut self, at: Duration, g: Option<(Geometry, Size)>) {
        self.sound = None;
        self.decoder = None;
        self.next = None;
        self.ended = false;
        self.waiting = true;
        *lock(&self.clock) = Clock { at, since: None };
        let Some((g, area)) = g else { return };
        // The same cells as the frame on screen (a seek, the loop, a move while zoomed): the
        // new frames cover it as they are, and number on.
        let first = match self.last {
            Some((was, n)) if (was.cells, was.padded) == (g.cells, g.padded) => n.wrapping_add(1),
            _ => 0,
        };
        match self.spawn((at, first), g, area) {
            Ok(d) => self.decoder = Some(d),
            Err(_) => self.ended = true,
        }
    }

    fn spawn(&self, (from, first): (Duration, u64), g: Geometry, area: Size) -> std::io::Result<Decoder> {
        let mut child = crate::quiet_command("ffmpeg", decoder_args(&self.path, from, g)).stdout(Stdio::piped()).spawn()?;
        let out = child.stdout.take();
        let ffmpeg = Process(child);
        let out = out.ok_or_else(|| std::io::Error::other("no pipe from ffmpeg"))?;
        let (tx, rx) = sync_channel(AHEAD);
        let starving = Arc::new(AtomicBool::new(true));
        let (clock, encode, wake, s) = (self.clock.clone(), self.encode.clone(), self.wake.clone(), starving.clone());
        std::thread::spawn(move || {
            let _ = crate::guard::catching(|| pump(out, g, (from, first), &clock, &encode, &Sink { tx: &tx, starving: &s, wake: &*wake }));
        });
        Ok(Decoder { rx, starving, geometry: g, area, ffmpeg })
    }

    fn start_sound(&mut self, at: Duration) {
        self.sound = None;
        if self.muted || !self.probe.audio {
            return;
        }
        let Some(program) = sound_program() else { return };
        self.sound = crate::quiet_command(program, sound_args(program, &self.path, at)).spawn().ok().map(Process);
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_ffprobe_says() {
        let json = r#"{"streams":[{"codec_type":"video","width":640,"height":360,"sample_aspect_ratio":"1:1"},{"codec_type":"audio"}],"format":{"duration":"8.008000"}}"#;
        assert_eq!(parse_probe(json), Some(Probe { width: 640, height: 360, duration: Some(Duration::from_millis(8008)), audio: true }));
        // Filmed upright: stored on its side, shown turned.
        let json = r#"{"streams":[{"codec_type":"video","width":480,"height":640,"side_data_list":[{"rotation":-90}]}],"format":{"duration":"6.000000"}}"#;
        assert_eq!(parse_probe(json), Some(Probe { width: 640, height: 480, duration: Some(Duration::from_secs(6)), audio: false }));
        // Anamorphic: wide pixels.
        let json = r#"{"streams":[{"codec_type":"video","width":720,"height":576,"sample_aspect_ratio":"16:11"}],"format":{}}"#;
        assert_eq!(parse_probe(json), Some(Probe { width: 1047, height: 576, duration: None, audio: false }));
        // No picture, or one claiming an absurd size: nothing to play.
        assert_eq!(parse_probe(r#"{"streams":[{"codec_type":"audio"}],"format":{}}"#), None);
        assert_eq!(parse_probe(r#"{"streams":[{"codec_type":"video","width":100000,"height":10}]}"#), None);
        assert_eq!(parse_probe("not json"), None);
    }

    #[test]
    fn the_programs_get_the_file_as_one_argument_and_read_nothing_else() {
        let path = Path::new("/c/my clip; rm -rf ~.webm");
        let g = geometry((640, 360), Size::new(80, 24), (10, 20), false, crate::images::Crop::FIT);
        let args: Vec<String> = decoder_args(path, Duration::from_millis(2500), g).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let at = |k: &str| args.iter().position(|a| a == k).map(|i| args[i + 1].as_str());
        assert_eq!(at("-i"), Some("/c/my clip; rm -rf ~.webm"));
        assert_eq!(at("-ss"), Some("2.500"));
        assert_eq!(at("-protocol_whitelist"), Some("file"));
        assert_eq!(at("-format_whitelist"), Some(FORMATS));
        assert_eq!(at("-vf"), Some("fps=17,scale=800:450:flags=bilinear,pad=800:460:0:0:color=black@0,format=rgba"));
        assert_eq!(args.last().map(String::as_str), Some("pipe:1"));
        for program in ["ffplay", "mpv"] {
            let args = sound_args(program, path, Duration::from_secs(3));
            assert_eq!(args.last().map(|a| a.to_string_lossy().into_owned()), Some(path.to_string_lossy().into_owned()), "{program}");
            assert!(args.iter().any(|a| a == "3.000" || a == "--start=3.000"), "{program}: {args:?}");
        }
        let probe: Vec<_> = probe_args(path);
        assert_eq!(probe.last().map(|a| a.to_string_lossy().into_owned()).as_deref(), Some("/c/my clip; rm -rf ~.webm"));
    }

    #[test]
    fn a_video_fits_the_viewer_in_whole_cells() {
        // Fitted to the area's height, padded to whole cells; 20 frames a second.
        let g = geometry((1080, 1920), Size::new(100, 30), (10, 20), false, crate::images::Crop::FIT);
        assert_eq!((g.scaled, g.cells, g.padded, g.fps), ((338, 600), Size::new(34, 30), (340, 600), 20));
        // Smaller than the area: scaled up to fill it, as players do (fewer frames, as they're
        // bigger).
        let g = geometry((320, 240), Size::new(100, 30), (10, 20), false, crate::images::Crop::FIT);
        assert_eq!((g.scaled, g.cells, g.padded, g.fps), ((800, 600), Size::new(80, 30), (800, 600), 13));
        let g = geometry((320, 240), Size::new(100, 30), (10, 20), true, crate::images::Crop::FIT);
        assert_eq!(g.cells, Size::new(80, 30));
        // Fitted to the area's width (1000 x 600 pixels): fewer frames, as each is bigger.
        let g = geometry((1920, 1080), Size::new(100, 30), (10, 20), false, crate::images::Crop::FIT);
        assert_eq!((g.scaled, g.cells, g.padded, g.fps), ((1000, 563), Size::new(100, 29), (1000, 580), 10));
        // A big area: smaller too, to stay within what a second's frames may take.
        let g = geometry((1920, 1080), Size::new(300, 80), (10, 20), false, crate::images::Crop::FIT);
        let bytes = u64::from(g.padded.0 * g.padded.1 * 4 * g.fps);
        assert!(g.fps == 10 && g.scaled.0 < 1920 && bytes <= PIXEL_BYTES_PER_SECOND * 21 / 20, "{g:?}");
        // Half-blocks: a cell's width, half its height, padded to whole cells.
        let g = geometry((1920, 1080), Size::new(100, 30), (10, 20), true, crate::images::Crop::FIT);
        assert_eq!(g.padded, (u32::from(g.cells.width), u32::from(g.cells.height) * 2));
        assert!(g.scaled.0 <= g.padded.0 && g.scaled.1 <= g.padded.1 && g.cells.width == 100, "{g:?}");
    }

    #[test]
    fn zoomed_in_the_part_is_cut_out_and_fills_the_area() {
        // 200%, in the middle: the middle half each way, filling the area as the whole did.
        let zoom = crate::images::Crop::FIT.zoomed(true).zoomed(true);
        let g = geometry((640, 360), Size::new(64, 18), (10, 20), true, zoom);
        assert_eq!((g.part, g.cells), (Some([250, 250, 500, 500]), Size::new(64, 18)));
        let args: Vec<String> = decoder_args(Path::new("/v"), Duration::ZERO, g).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.iter().any(|a| a.starts_with("fps=20,crop=iw*0.5:ih*0.5:iw*0.25:ih*0.25,scale=")), "{args:?}");
        // Moved to the left edge: the part from there.
        let g = geometry((640, 360), Size::new(64, 18), (10, 20), true, zoom.moved(-4, 0, (500, 500)));
        assert_eq!(g.part.map(|p| p[0]), Some(0));
    }

    /// Frames in a stream as ffmpeg writes them: `n` frames of `w` x `h`, each one a color
    /// from `color(i)`.
    fn stream(w: u32, h: u32, n: u8, color: impl Fn(u8) -> u8) -> Vec<u8> {
        (0..n).flat_map(|i| std::iter::repeat_n(color(i), (w * h * 4) as usize)).collect()
    }

    fn pump_all(bytes: &[u8], g: Geometry, clock: Clock) -> Vec<Frame> {
        let (tx, rx) = sync_channel(64);
        let encode: Encode = Arc::new(|img, cells, _| crate::images::halfblocks_of(img, cells));
        pump(bytes, g, (Duration::from_secs(1), 0), &Mutex::new(clock), &encode, &Sink { tx: &tx, starving: &AtomicBool::new(false), wake: &|| {} });
        drop(tx);
        rx.into_iter().collect()
    }

    #[test]
    fn frames_come_with_their_times_and_repeats_and_late_ones_are_dropped() {
        let g = Geometry { part: None, scaled: (4, 4), padded: (4, 4), cells: Size::new(4, 2), fps: 20 };
        let stopped = Clock { at: Duration::ZERO, since: None };
        // Frames 0 and 1 the same, then 2, then 3: three pictures and the end.
        let frames = pump_all(&stream(4, 4, 4, |i| if i < 2 { 0 } else { i }), g, stopped);
        let times: Vec<Option<u64>> = frames.iter().map(|f| if let Frame::Picture(t, ..) = f { Some(t.as_millis() as u64) } else { None }).collect();
        assert_eq!(times, [Some(1000), Some(1100), Some(1150), None]);
        // A truncated stream: the whole frames, then the end.
        let mut bytes = stream(4, 4, 2, |i| i);
        bytes.truncate(bytes.len() - 7);
        let frames = pump_all(&bytes, g, stopped);
        assert!(matches!(frames.as_slice(), [Frame::Picture(..), Frame::End]));
        // Played up to 1.12s already: the frames before it are dropped unencoded.
        let behind = Clock { at: Duration::from_millis(1120), since: Some(Instant::now()) };
        let frames = pump_all(&stream(4, 4, 6, |i| i), g, behind);
        let first = frames.iter().find_map(|f| if let Frame::Picture(t, ..) = f { Some(*t) } else { None });
        assert!(first.is_some_and(|t| t >= Duration::from_millis(1100)), "{first:?}");
        // Nothing at all: just the end.
        assert!(matches!(pump_all(&[], g, stopped).as_slice(), [Frame::End]));
    }

    #[test]
    fn a_frame_never_leaves_the_screen_before_the_next_is_there() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(40, 40, image::Rgba([200, 30, 30, 255])));
        let cells = Size::new(4, 2);
        // Sixel: only the first frame clears the area first; the next ones cover it.
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize((10, 20).into());
        picker.set_protocol_type(ProtocolType::Sixel);
        let encode = encoder(&picker);
        let data = |n| match encode(img.clone(), cells, n).unwrap() {
            Protocol::Sixel(s) => s.data,
            _ => panic!("not sixel"),
        };
        assert!(data(0).starts_with(&clear(cells, false)) && data(0).contains("\x1bP"));
        assert!(data(1).starts_with("\x1bP") && data(2).starts_with("\x1bP"));
        // Kitty: frames take turns between two images (the number is the cells' color).
        picker.set_protocol_type(ProtocolType::Kitty);
        let encode = encoder(&picker);
        let image_of = |n| {
            let p = encode(img.clone(), cells, n).unwrap();
            let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 4, 2));
            ratatui::widgets::Widget::render(ratatui_image::Image::new(&p), buf.area, &mut buf);
            buf[(1, 1)].fg
        };
        assert!(image_of(0) != image_of(1) && image_of(0) == image_of(2) && image_of(1) == image_of(3));
    }

    #[test]
    fn the_clock_starts_and_stops() {
        let t0 = Instant::now();
        let mut c = Clock { at: Duration::from_secs(2), since: Some(t0) };
        assert_eq!(c.now(t0 + Duration::from_secs(3)), Duration::from_secs(5));
        c.stop(t0 + Duration::from_secs(1));
        assert_eq!(c.now(t0 + Duration::from_secs(10)), Duration::from_secs(3));
    }

    #[test]
    fn a_failure_is_shown_and_never_needs_a_redraw() {
        let mut p = Player::failed("https://x/a.webm", "ffprobe: Invalid data found when processing input");
        let (shown, wake) = p.frame((Size::new(80, 24), crate::images::Crop::FIT), (10, 20), true, Instant::now());
        assert!(matches!(shown, Shown::Failed(w) if w.contains("Invalid data")));
        assert!(wake.is_none());
        assert!(!p.toggle_pause() && p.position().is_none());
    }
}
