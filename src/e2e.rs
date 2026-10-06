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

use std::fmt::Write as _;
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

/// What every JSON answer says it was last modified (so ck asks `If-Modified-Since`).
const MODIFIED: &str = "Thu, 01 Oct 2026 00:00:00 GMT";

fn respond(mut stream: TcpStream, port: u16, answer: &(dyn Fn(&str, Option<&str>) -> crate::http::Raw + Send + Sync), rng: &std::sync::Mutex<Rng>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut buf = [0; 4096];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&request).to_string();
    let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
    let since = text.lines().any(|l| l.to_ascii_lowercase().starts_with("if-modified-since:") && l.contains(MODIFIED));
    let (delay, drop, image_index, corrupt, unchanged, cut) = {
        let mut r = crate::http::lock(rng);
        (Duration::from_millis(*r.pick(&[0, 0, 0, 50, 300, 1500]) as u64), r.chance(2), r.below(10), r.chance(15), r.chance(50), r.chance(2))
    };
    let long_post = crate::http::lock(rng).chance(30);
    std::thread::sleep(delay);
    if drop {
        return;
    }
    let is_image = [".png", ".jpg", ".jpeg", ".gif", ".webp", "/thumb/", "/src/", "/ext/"].iter().any(|k| path.contains(k)) && !path.ends_with(".json");
    let (status, kind, body) = if since && unchanged && !is_image {
        (304, "application/json", Vec::new())
    } else if is_image {
        let mut bytes = fuzz::IMAGES[image_index % fuzz::IMAGES.len()].clone();
        if corrupt {
            bytes.truncate(bytes.len() / 2);
        }
        (200, "image/png", bytes)
    } else {
        let raw = answer(&format!("http://127.0.0.1:{port}{path}"), None);
        let mut body = local_urls(&raw.body, port);
        // Now and then a post far taller than the screen.
        if long_post && !path.contains("catalog") {
            body = make_long_post(&body);
        }
        (raw.status, "application/json", body.into_bytes())
    };
    let modified = if kind == "application/json" && status == 200 { format!("Last-Modified: {MODIFIED}\r\n") } else { String::new() };
    let head = format!("HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\n{modified}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    // Now and then the connection drops halfway through the body.
    let sent = if cut { body.len() / 2 } else { body.len() };
    let _ = stream.write_all(body.get(..sent).unwrap_or_default());
}

/// The first comment in an answer (whichever engine's field it's in) made a few hundred
/// lines long.
fn make_long_post(body: &str) -> String {
    let lines = "a long post, line after line<br>".repeat(300);
    let plain = "a long post, line after line\\n".repeat(300);
    for field in ["\"com\":\"", "\"comment\":\"", "\"message\":\"", "\"markdown\":\""] {
        if let Some(at) = body.find(field) {
            let at = at + field.len();
            let insert = if field.contains("message") { &plain } else { &lines };
            return format!("{}{insert}{}", body.get(..at).unwrap_or_default(), body.get(at..).unwrap_or_default());
        }
    }
    body.to_string()
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

    fn start(&self, ck: &Path, env: &[(String, String)], size: (u16, u16)) {
        // The window stays after ck exits, showing how it went.
        self.launch(&format!("{}; echo CK_EXIT=$?; sleep 600", command(ck, env, &[])), size);
    }

    /// `cmd` in a new window of a new server.
    fn launch(&self, cmd: &str, (w, h): (u16, u16)) {
        let _ = self.run(&["kill-server"]);
        self.run(&["new-session", "-d", "-s", "ck", "-x", &w.to_string(), "-y", &h.to_string(), cmd]);
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

/// The shell command that runs ck with `env` and `args`.
fn command(ck: &Path, env: &[(String, String)], args: &[&str]) -> String {
    let vars: String = env.iter().map(|(k, v)| format!("{k}='{v}' ")).collect();
    let args: String = args.iter().map(|a| format!(" '{a}'")).collect();
    format!("env {vars} '{}'{args}", ck.display())
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

/// Wait until ck has drawn, then check that the first key, sent once after a pause, is
/// acted on. (A query about images that the terminal never answered used to leave a reader
/// on stdin that took the first key: tmux drops the query unless `allow-passthrough` is on.)
fn ready(t: &Tmux) {
    for _ in 0..50 {
        if t.screen().contains(" ck ") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_secs(3));
    t.keys(&["?"]);
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        let s = t.screen();
        assert!(!s.contains("CK_EXIT="), "ck exited while starting:\n{s}");
        if s.contains("esc close") {
            t.keys(&["z"]);
            std::thread::sleep(Duration::from_millis(300));
            return;
        }
    }
    panic!("the first key wasn't acted on:\n{}", t.screen());
}

/// The release binary (`CK_BIN`, or target/release/ck).
fn release_ck() -> PathBuf {
    let ck = PathBuf::from(std::env::var("CK_BIN").unwrap_or_else(|_| format!("{}/target/release/ck", env!("CARGO_MANIFEST_DIR"))));
    assert!(ck.exists(), "no {} (cargo build --release first)", ck.display());
    ck
}

/// The release binary, if tmux is there to run it in (`None`: skipped).
fn ck_and_tmux(test: &str) -> Option<PathBuf> {
    let ck = release_ck();
    if Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("{test}: no tmux, skipped");
        return None;
    }
    Some(ck)
}

/// `config` written to a scratch config directory in `dir`, and the environment that has
/// ck use it and keep its data, cache and last frame (frame.txt) in `dir` too.
fn scratch(dir: &Path, config: &str) -> Vec<(String, String)> {
    std::fs::create_dir_all(dir.join("config/ck")).unwrap();
    std::fs::write(dir.join("config/ck/config.toml"), config).unwrap();
    [("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache")]
        .iter()
        .map(|(k, d)| (k.to_string(), dir.join(d).display().to_string()))
        .chain([("CK_NO_EXTERNAL".into(), "1".into()), ("COLORTERM".into(), "truecolor".into())])
        .chain([("CK_FRAME_DUMP".into(), dir.join("frame.txt").display().to_string())])
        .collect()
}

const KEYS: &[&str] = &[
    "j", "j", "j", "k", "Enter", "Enter", "Escape", "h", "l", "g", "G", "Down", "Up", "PageDown", "PageUp", "Home", "End",
    "Tab", "BTab", "Space", "BSpace", "w", "s", "c", "v", "i", "b", "u", "U", "p", "n", "N", "S", "d", "D", "a", "x", "y",
    "Y", "O", "H", "Z", "R", "V", "E", "e", "m", "T", "*", "F", "f", "r", "F5", ",", "?", "C-w", "z", "1", "2", ".", "[",
    "]", "A", "Tab", "Tab", "a", "s", "d", "X", "Enter", "X", "u", "c", "c", "Escape", "+", "+", "-", "0",
];

#[test]
#[ignore = "slow: runs the release binary in tmux for a minute"]
fn e2e_soak() {
    let Some(ck) = ck_and_tmux("e2e_soak") else { return };
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
        let _ = write!(config, "\n[[site]]\nname = \"{name}\"\nkind = \"{kind}\"\nurl = \"http://127.0.0.1:{port}\"\n{extra}");
    }
    let env = scratch(dir.path(), &config);
    let tmux = Tmux { socket: format!("ck-e2e-{}", std::process::id()) };
    tmux.start(&ck, &env, (110, 32));
    ready(&tmux);

    let start = Instant::now();
    let (mut sent, mut restarts, mut quits, mut warm, mut peak) = (0u64, 0, 0, None, 0u64);
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
            let mut s = tmux.screen();
            // Random keys can choose "quit" in the . menu (G goes to its last row, enter runs
            // it): a clean exit from a frame showing the menu is that. Start again.
            let frame = std::fs::read_to_string(dir.path().join("frame.txt")).unwrap_or_default();
            if s.contains("CK_EXIT=0") && frame.contains("enter run") && frame.lines().any(|l| l.trim_end().ends_with(" quit")) {
                tmux.start(&ck, &env, (110, 32));
                ready(&tmux);
                (quits, warm) = (quits + 1, None);
                s = tmux.screen();
            }
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
    eprintln!("e2e_soak: {sent} steps, {restarts} restarts, {quits} quits from the menu, peak {peak} KB, no failures");
}

/// A config with one small vichan site on a local server.
fn one_site(seed: u64) -> String {
    let port = serve(SiteKind::Vichan, seed);
    format!("notify = \"off\"\nrestore_session = true\n\n[[site]]\nname = \"vichan\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:{port}\"\nboards = [\"g\"]\n")
}

/// Closing the window (here, killing the pane) hangs ck up and takes its terminal away: it
/// must still save and exit, not abort when it can't put the terminal back. (It used to:
/// ratatui::restore panicked writing its error to the gone stderr, inside the panic hook.)
#[test]
#[ignore = "slow: runs the release binary in tmux"]
fn e2e_terminal_gone() {
    let Some(ck) = ck_and_tmux("e2e_terminal_gone") else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = scratch(dir.path(), &one_site(7));
    let status = dir.path().join("status");
    let session = dir.path().join("data/ck/session.json");
    let tmux = Tmux { socket: format!("ck-e2e-gone-{}", std::process::id()) };
    // The shell outlives the hangup (a trap, which ck doesn't inherit) to note how ck ended.
    let cmd = format!("trap : HUP; {}; echo $? > '{}'", command(&ck, &env, &["vichan/g"]), status.display());
    tmux.launch(&cmd, (110, 32));
    ready(&tmux);
    assert!(!session.exists(), "the session was saved before ck quit");
    tmux.run(&["kill-pane", "-t", "ck"]);
    let ended = Instant::now();
    while !status.exists() && ended.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(100));
    let code = std::fs::read_to_string(&status).unwrap_or_else(|_| "(still running)".into());
    // 0, or 1 when drawing failed before the hangup was seen: not a panic (101) or a signal
    // (128 and up; 134 is an abort).
    assert!(["0", "1"].contains(&code.trim()), "ck didn't exit cleanly when its terminal went: {code}");
    assert!(session.exists(), "ck didn't save its session when its terminal went");
}

/// Output to a closed pipe (`ck --help | head -0`): ck must exit as usual, not panic on the
/// failed write.
#[test]
#[ignore = "runs the release binary"]
fn e2e_closed_output() {
    let ck = release_ck();
    let dir = tempfile::tempdir().unwrap();
    let env = scratch(dir.path(), "");
    for (args, code) in [(&["--help"][..], 0), (&["--version"], 0), (&["--print-config"], 0), (&["--print-sites"], 0), (&["--nosuch"], 1), (&["a", "b"], 1)] {
        let (closed, out) = std::io::pipe().unwrap();
        drop(closed);
        let err = out.try_clone().unwrap();
        let status = Command::new(&ck).args(args).envs(env.iter().cloned()).stdout(out).stderr(err).status().unwrap();
        assert_eq!(status.code(), Some(code), "ck {args:?} into a closed pipe: {status}");
    }
}
