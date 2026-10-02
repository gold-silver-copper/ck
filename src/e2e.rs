//! End to end: the release binary in tmux, against local servers that answer like the real
//! engines (from the fixtures, mangled now and then, slowly or not at all), driven by random
//! keys, text, resizes and restarts for a while. It must stay up, never print a panic, quit
//! cleanly, keep its memory flat and leave data that loads.
//!
//!   cargo build --release && cargo test -- --ignored e2e_soak --nocapture
//!
//! `E2E_SECS` (default 60) and `FUZZ_SEED`. Needs tmux; uses its own tmux server, scratch
//! config, data, cache and download directories, and `CK_NO_EXTERNAL` (no browser,
//! clipboard or notifications). Every site is on 127.0.0.1, and URLs in answers are
//! rewritten to point there: nothing reaches the network.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::SiteKind;
use crate::fuzz::{self, Rng};

/// A local server answering like an engine of `kind`; returns its port.
fn serve(kind: SiteKind, seed: u64) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let answer = fuzz::fake_site(kind, seed, 25);
    let rng = Arc::new(std::sync::Mutex::new(Rng::new(seed ^ 1)));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (answer, rng) = (answer.clone(), rng.clone());
            std::thread::spawn(move || respond(stream, port, &*answer, &rng));
        }
    });
    port
}

fn respond(mut stream: TcpStream, port: u16, answer: &(dyn Fn(&str) -> crate::http::Raw + Send + Sync), rng: &std::sync::Mutex<Rng>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut buf = [0; 4096];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let path = String::from_utf8_lossy(&request).split_whitespace().nth(1).unwrap_or("/").to_string();
    let (delay, drop, image_index, corrupt) = {
        let mut r = crate::http::lock(rng);
        (Duration::from_millis(*r.pick(&[0, 0, 0, 50, 300, 1500]) as u64), r.chance(2), r.below(10), r.chance(15))
    };
    std::thread::sleep(delay);
    if drop {
        return;
    }
    let is_image = [".png", ".jpg", ".jpeg", ".gif", ".webp", "/thumb/", "/src/", "/ext/"].iter().any(|k| path.contains(k)) && !path.ends_with(".json");
    let (status, kind, body) = if is_image {
        let mut bytes = fuzz::IMAGES[image_index % fuzz::IMAGES.len()].clone();
        if corrupt {
            bytes.truncate(bytes.len() / 2);
        }
        (200, "image/png", bytes)
    } else {
        let raw = answer(&format!("http://127.0.0.1:{port}{path}"));
        (raw.status, "application/json", local_urls(&raw.body, port).into_bytes())
    };
    let head = format!("HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}

/// Every absolute URL in an answer, pointed at this server instead.
fn local_urls(body: &str, port: u16) -> String {
    let local = format!("http://127.0.0.1:{port}/ext/");
    let escaped = local.replace('/', "\\/");
    let mut out = body.to_string();
    for (from, to) in [("https://", &local), ("http://", &local), ("https:\\/\\/", &escaped), ("http:\\/\\/", &escaped)] {
        let mut s = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some((before, after)) = rest.split_once(from) {
            s.push_str(before);
            s.push_str(if after.starts_with("127.0.0.1") { from } else { to });
            rest = after;
        }
        s.push_str(rest);
        out = s;
    }
    out
}

/// One tmux server of our own, with ck in a window that outlives it.
struct Tmux {
    socket: String,
}

impl Tmux {
    fn run(&self, args: &[&str]) -> String {
        let out = Command::new("tmux").arg("-L").arg(&self.socket).args(args).output().unwrap();
        if !out.status.success() && args.first() != Some(&"kill-server") {
            eprintln!("tmux {args:?}: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn start(&self, ck: &Path, env: &[(String, String)], (w, h): (u16, u16)) {
        let vars: String = env.iter().map(|(k, v)| format!("{k}='{v}' ")).collect();
        // The window stays after ck exits, showing how it went.
        let cmd = format!("env {vars} '{}'; echo CK_EXIT=$?; sleep 600", ck.display());
        let _ = self.run(&["kill-server"]);
        self.run(&["new-session", "-d", "-s", "ck", "-x", &w.to_string(), "-y", &h.to_string(), &cmd]);
    }

    fn screen(&self) -> String {
        self.run(&["capture-pane", "-p", "-t", "ck"])
    }

    fn keys(&self, keys: &[&str]) {
        let mut args = vec!["send-keys", "-t", "ck"];
        args.extend_from_slice(keys);
        self.run(&args);
    }

    fn text(&self, text: &str) {
        self.run(&["send-keys", "-t", "ck", "-l", text]);
    }

    /// ck's process (the shell in the window is its parent).
    fn ck_pid(&self) -> Option<u32> {
        let shell = self.run(&["display", "-p", "-t", "ck", "#{pane_pid}"]);
        let out = Command::new("pgrep").args(["-P", shell.trim()]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).lines().next()?.trim().parse().ok()
    }
}

impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = self.run(&["kill-server"]);
    }
}

/// The first row where the screen and ck's last frame differ, when neither is changing
/// (a frame drawn but not yet written out isn't a difference): it must hold three times.
fn mismatch(t: &Tmux, frame: &Path) -> Option<(usize, String, String)> {
    let read = || std::fs::read_to_string(frame).unwrap_or_default();
    let rows = |s: &str| s.lines().map(|l| l.trim_end().to_string()).collect::<Vec<_>>();
    let mut found = None;
    for _ in 0..3 {
        let (mut screen, mut meant) = (t.screen(), read());
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(150));
            let (s, m) = (t.screen(), read());
            if s == screen && m == meant {
                break;
            }
            (screen, meant) = (s, m);
        }
        let (shown, meant) = (rows(&screen), rows(&meant));
        found = shown.iter().zip(&meant).enumerate().find(|(_, (a, b))| a != b).map(|(i, (a, b))| (i, a.clone(), b.clone()));
        found.as_ref()?;
    }
    found
}

/// The screen once ck has exited (it saves first), or after three seconds.
fn exited(t: &Tmux) -> String {
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        let s = t.screen();
        if s.contains("CK_EXIT=") {
            return s;
        }
    }
    t.screen()
}

fn rss_kb(pid: u32) -> Option<u64> {
    let out = Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Wait until ck answers keys, with help open. Keys typed while it asks the terminal about
/// images are swallowed, and keep it asking (ratatui-image restarts its timeout on every
/// byte), so the first one waits until it has drawn.
fn ready(t: &Tmux) {
    for _ in 0..50 {
        if t.screen().contains(" ck ") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for _ in 0..40 {
        t.keys(&["?"]);
        std::thread::sleep(Duration::from_millis(400));
        let s = t.screen();
        assert!(!s.contains("CK_EXIT="), "ck exited while starting:\n{s}");
        if s.contains("esc close") {
            t.keys(&["z"]);
            std::thread::sleep(Duration::from_millis(300));
            return;
        }
    }
    panic!("ck didn't respond:\n{}", t.screen());
}

const KEYS: &[&str] = &[
    "j", "j", "j", "k", "Enter", "Enter", "Escape", "h", "l", "g", "G", "Down", "Up", "PageDown", "PageUp", "Home", "End",
    "Tab", "BTab", "Space", "BSpace", "w", "s", "c", "v", "i", "b", "u", "U", "p", "n", "N", "S", "d", "D", "a", "x", "y",
    "Y", "O", "H", "Z", "R", "V", "E", "e", "m", "T", "*", "F", "f", "r", "F5", ",", "?", "C-w", "z", "1", "2", ".", "[",
    "]", "A", "Tab", "Tab", "a", "s", "d",
];

#[test]
#[ignore]
fn e2e_soak() {
    let ck = PathBuf::from(std::env::var("CK_BIN").unwrap_or_else(|_| format!("{}/target/release/ck", env!("CARGO_MANIFEST_DIR"))));
    assert!(ck.exists(), "no {} (cargo build --release first)", ck.display());
    if Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("e2e_soak: no tmux, skipped");
        return;
    }
    let secs: u64 = std::env::var("E2E_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let seed: u64 = std::env::var("FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
    });
    eprintln!("e2e_soak: seed {seed}, {secs}s");
    let mut rng = Rng::new(seed);
    let dir = tempfile::tempdir().unwrap();
    let sites = [
        ("vichan", SiteKind::Vichan, "boards = [\"g\", \"b\", \"tech\"]\narchive = \"fool\"\n"),
        ("lynx", SiteKind::Lynxchan, ""),
        ("fool", SiteKind::Foolfuuka, ""),
        ("js", SiteKind::Jschan, ""),
        ("mak", SiteKind::Makaba, ""),
    ];
    let mut config = format!(
        "notify = \"off\"\nrestore_session = true\nrefresh_thread_secs = 10\nrefresh_watched_secs = 60\ndownload_dir = \"{}\"\nfavorites = [\"vichan/g\", \"js/b\"]\n",
        dir.path().join("downloads").display()
    );
    for (name, kind, extra) in sites {
        let port = serve(kind, rng.next());
        let kind = format!("{kind:?}").to_lowercase();
        config.push_str(&format!("\n[[site]]\nname = \"{name}\"\nkind = \"{kind}\"\nurl = \"http://127.0.0.1:{port}\"\n{extra}"));
    }
    std::fs::create_dir_all(dir.path().join("config/ck")).unwrap();
    std::fs::write(dir.path().join("config/ck/config.toml"), &config).unwrap();
    let env: Vec<(String, String)> = [("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache")]
        .iter()
        .map(|(k, d)| (k.to_string(), dir.path().join(d).display().to_string()))
        .chain([("CK_NO_EXTERNAL".into(), "1".into()), ("COLORTERM".into(), "truecolor".into())])
        .chain([("CK_FRAME_DUMP".into(), dir.path().join("frame.txt").display().to_string())])
        .collect();
    let tmux = Tmux { socket: format!("ck-e2e-{}", std::process::id()) };
    tmux.start(&ck, &env, (110, 32));
    ready(&tmux);

    let start = Instant::now();
    let (mut sent, mut restarts, mut warm, mut peak) = (0u64, 0, None, 0u64);
    while start.elapsed() < Duration::from_secs(secs) {
        match rng.below(100) {
            0..80 => tmux.keys(&[*rng.pick(KEYS)]),
            80..86 => {
                // Text, only once an input is open (a stray key could do anything).
                tmux.keys(&[*rng.pick(&[":", "/", "f"])]);
                std::thread::sleep(Duration::from_millis(200));
                let s = tmux.screen();
                if s.lines().last().is_some_and(|l| l.contains("enter")) || s.contains("Search") {
                    tmux.text(rng.pick(&["vichan/g", "js/b/1", "fool/a", "lynx", "the", "mak/b", "nosuch/x", "日本", "saved", "saved", "watched"]));
                    tmux.keys(&["Enter"]);
                }
            }
            86..90 => {
                let (w, h) = *rng.pick(&[(110, 32), (60, 20), (200, 50), (30, 8), (80, 24)]);
                tmux.run(&["resize-window", "-t", "ck", "-x", &w.to_string(), "-y", &h.to_string()]);
            }
            90..91 => {
                // Quit and start again: it must exit cleanly and restore where it was.
                tmux.keys(&["Escape", "Escape", "Escape"]);
                tmux.keys(&["C-c"]);
                let s = exited(&tmux);
                assert!(s.contains("CK_EXIT=0"), "ck didn't quit cleanly (seed {seed}):\n{s}");
                tmux.start(&ck, &env, (110, 32));
                ready(&tmux);
                restarts += 1;
                warm = None;
            }
            _ => std::thread::sleep(Duration::from_millis(rng.below(1500) as u64)),
        }
        sent += 1;
        std::thread::sleep(Duration::from_millis(30));
        if sent % 25 == 0 {
            let s = tmux.screen();
            assert!(!s.contains("CK_EXIT="), "ck exited (seed {seed}, after {sent} steps):\n{s}");
            assert!(!s.contains("panicked"), "ck panicked (seed {seed}):\n{s}");
            // The screen is what ck drew: nothing left over, nothing out of place.
            if let Some((row, shown, meant)) = mismatch(&tmux, &dir.path().join("frame.txt")) {
                panic!("row {row} on screen isn't what ck drew (seed {seed}, step {sent}):\n  screen: {shown:?}\n  frame:  {meant:?}");
            }
            if std::env::var_os("E2E_SHOW").is_some() {
                eprintln!("--- step {sent}:\n{s}");
            }
            if let Some(kb) = tmux.ck_pid().and_then(rss_kb) {
                peak = peak.max(kb);
                // Memory after the first half minute of each run is the baseline.
                if warm.is_none() && start.elapsed() > Duration::from_secs(30) {
                    warm = Some(kb);
                }
                if let Some(w) = warm {
                    assert!(kb <= (w * 3).max(w + 200_000), "memory grew from {w} KB to {kb} KB (seed {seed})");
                }
            }
        }
    }
    tmux.keys(&["Escape", "Escape", "C-c"]);
    let s = exited(&tmux);
    assert!(s.contains("CK_EXIT=0"), "ck didn't quit cleanly at the end (seed {seed}):\n{s}");
    let (_, warnings) = crate::store::Store::load(Some(dir.path().join("data/ck")));
    assert!(warnings.is_empty(), "the data doesn't load cleanly: {warnings:?}");
    eprintln!("e2e_soak: {sent} steps, {restarts} restarts, peak {peak} KB, no failures");
}
