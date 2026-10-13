//! The whole app, timed: the release binary in tmux against a local vichan site with a big
//! board and a long thread (generated, the same every run), taken through a session of
//! everyday steps. For each step: how long until the screen shows what it should and ck
//! stops drawing, the CPU ck used (all its threads), the frames it drew, and what it wrote
//! to the terminal; then its memory. Nothing reaches the network.
//!
//!   cargo build --release && cargo test --release -- --ignored bench_app --nocapture --test-threads=1
//!
//! `BENCH_RUNS` (default 3): runs, each step's median reported. `BENCH_POSTS` (default
//! 1500): the long thread's posts. `BENCH_PROFILE=<file>`: one run under samply (`cargo
//! install samply`), its profile saved to the file (`samply load <file>` opens it).
//! Needs tmux.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::e2e::{Tmux, command, release_ck, scratch};
use crate::fuzz::Rng;

/// The long thread's number; the board's other threads come after it.
const BIG: u64 = 1000;
const THREADS: u64 = 150;

/// Bodies for posts: the markup samples (code, spoilers, links, ...) and plain talk.
fn bodies() -> Vec<String> {
    let samples = crate::backend::fixture("markup_samples.json");
    let mut out: Vec<String> = samples.as_object().into_iter().flatten().filter_map(|(_, v)| v.as_str().map(str::to_string)).collect();
    let words = "the a it this that linux kernel rust thread anon is was just why how what compile build boot driver window manager terminal font config install package update broken works fine never always really based".split(' ').collect::<Vec<_>>();
    let mut rng = Rng::new(7);
    for _ in 0..60 {
        let lines = 1 + rng.below(6);
        let text: Vec<String> = (0..lines)
            .map(|_| {
                let line = (0..3 + rng.below(25)).map(|_| *rng.pick(&words)).collect::<Vec<_>>().join(" ");
                if rng.chance(15) { format!("<span class=\"quote\">&gt;{line}</span>") } else { line }
            })
            .collect();
        out.push(text.join("<br>"));
    }
    out.push("see https://github.com/rust-lang/rust/issues/12345 and https://example.org/a/b?c=d".into());
    out
}

/// Thread `no` of `n` posts, as vichan's `res/{no}.json`: replies quote earlier posts (the
/// OP and recent ones most), about a third have a file.
fn thread(no: u64, n: u64, bodies: &[String]) -> Value {
    let mut rng = Rng::new(no);
    let posts: Vec<Value> = (0..n)
        .map(|i| {
            let pno = if i == 0 { no } else { no * 10_000 + i };
            let mut com = String::new();
            for _ in 0..if i == 0 { 0 } else { rng.below(3) } {
                let to = match rng.below(4) {
                    0 => no,
                    1 => no * 10_000 + 1 + rng.below(i as usize) as u64,
                    _ => no * 10_000 + i.saturating_sub(1 + rng.below(8) as u64).max(1),
                };
                let _ = write!(com, "<a href=\"/g/res/{no}.html#{to}\" onclick=\"highlightReply('{to}');\">&gt;&gt;{to}</a><br>");
            }
            com.push_str(rng.pick(bodies));
            let mut p = json!({ "no": pno, "resto": if i == 0 { 0 } else { no }, "time": 1_790_000_000 + i * 30, "name": "Anonymous", "com": com });
            if i == 0 {
                p["sub"] = json!(format!("Thread {no}: the general"));
                p["replies"] = json!(n - 1);
                p["images"] = json!(n / 3);
            }
            if i == 0 || rng.chance(33) {
                p["tim"] = json!(format!("{}{i}", 1_790_000_000_000u64 + no));
                p["ext"] = json!(".jpg");
                p["filename"] = json!(format!("image{i}"));
                p["w"] = json!(1600);
                p["h"] = json!(1200);
                p["tn_w"] = json!(250);
                p["tn_h"] = json!(188);
                p["fsize"] = json!(250_000);
            }
            p
        })
        .collect();
    json!({ "posts": posts })
}

/// The board's catalog: its threads' OPs, with their last few replies.
fn catalog(threads: &HashMap<u64, Value>) -> Value {
    let mut ops: Vec<Value> = (0..THREADS)
        .filter_map(|i| {
            let posts = threads.get(&(BIG + i))?["posts"].as_array()?;
            let mut op = posts.first()?.clone();
            op["last_replies"] = json!(posts.iter().rev().take(3).rev().cloned().collect::<Vec<_>>());
            op["last_modified"] = json!(1_790_000_000 + i);
            Some(op)
        })
        .collect();
    ops.truncate(THREADS as usize);
    json!([{ "page": 0, "threads": ops }])
}

/// A photo-like JPEG (encoders work harder on detail than on flat color).
fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x ^ y) as u8, (x * 3 + y) as u8, (y * 7) as u8])));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

struct Site {
    threads: HashMap<u64, Value>,
    catalog: Vec<u8>,
    thumb: Vec<u8>,
    full: Vec<u8>,
    /// Paths asked for that it doesn't have.
    missing: Mutex<Vec<String>>,
}

const MODIFIED: &str = "Thu, 01 Oct 2026 00:00:00 GMT";

/// The site on a local port.
fn serve(posts: u64) -> (u16, Arc<Site>) {
    let bodies = bodies();
    let threads: HashMap<u64, Value> = (0..THREADS).map(|i| (BIG + i, thread(BIG + i, if i == 0 { posts } else { 20 + (i * 7) % 120 }, &bodies))).collect();
    let catalog = serde_json::to_vec(&catalog(&threads)).unwrap();
    let site = Arc::new(Site { threads, catalog, thumb: jpeg(250, 188), full: jpeg(1600, 1200), missing: Mutex::new(Vec::new()) });
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let s = site.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let s = s.clone();
            std::thread::spawn(move || respond(stream, &s));
        }
    });
    (port, site)
}

fn respond(mut stream: TcpStream, site: &Site) {
    let mut request = Vec::new();
    let mut buf = [0; 4096];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&request);
    let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
    let since = text.lines().any(|l| l.to_ascii_lowercase().starts_with("if-modified-since:"));
    let json = |v: &Value| serde_json::to_vec(v).unwrap();
    let (status, kind, body) = if path.contains("/thumb/") {
        (200, "image/jpeg", site.thumb.clone())
    } else if path.contains("/src/") {
        (200, "image/jpeg", site.full.clone())
    } else if since {
        (304, "application/json", Vec::new())
    } else if path == "/g/catalog.json" {
        (200, "application/json", site.catalog.clone())
    } else if let Some(t) = path.strip_prefix("/g/res/").and_then(|p| p.strip_suffix(".json")).and_then(|n| n.parse().ok()).and_then(|n: u64| site.threads.get(&n)) {
        (200, "application/json", json(t))
    } else {
        crate::http::lock(&site.missing).push(path);
        (404, "text/plain", Vec::new())
    };
    let modified = if status == 200 && kind == "application/json" { format!("Last-Modified: {MODIFIED}\r\n") } else { String::new() };
    let head = format!("HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\n{modified}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}

/// ck's process: the process named ck under the window's shell (under samply, a level
/// further down).
fn ck_pid(t: &Tmux) -> Option<u32> {
    let mut pids = vec![t.run(&["display", "-p", "-t", "ck", "#{pane_pid}"]).trim().parse::<u32>().ok()?];
    for _ in 0..4 {
        let mut next = Vec::new();
        for pid in pids {
            for child in std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap_or_default().split_whitespace() {
                let child: u32 = child.parse().ok()?;
                if std::fs::read_to_string(format!("/proc/{child}/comm")).is_ok_and(|c| c.trim() == "ck") {
                    return Some(child);
                }
                next.push(child);
            }
        }
        pids = next;
    }
    None
}

/// CPU time of all of a process's threads, from the scheduler (to the nanosecond).
fn cpu(pid: u32) -> Duration {
    let tasks = std::fs::read_dir(format!("/proc/{pid}/task")).into_iter().flatten().flatten();
    let ns: u64 = tasks.filter_map(|t| std::fs::read_to_string(t.path().join("schedstat")).ok()?.split_whitespace().next()?.parse::<u64>().ok()).sum();
    Duration::from_nanos(ns)
}

/// `VmHWM` and `VmRSS`, in KB.
fn memory(pid: u32) -> (u64, u64) {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    let field = |name: &str| status.lines().find_map(|l| l.strip_prefix(name)?.trim().strip_suffix(" kB")?.trim().parse().ok()).unwrap_or(0);
    (field("VmHWM:"), field("VmRSS:"))
}

#[derive(Clone, Copy, Default)]
struct Cost {
    wall: Duration,
    cpu: Duration,
    frames: usize,
    bytes: u64,
}

struct Session {
    tmux: Tmux,
    dir: PathBuf,
    pid: u32,
    costs: Vec<(&'static str, Cost)>,
}

impl Session {
    fn frame(&self) -> String {
        std::fs::read_to_string(self.dir.join("frame.txt")).unwrap_or_default()
    }

    fn frames(&self) -> usize {
        std::fs::read_to_string(self.dir.join("input.log")).unwrap_or_default().lines().filter(|l| l.ends_with("  frame")).count()
    }

    fn bytes(&self) -> u64 {
        std::fs::metadata(self.dir.join("tty.out")).map_or(0, |m| m.len())
    }

    /// `act`, then wait for the frame to satisfy `done` and for ck to stop drawing (no frame
    /// for `quiet`); noted as `label`.
    fn step(&mut self, label: &'static str, quiet: Duration, act: impl FnOnce(&Tmux), done: impl Fn(&str) -> bool) {
        let (cpu0, frames0, bytes0) = (cpu(self.pid), self.frames(), self.bytes());
        let start = Instant::now();
        act(&self.tmux);
        let deadline = start + Duration::from_secs(30);
        while !done(&self.frame()) {
            if Instant::now() > deadline {
                let log = std::fs::read_to_string(self.dir.join("input.log")).unwrap_or_default();
                let _ = std::fs::write(std::env::temp_dir().join("ck-bench-input.log"), &log);
                panic!("{label}: never got there; the frame:\n{}", self.frame());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut wall = start.elapsed();
        // Then until it stops drawing (images coming in, a status fading, ...).
        let (mut seen, mut since) = (self.frames(), Instant::now());
        while since.elapsed() < quiet && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            let n = self.frames();
            if n != seen {
                (seen, since) = (n, Instant::now());
                wall = start.elapsed();
            }
        }
        let cost = Cost { wall, cpu: cpu(self.pid) - cpu0, frames: self.frames() - frames0, bytes: self.bytes() - bytes0 };
        self.costs.push((label, cost));
    }

    /// Nothing done for `secs`: what ck does by itself (redraws, refreshes).
    fn idle(&mut self, label: &'static str, secs: u64) {
        let (cpu0, frames0, bytes0) = (cpu(self.pid), self.frames(), self.bytes());
        let start = Instant::now();
        std::thread::sleep(Duration::from_secs(secs));
        let cost = Cost { wall: start.elapsed(), cpu: cpu(self.pid) - cpu0, frames: self.frames() - frames0, bytes: self.bytes() - bytes0 };
        self.costs.push((label, cost));
    }
}

/// Where the footer's ruler is (`301/1500  20%`): the post, of how many.
fn ruler(frame: &str) -> Option<(u64, u64)> {
    let last = frame.lines().last()?;
    let (n, of) = last.split_whitespace().rev().find_map(|w| w.split_once('/'))?;
    Some((n.parse().ok()?, of.parse().ok()?))
}

/// The ruler at `n` of `of`.
fn at(n: u64, of: u64) -> impl Fn(&str) -> bool {
    move |f: &str| ruler(f) == Some((n, of))
}

/// The ruler at `n` or further, of `of` (a key can scroll a tall post rather than move).
fn past(n: u64, of: u64) -> impl Fn(&str) -> bool {
    move |f: &str| ruler(f).is_some_and(|(m, o)| o == of && m >= n)
}

/// One run of the session; its steps' costs and ck's memory at the end.
fn run(ck: &Path, port: u16, posts: u64, profile: Option<&Path>) -> (Vec<(&'static str, Cost)>, (u64, u64)) {
    let dir = tempfile::tempdir().unwrap();
    let config = format!(
        "notify = \"off\"\nrestore_session = false\nrefresh_thread_secs = 10\ndownload_dir = \"{}\"\n\n[[site]]\nname = \"vichan\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:{port}\"\nboards = [\"g\"]\n",
        dir.path().join("downloads").display()
    );
    let mut env = scratch(dir.path(), &config);
    env.push(("CK_INPUT_LOG".into(), dir.path().join("input.log").display().to_string()));
    let tmux = Tmux { socket: format!("ck-bench-{}", std::process::id()) };
    let mut cmd = command(ck, &env, &[]);
    if let Some(p) = profile {
        cmd = format!("samply record --save-only --unstable-presymbolicate -o '{}' -- {cmd}", p.display());
    }
    let start = Instant::now();
    tmux.launch(&format!("{cmd}; echo CK_EXIT=$?; sleep 600"), (120, 40));
    tmux.run(&["set", "-s", "escape-time", "0"]);
    tmux.run(&["pipe-pane", "-o", "-t", "ck", &format!("cat >> '{}'", dir.path().join("tty.out").display())]);
    let mut pid = None;
    while pid.is_none() && start.elapsed() < Duration::from_secs(10) {
        pid = ck_pid(&tmux);
    }
    let mut s = Session { tmux, dir: dir.path().to_path_buf(), pid: pid.expect("ck didn't start"), costs: Vec::new() };
    while !s.frame().contains("vichan") {
        assert!(start.elapsed() < Duration::from_secs(10), "ck didn't draw:\n{}", s.tmux.screen());
        std::thread::sleep(Duration::from_millis(1));
    }
    s.costs.push(("start: the first frame", Cost { wall: start.elapsed(), cpu: cpu(s.pid), frames: s.frames(), bytes: s.bytes() }));
    s.idle("idle on the home screen, 3s", 3);

    let quiet = Duration::from_millis(400);
    let first = format!("Thread {BIG}: the general");
    s.step("open the board's catalog (150 threads)", quiet, |t| {
        t.keys(&[":"]);
        t.text("vichan/g");
        t.keys(&["Enter"]);
    }, |f| f.contains(&first));
    s.step("open the long thread", quiet, |t| t.keys(&["Enter"]), at(1, posts));
    s.step("j x300, one at a time as typed", quiet, |t| t.keys(&["-N", "300", "j"]), past(290, posts));
    s.step("G: to the end", quiet, |t| t.keys(&["G"]), at(posts, posts));
    s.step("gg: back to the top", quiet, |t| t.keys(&["g", "g"]), at(1, posts));
    s.step("ctrl-d x40", quiet, |t| t.keys(&["-N", "40", "C-d"]), |f| !at(1, posts)(f));
    s.step("search: /linux", quiet, |t| {
        t.keys(&["/"]);
        t.text("linux");
        t.keys(&["Enter"]);
    }, |f| f.contains("linux") && f.lines().last().is_some_and(|l| l.contains("match")));
    s.step("n x30: next match", quiet, |t| t.keys(&["-N", "30", "n"]), |_| true);
    s.step("e x20: replies under posts, open and shut", quiet, |t| {
        t.keys(&["Escape"]);
        t.keys(&["g", "g"]);
        t.keys(&["-N", "20", "e"]);
    }, |_| true);
    s.step("the gallery (V)", quiet, |t| t.keys(&["V"]), |f| f.contains("files"));
    s.step("the gallery: 60 files across", quiet, |t| t.keys(&["-N", "60", "l"]), |_| true);
    s.step("the image viewer, then 10 files on", quiet, |t| {
        t.keys(&["Enter"]);
        t.keys(&["-N", "10", "l"]);
    }, |_| true);
    s.step("back to the thread", quiet, |t| t.keys(&["q", "q"]), past(1, posts));
    s.idle("idle in the thread, 12s (one refresh)", 12);
    s.step("resize to 200x60 and back", quiet, |t| {
        t.run(&["resize-window", "-t", "ck", "-x", "200", "-y", "60"]);
        t.run(&["resize-window", "-t", "ck", "-x", "120", "-y", "40"]);
    }, |_| true);
    s.step("back to the catalog", quiet, |t| t.keys(&["h"]), |f| f.contains(&first));
    s.step("catalog: j x100", quiet, |t| t.keys(&["-N", "100", "j"]), |_| true);
    let mem = memory(s.pid);
    let quit = Instant::now();
    s.tmux.keys(&["C-c"]);
    while !s.tmux.screen().contains("CK_EXIT=") {
        assert!(quit.elapsed() < Duration::from_secs(20), "ck didn't quit");
        std::thread::sleep(Duration::from_millis(2));
    }
    s.costs.push(("quit (saving)", Cost { wall: quit.elapsed(), ..Cost::default() }));
    assert!(s.tmux.screen().contains("CK_EXIT=0"), "ck didn't quit cleanly:\n{}", s.tmux.screen());
    if profile.is_some() {
        // samply writes the profile after ck exits.
        let wrote = Instant::now();
        while !profile.is_some_and(Path::exists) && wrote.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    // The last run's log, to see what ck did when.
    let _ = std::fs::copy(dir.path().join("input.log"), std::env::temp_dir().join("ck-bench-input.log"));
    (s.costs, mem)
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

#[test]
#[ignore = "benchmark: runs the release binary in tmux for a few minutes"]
fn bench_app() {
    let ck = release_ck();
    if std::process::Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("bench_app: no tmux, skipped");
        return;
    }
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let posts = env("BENCH_POSTS", 1500);
    let profile = std::env::var_os("BENCH_PROFILE").map(PathBuf::from);
    let runs = if profile.is_some() { 1 } else { env("BENCH_RUNS", 3) };
    let (port, site) = serve(posts);
    let results: Vec<_> = (0..runs).map(|_| run(&ck, port, posts, profile.as_deref())).collect();
    let missing = crate::http::lock(&site.missing).clone();
    if !missing.is_empty() {
        eprintln!("(the site had no {missing:?})");
    }
    eprintln!("\n== the app, {posts}-post thread, median of {runs} ==");
    eprintln!("{:<46} {:>9} {:>9} {:>7} {:>9}", "", "wall", "cpu", "frames", "out KB");
    for (i, (label, _)) in results[0].0.iter().enumerate() {
        let of = |f: fn(&Cost) -> Duration| median(results.iter().map(|r| f(&r.0[i].1)).collect());
        let frames = { let mut v: Vec<usize> = results.iter().map(|r| r.0[i].1.frames).collect(); v.sort(); v[v.len() / 2] };
        let bytes = { let mut v: Vec<u64> = results.iter().map(|r| r.0[i].1.bytes).collect(); v.sort(); v[v.len() / 2] };
        eprintln!("{label:<46} {:>9.1?} {:>9.1?} {frames:>7} {:>9}", of(|c| c.wall), of(|c| c.cpu), bytes >> 10);
    }
    let total = median(results.iter().map(|r| r.0.iter().map(|(_, c)| c.cpu).sum()).collect());
    let (peak, rss) = results.iter().map(|r| r.1).max().unwrap_or_default();
    eprintln!("{:<46} {:>9} {total:>9.1?}", "all", "");
    eprintln!("memory: peak {} MB, at the end {} MB", peak >> 10, rss >> 10);
    if let Some(p) = profile {
        eprintln!("profile: {} (samply load it)", p.display());
    }
}
