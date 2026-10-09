//! ck-web: ck's browser helper. Chromium (CEF) with no window, on 4chan, for what a plain
//! HTTP client can't do there: get past Cloudflare to the captcha, and post. ck starts it
//! and talks to it on its stdin and stdout (`protocol.rs`); when the site wants a person
//! (Cloudflare's check, hCaptcha), it sends what the page shows, and ck sends the clicks.
//!
//! CEF starts its other processes (renderer, GPU, network) from this same program, with a
//! `--type` switch: `execute_process` runs those and returns.

#[path = "../../src/web/protocol.rs"]
mod protocol;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use base64::Engine;
use cef::{args::Args, rc::Rc as _, *};
use protocol::{Reply, Request};
use serde::Deserialize;

/// The browser view's size, in pixels: room for Cloudflare's check and hCaptcha's puzzles.
const VIEW_W: i32 = 480;
const VIEW_H: i32 = 600;
/// What the page's script starts its console messages with (`page.js`).
const SAID: &str = "ck-web:";
/// Without a captcha this long after its frame starts loading, the site is asking for a
/// person: the view is shown.
const PERSON_AFTER: Duration = Duration::from_millis(2500);
/// The view is sent at most this often.
const FRAME_EVERY: Duration = Duration::from_millis(150);

/// What the page's script said (`page.js`): a reply for ck, or news for ck-web alone.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Said {
    Page(Page),
    Reply(Reply),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "is", rename_all = "snake_case")]
enum Page {
    /// The captcha frame is loading.
    Loading,
    /// Something only a person can do is showing (hCaptcha).
    Human,
}

/// What CEF's handlers found, for the main loop: they run on its thread, inside
/// `do_message_loop_work`.
#[derive(Default)]
struct Found {
    /// The view as last painted (BGRA), and whether it's changed since it was last sent.
    frame: Option<(Vec<u8>, u32, u32)>,
    dirty: bool,
    said: Vec<Said>,
    /// The page's main frame finished loading: time for the script.
    loaded: bool,
}

type Shared = Rc<RefCell<Found>>;

#[derive(Clone)]
struct WebApp;

wrap_app! {
    struct AppBuilder { app: WebApp, }
    impl App {
        fn on_before_command_line_processing(&self, _process: Option<&CefStringUtf16>, line: Option<&mut CommandLine>) {
            let Some(line) = line else { return };
            for switch in ["no-startup-window", "noerrdialogs", "disable-gpu", "use-mock-keychain", "mute-audio"] {
                line.append_switch(Some(&switch.into()));
            }
        }
    }
}

#[derive(Clone)]
struct Painter(Shared);

wrap_render_handler! {
    struct PainterBuilder { painter: Painter, }
    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(r) = rect {
                r.width = VIEW_W;
                r.height = VIEW_H;
            }
        }

        fn on_paint(&self, _browser: Option<&mut Browser>, kind: PaintElementType, _dirty: Option<&[Rect]>, buffer: *const u8, width: i32, height: i32) {
            let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else { return };
            if kind != PaintElementType::default() || buffer.is_null() || w == 0 || h == 0 {
                return;
            }
            let len = (w as usize) * (h as usize) * 4;
            // SAFETY: CEF hands a BGRA buffer of `width` x `height` pixels, valid for this call.
            let pixels = unsafe { std::slice::from_raw_parts(buffer, len) }.to_vec();
            let mut found = self.painter.0.borrow_mut();
            found.frame = Some((pixels, w, h));
            found.dirty = true;
        }
    }
}

#[derive(Clone)]
struct Console(Shared);

wrap_display_handler! {
    struct ConsoleBuilder { console: Console, }
    impl DisplayHandler {
        fn on_console_message(&self, _browser: Option<&mut Browser>, _level: LogSeverity, message: Option<&CefString>, _source: Option<&CefString>, _line: i32) -> i32 {
            let text = message.map(|m| m.to_string()).unwrap_or_default();
            if let Some(said) = text.strip_prefix(SAID).and_then(|json| serde_json::from_str(json).ok()) {
                self.console.0.borrow_mut().said.push(said);
                return 1;
            }
            0
        }
    }
}

#[derive(Clone)]
struct Loads(Shared);

wrap_load_handler! {
    struct LoadsBuilder { loads: Loads, }
    impl LoadHandler {
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _status: i32) {
            if frame.is_some_and(|f| f.is_main() == 1) {
                self.loads.0.borrow_mut().loaded = true;
            }
        }
    }
}

#[derive(Clone)]
struct NoPopups;

wrap_life_span_handler! {
    struct NoPopupsBuilder { none: NoPopups, }
    impl LifeSpanHandler {
        // A link that would open a window (a captcha's "privacy" link) opens nothing.
        #[allow(clippy::too_many_arguments)]
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: i32,
            _target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: i32,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut i32>,
        ) -> i32 {
            1
        }
    }
}

wrap_client! {
    struct ClientBuilder { painter: RenderHandler, console: DisplayHandler, loads: LoadHandler, popups: LifeSpanHandler, }
    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> { Some(self.painter.clone()) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(self.console.clone()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(self.loads.clone()) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(self.popups.clone()) }
    }
}

fn main() -> ExitCode {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = Args::new();
    let Some(line) = args.as_cmd_line() else { return ExitCode::FAILURE };
    let browser_process = line.has_switch(Some(&CefString::from("type"))) != 1;
    let mut app = AppBuilder::new(WebApp);
    let code = execute_process(Some(args.as_main_args()), Some(&mut app), std::ptr::null_mut());
    if !browser_process {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    // The replies' own channel, before CEF starts anything that could write to stdout.
    let Some(out) = take_stdout() else {
        eprintln!("ck-web: can't set up its output");
        return ExitCode::FAILURE;
    };
    let profile = std::env::var_os("CK_WEB_PROFILE").map(PathBuf::from).or_else(|| dirs::cache_dir().map(|d| d.join("ck").join("web")));
    // Held until ck-web exits.
    let Some((profile, _lock)) = profile.and_then(|p| free_profile(&p)) else {
        eprintln!("ck-web: no browser profile folder it can use");
        return ExitCode::FAILURE;
    };
    let mut web = Web { out: Some(out), ..Default::default() };
    match run(&args, &mut app, &profile, &mut web) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            web.send(&Reply::Failed { error: e.to_string() });
            ExitCode::FAILURE
        }
    }
}

/// The browser process: start CEF on a blank page, then take ck's requests until its stdin
/// closes.
fn run(args: &Args, app: &mut cef::App, profile: &std::path::Path, web: &mut Web) -> Result<(), String> {
    let path = |p: PathBuf| CefString::from(p.to_string_lossy().as_ref());
    let settings = Settings {
        windowless_rendering_enabled: 1,
        external_message_pump: 1,
        // The profile keeps Cloudflare's pass and 4chan's cookies between runs.
        persist_session_cookies: 1,
        root_cache_path: path(profile.to_path_buf()),
        cache_path: path(profile.join("profile")),
        log_file: path(profile.join("log.txt")),
        log_severity: LogSeverity::WARNING,
        // The bundle ships English alone.
        locale: CefString::from("en-US"),
        no_sandbox: i32::from(!sandbox_works()),
        ..Default::default()
    };
    if initialize(Some(args.as_main_args()), Some(&settings), Some(app), std::ptr::null_mut()) != 1 {
        return Err("Chromium (CEF) didn't start".into());
    }
    let found = Shared::default();
    let mut client = ClientBuilder::new(
        PainterBuilder::new(Painter(found.clone())),
        ConsoleBuilder::new(Console(found.clone())),
        LoadsBuilder::new(Loads(found.clone())),
        NoPopupsBuilder::new(NoPopups),
    );
    let window = WindowInfo { windowless_rendering_enabled: 1, ..Default::default() };
    let browser_settings = BrowserSettings { windowless_frame_rate: 15, ..Default::default() };
    let Some(browser) = browser_host_create_browser_sync(Some(&window), Some(&mut client), Some(&"about:blank".into()), Some(&browser_settings), None, None) else {
        shutdown();
        return Err("Chromium (CEF) didn't open a page".into());
    };
    if let Some(host) = browser.host() {
        host.set_focus(1);
    }
    let requests = read_requests();
    let mut open = true;
    while open {
        do_message_loop_work();
        loop {
            match requests.try_recv() {
                Ok(r) => web.waiting.push_back(r),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    open = false;
                    break;
                }
            }
        }
        let (loaded, said) = {
            let mut f = found.borrow_mut();
            (std::mem::take(&mut f.loaded), std::mem::take(&mut f.said))
        };
        if loaded {
            web.reloaded();
            run_js(&browser, include_str!("page.js"));
        }
        let at = browser.main_frame().map(|f| CefString::from(&f.url()).to_string()).unwrap_or_default();
        for s in said {
            web.on_said(s, &at);
        }
        web.take_requests(&browser);
        web.tick(&browser, &found);
        open &= web.out.is_some();
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Some(host) = browser.host() {
        host.close_browser(1);
    }
    for _ in 0..50 {
        do_message_loop_work();
        std::thread::sleep(Duration::from_millis(10));
    }
    shutdown();
    Ok(())
}

/// ck's side: its requests, where replies go, and whether the browser view is being sent.
#[derive(Default)]
struct Web {
    /// None once ck has gone (a write failed).
    out: Option<Box<dyn Write>>,
    /// ck's requests not taken yet, and the one being answered.
    waiting: VecDeque<Request>,
    busy: Option<Request>,
    /// Whether the page's script is running (it isn't while the page loads).
    ready: bool,
    viewing: bool,
    /// When to show the view if what's awaited hasn't come by then.
    person_at: Option<Instant>,
    last_frame: Option<Instant>,
}

impl Web {
    fn send(&mut self, reply: &Reply) {
        let Some(out) = &mut self.out else { return };
        let Ok(line) = serde_json::to_string(reply) else { return };
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            self.out = None;
        }
    }

    /// The page loaded again (Cloudflare's check passed, say): what it was doing is lost.
    fn reloaded(&mut self) {
        self.ready = false;
        match self.busy.take() {
            // Answered once the new page is ready.
            Some(r @ Request::Open { .. }) => self.busy = Some(r),
            // Asked again on the new page.
            Some(r @ Request::Captcha { .. }) => self.waiting.push_front(r),
            Some(Request::Fetch { method, .. }) if method != "GET" => {
                // A form sent may have gone through, and sending it again could post twice.
                self.send(&Reply::Failed { error: "The page reloaded before the site answered: check the thread before posting again".into() });
            }
            Some(r) => self.waiting.push_front(r),
            None => {}
        }
    }

    /// What the page said, on the page at `at`.
    fn on_said(&mut self, said: Said, at: &str) {
        match said {
            Said::Page(Page::Loading) => {
                self.viewing = false;
                self.person_at = Instant::now().checked_add(PERSON_AFTER);
            }
            Said::Page(Page::Human) => self.show(),
            // The blank page ck-web starts on isn't the one asked for, though its script runs.
            Said::Reply(Reply::Ready) if at.is_empty() || at.starts_with("about:") => {}
            Said::Reply(Reply::Ready) => {
                self.ready = true;
                // Only an `Open` is answered: a reload's ready is the page's own business.
                if matches!(self.busy, Some(Request::Open { .. })) {
                    self.busy = None;
                    self.hide();
                    self.send(&Reply::Ready);
                }
            }
            Said::Reply(reply) => {
                // An answer to something cancelled is dropped, not taken for the next one's.
                if self.busy.take().is_none() {
                    return;
                }
                self.hide();
                self.send(&reply);
            }
        }
    }

    /// Take ck's requests: the page's own once its script runs and nothing else is being
    /// answered; the browser's at once.
    fn take_requests(&mut self, browser: &Browser) {
        while let Some(r) = self.waiting.front() {
            let page = matches!(r, Request::Fetch { .. } | Request::Captcha { .. });
            if page && (!self.ready || self.busy.is_some()) {
                return;
            }
            let Some(r) = self.waiting.pop_front() else { return };
            self.on_request(browser, r);
        }
    }

    fn show(&mut self) {
        self.viewing = true;
        self.person_at = None;
        // The page as it is now goes out at once.
        self.last_frame = None;
    }

    fn hide(&mut self) {
        self.viewing = false;
        self.person_at = None;
    }

    fn on_request(&mut self, browser: &Browser, request: Request) {
        match &request {
            Request::Open { url } => {
                self.ready = false;
                self.person_at = Instant::now().checked_add(PERSON_AFTER);
                if let Some(frame) = browser.main_frame() {
                    frame.load_url(Some(&url.as_str().into()));
                }
            }
            Request::Captcha { board, thread } => run_js(browser, &format!("ckweb.captcha({}, {thread})", json(board))),
            Request::Fetch { method, url, headers, fields, file, body } => {
                let file = match file.as_ref().map(upload) {
                    None => serde_json::Value::Null,
                    Some(Ok(f)) => f,
                    Some(Err(error)) => return self.send(&Reply::Failed { error }),
                };
                run_js(browser, &format!("ckweb.send({}, {}, {}, {}, {file}, {})", json(method), json(url), json(headers), json(fields), json(body)));
            }
            Request::Click { x, y } => {
                let Some(host) = browser.host() else { return };
                let at = mouse(*x, *y);
                host.send_mouse_move_event(Some(&at), 0);
                host.send_mouse_click_event(Some(&at), MouseButtonType::LEFT, 0, 1);
                host.send_mouse_click_event(Some(&at), MouseButtonType::LEFT, 1, 1);
                return;
            }
            Request::Scroll { x, y, dy } => {
                if let Some(host) = browser.host() {
                    host.send_mouse_wheel_event(Some(&mouse(*x, *y)), 0, -dy);
                }
                return;
            }
            Request::Cancel => {
                self.hide();
                self.busy = None;
                self.waiting.clear();
                run_js(browser, "ckweb && ckweb.cancel()");
                return;
            }
        }
        self.busy = Some(request);
    }

    /// Show the view once the captcha is late, and send it when it's changed.
    fn tick(&mut self, browser: &Browser, found: &Shared) {
        let now = Instant::now();
        if self.person_at.is_some_and(|at| now >= at) {
            self.show();
            if let Some(host) = browser.host() {
                host.invalidate(PaintElementType::default());
            }
        }
        if !self.viewing || self.last_frame.is_some_and(|t| now.duration_since(t) < FRAME_EVERY) {
            return;
        }
        let frame = {
            let mut f = found.borrow_mut();
            if !f.dirty && self.last_frame.is_some() {
                return;
            }
            f.dirty = false;
            f.frame.clone()
        };
        let Some((bgra, width, height)) = frame else { return };
        self.last_frame = Some(now);
        if let Some((png, left, top)) = png(bgra, width, height) {
            self.send(&Reply::View { png, left, top });
        }
    }
}

fn run_js(browser: &Browser, code: &str) {
    if let Some(frame) = browser.main_frame() {
        let url = CefString::from(&frame.url());
        frame.execute_java_script(Some(&code.into()), Some(&url), 0);
    }
}

fn json<T: serde::Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

fn mouse(x: u32, y: u32) -> MouseEvent {
    let px = |v: u32, max: i32| i32::try_from(v).unwrap_or(max).clamp(0, max - 1);
    MouseEvent { x: px(x, VIEW_W), y: px(y, VIEW_H), modifiers: 0 }
}

/// A file for the page's form: its field, name and bytes (base64).
fn upload(file: &protocol::Upload) -> Result<serde_json::Value, String> {
    let p = std::path::Path::new(&file.path);
    let bytes = std::fs::read(p).map_err(|e| format!("Couldn't read {}: {e}", file.path))?;
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(serde_json::json!({ "field": file.field, "name": name, "data": data }))
}

/// A painted BGRA frame as a PNG (base64): the part with something on it (not the page's
/// white), with a margin, and where that part starts.
fn png(mut bgra: Vec<u8>, width: u32, height: u32) -> Option<(String, u32, u32)> {
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(width, height, bgra)?).to_rgb8();
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p.0.iter().any(|&c| c < 0xf0) {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    let margin = 12;
    let (left, top) = if x0 > x1 { (0, 0) } else { (x0.saturating_sub(margin), y0.saturating_sub(margin)) };
    let (right, bottom) = if x0 > x1 { (width, height) } else { ((x1 + margin).min(width), (y1 + margin).min(height)) };
    let part = image::imageops::crop_imm(&img, left, top, right - left, bottom - top).to_image();
    let mut out = std::io::Cursor::new(Vec::new());
    part.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some((base64::engine::general_purpose::STANDARD.encode(out.into_inner()), left, top))
}

/// A profile folder no other ck-web is using (Chromium allows one browser a profile): `base`,
/// else `base-1`, `base-2`… for a second ck running at once, each keeping its own cookies.
/// It's held while the file returned is open.
fn free_profile(base: &std::path::Path) -> Option<(PathBuf, std::fs::File)> {
    (0..10).find_map(|n| {
        let dir = if n == 0 { base.to_path_buf() } else { base.with_file_name(format!("{}-{n}", base.file_name()?.to_string_lossy())) };
        std::fs::create_dir_all(&dir).ok()?;
        // The folder itself is locked (flock works on a folder opened for reading).
        let lock = std::fs::File::open(&dir).ok()?;
        lock.try_lock().ok()?;
        Some((dir, lock))
    })
}

/// Whether Chromium's sandbox can start: it needs user namespaces, which Ubuntu (since
/// 23.10) and some kernels keep from programs like this one. Without them it runs unsandboxed
/// (it only ever loads 4chan's pages and its captchas').
fn sandbox_works() -> bool {
    let read = |p: &str| std::fs::read_to_string(p).map(|s| s.trim().to_string()).ok();
    read("/proc/sys/kernel/apparmor_restrict_unprivileged_userns").as_deref() != Some("1") && read("/proc/sys/kernel/unprivileged_userns_clone").as_deref() != Some("0")
}

/// ck's requests, a line each, read on their own thread. The channel closes with stdin.
fn read_requests() -> Receiver<Request> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str(&line) {
                Ok(r) => {
                    if tx.send(r).is_err() {
                        break;
                    }
                }
                Err(e) => eprintln!("ck-web: not a request: {e}: {line}"),
            }
        }
    });
    rx
}

/// Stdout for the replies alone: kept on a descriptor of its own (closed in CEF's other
/// processes), with stdout itself pointed at stderr, so nothing CEF prints gets in.
#[cfg(unix)]
fn take_stdout() -> Option<Box<dyn Write>> {
    use std::os::fd::FromRawFd;
    // SAFETY: descriptor calls on 1 and 2, which every process starts with; the new
    // descriptor is owned by the file made from it, and by nothing else.
    unsafe {
        let fd = libc::fcntl(1, libc::F_DUPFD_CLOEXEC, 3);
        if fd < 0 || libc::dup2(2, 1) < 0 {
            return None;
        }
        Some(Box::new(std::fs::File::from_raw_fd(fd)))
    }
}

#[cfg(not(unix))]
fn take_stdout() -> Option<Box<dyn Write>> {
    Some(Box::new(std::io::stdout()))
}
