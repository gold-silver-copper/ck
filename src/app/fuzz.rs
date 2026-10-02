//! The app under random input: keys, clicks, pastes, resizes, time passing and restarts,
//! against fake sites whose answers the fuzzer hands out in any order (or as errors). After
//! every step it draws a frame and checks what must always hold; when nothing is left in
//! flight it checks that nothing is stuck loading. Nothing leaves the machine: the fake
//! sites answer from generated data, and their file URLs are on `fuzz.invalid`, which
//! `http` refuses in tests.
//!
//! `cargo test fuzz_app` runs a short pass; `cargo test --release -- --ignored fuzz_app_long
//! --nocapture` a long one (`FUZZ_SEED`, `FUZZ_RUNS`, `FUZZ_STEPS`).

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::*;
use crate::backend::{Partial, SearchPage};
use crate::fuzz::{self, Rng};
use crate::markup;
use crate::http::{HttpError, lock};
use crate::model::Attachment;

// ----- fake sites -----

/// Holds every backend call until the fuzzer lets it answer.
#[derive(Default)]
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}

#[derive(Default)]
struct GateState {
    next: u64,
    /// Calls waiting to answer: (ticket, what was asked).
    waiting: Vec<(u64, String)>,
    released: HashSet<u64>,
    /// Calls answering right now (released, not yet returned).
    running: usize,
    /// Everything answers at once (the episode is over).
    open: bool,
}

/// A call in progress; dropping it marks the answer as given.
struct Pass<'a>(&'a Gate);

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        lock(&self.0.state).running -= 1;
        self.0.changed.notify_all();
    }
}

impl Gate {
    fn enter(&self, what: String) -> Pass<'_> {
        let mut s = lock(&self.state);
        let ticket = s.next;
        s.next += 1;
        s.waiting.push((ticket, what));
        self.changed.notify_all();
        while !s.open && !s.released.contains(&ticket) {
            s = self.changed.wait(s).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        s.released.remove(&ticket);
        s.waiting.retain(|(t, _)| *t != ticket);
        s.running += 1;
        Pass(self)
    }

    /// The waiting calls, in a stable order (by what was asked).
    fn waiting(&self) -> Vec<(u64, String)> {
        let mut w = lock(&self.state).waiting.clone();
        w.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        w
    }

    /// Let one call answer, and wait until it has.
    fn release(&self, ticket: u64) {
        let mut s = lock(&self.state);
        s.released.insert(ticket);
        self.changed.notify_all();
        let deadline = Instant::now() + Duration::from_secs(10);
        while (s.waiting.iter().any(|(t, _)| *t == ticket) || s.running > 0) && Instant::now() < deadline {
            s = self.changed.wait_timeout(s, Duration::from_millis(50)).unwrap_or_else(std::sync::PoisonError::into_inner).0;
        }
    }

    fn open(&self) {
        lock(&self.state).open = true;
        self.changed.notify_all();
    }

    fn calls(&self) -> u64 {
        lock(&self.state).next
    }
}

/// A site that answers from generated data, through the gate. URLs are the real site's.
struct Fake {
    real: Arc<dyn Backend>,
    site: String,
    seed: u64,
    gate: Arc<Gate>,
    /// How often each thread has been fetched: threads grow between fetches.
    fetched: Mutex<HashMap<(String, u64), u64>>,
    calls: AtomicU64,
}

fn hash(parts: &[&dyn std::fmt::Debug]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        format!("{p:?}").hash(&mut h);
    }
    h.finish()
}

const BOARD_NAMES: &[&str] = &["g", "b", "a", "λ", "tech", "x", "qa", "lounge", "日本", "a-very-long-board-name", "v"];
const EXTS: &[&str] = &["png", "jpg", "gif", "webm", "mp4", "pdf", "webp", ""];

impl Fake {
    /// The outcome of a call: same question, same answer (a refetch may differ).
    fn rng(&self, what: &str) -> Rng {
        Rng::new(hash(&[&self.seed, &self.site, &what, &self.calls.fetch_add(0, Ordering::Relaxed)]))
    }

    fn fail(&self, rng: &mut Rng, url: &str) -> Option<anyhow::Error> {
        match rng.below(100) {
            0..4 => Some(HttpError::NotFound(url.into()).into()),
            4..7 => Some(HttpError::RateLimited.into()),
            7..10 => Some(anyhow::anyhow!("connection reset (fake)")),
            _ => None,
        }
    }

    fn file(&self, rng: &mut Rng, board: &str, n: u64) -> Attachment {
        let ext = *rng.pick(EXTS);
        let name = if ext.is_empty() { format!("{n}") } else { format!("{n}.{ext}") };
        let url = format!("https://fuzz.invalid/{}/{board}/src/{name}", self.site);
        Attachment {
            filename: if rng.chance(10) { fuzz::html(rng, 3) } else { name.clone() },
            thumb: rng.chance(80).then(|| format!("https://fuzz.invalid/{}/{board}/thumb/{n}s.jpg", self.site)),
            spoiler: rng.chance(10),
            width: rng.chance(70).then(|| rng.below(5000) as u32),
            height: rng.chance(70).then(|| rng.below(5000) as u32),
            size: rng.chance(70).then(|| rng.below(1 << 30) as u64),
            md5: rng.chance(30).then(|| format!("{:x}", rng.next())),
            url,
        }
    }

    fn post(&self, rng: &mut Rng, board: &str, no: u64, earlier: &[u64]) -> Post {
        let pieces = rng.below(25);
        let mut html = fuzz::html(rng, pieces);
        for _ in 0..rng.below(4) {
            let target = if !earlier.is_empty() && rng.chance(80) { *rng.pick(earlier) } else { rng.below(1_000_000) as u64 };
            html = format!("<a href=\"#p{target}\" class=\"quotelink\">&gt;&gt;{target}</a><br>{html}");
        }
        let flavor = *rng.pick(&[markup::Flavor::Fourchan, markup::Flavor::Vichan, markup::Flavor::Lynxchan, markup::Flavor::Jschan]);
        let most = if rng.chance(70) { 2 } else { 6 };
        let files = rng.below(most);
        Post {
            no,
            name: if rng.chance(90) { "Anonymous".into() } else { fuzz::html(rng, 2) },
            subject: rng.chance(30).then(|| fuzz::html(rng, 3)),
            time: 1_790_000_000 - rng.below(400 * 86_400) as i64,
            files: (0..files).map(|k| self.file(rng, board, no * 10 + k as u64)).collect(),
            replies: rng.chance(90).then(|| rng.below(800) as u32),
            images: rng.chance(90).then(|| rng.below(200) as u32),
            sticky: rng.chance(5),
            locked: rng.chance(5),
            bumplimit: rng.chance(5),
            board: rng.chance(10).then(|| rng.pick(BOARD_NAMES).to_string()),
            ..markup::parse_html(&html, flavor).into()
        }
    }
}

impl Backend for Fake {
    fn boards(&self, partial: Partial<Board>) -> Result<Vec<Board>> {
        let _pass = self.gate.enter(format!("{} boards", self.site));
        let mut rng = self.rng("boards");
        if let Some(e) = self.fail(&mut rng, "boards") {
            return Err(e);
        }
        let mut names: Vec<&str> = (0..rng.below(BOARD_NAMES.len() + 1)).map(|_| *rng.pick(BOARD_NAMES)).collect();
        names.dedup();
        let boards: Vec<Board> = names
            .iter()
            .map(|uri| Board { uri: uri.to_string(), title: fuzz::html(&mut rng, 2), nsfw: rng.chance(50).then(|| rng.chance(30)) })
            .collect();
        if rng.chance(30) {
            partial(boards.get(..boards.len() / 2).unwrap_or_default());
        }
        Ok(boards)
    }

    fn catalog(&self, board: &str, partial: Partial<Post>) -> Result<Vec<Post>> {
        let _pass = self.gate.enter(format!("{} catalog /{board}/", self.site));
        let mut rng = self.rng(&format!("catalog {board}"));
        if let Some(e) = self.fail(&mut rng, board) {
            return Err(e);
        }
        let n = if rng.chance(10) { 0 } else { rng.below(60) };
        let posts: Vec<Post> = (0..n)
            .map(|i| {
                let no = 1000 + (i as u64) * 7 + rng.below(5) as u64;
                self.post(&mut rng, board, no, &[])
            })
            .collect();
        if rng.chance(30) {
            partial(posts.get(..posts.len() / 2).unwrap_or_default());
        }
        Ok(posts)
    }

    fn thread(&self, board: &str, no: u64) -> Result<Vec<Post>> {
        let _pass = self.gate.enter(format!("{} thread /{board}/{no}", self.site));
        let generation = {
            let mut f = lock(&self.fetched);
            let g = f.entry((board.to_string(), no)).or_default();
            *g += 1;
            *g
        };
        let mut rng = Rng::new(hash(&[&self.seed, &self.site, &board, &no, &generation]));
        if let Some(e) = self.fail(&mut rng, board) {
            return Err(e);
        }
        if rng.chance(3) {
            return Ok(Vec::new());
        }
        // The same thread each time, more of it each fetch, and now and then a post deleted.
        let mut base = Rng::new(hash(&[&self.seed, &self.site, &board, &no]));
        let len = 1 + base.below(80) + (generation as usize - 1) * rng.below(6);
        let mut nos = vec![no];
        let mut posts = vec![self.post(&mut base, board, no, &[])];
        let mut p = no;
        for _ in 1..len {
            p += 1 + base.below(3) as u64;
            let post = self.post(&mut base, board, p, &nos);
            nos.push(p);
            if !rng.chance(3) {
                posts.push(post);
            }
        }
        Ok(posts)
    }

    fn find_thread(&self, board: &str, post: u64) -> Result<Option<u64>> {
        let _pass = self.gate.enter(format!("{} find /{board}/{post}", self.site));
        let mut rng = self.rng(&format!("find {board} {post}"));
        if let Some(e) = self.fail(&mut rng, board) {
            return Err(e);
        }
        Ok(rng.chance(70).then(|| post.saturating_sub(rng.below(50) as u64).max(1)))
    }

    fn board_url(&self, board: &str) -> String {
        self.real.board_url(board)
    }

    fn thread_url(&self, board: &str, no: u64) -> String {
        self.real.thread_url(board, no)
    }

    fn search(&self, board: &str, query: &str, page: u32) -> Result<SearchPage> {
        let _pass = self.gate.enter(format!("{} search /{board}/ {query:?} {page}", self.site));
        let mut rng = self.rng(&format!("search {board} {query} {page}"));
        if let Some(e) = self.fail(&mut rng, board) {
            return Err(e);
        }
        let hits = (0..rng.below(12)).map(|i| (1000 + i as u64, self.post(&mut rng, board, 2000 + i as u64, &[]))).collect();
        Ok(SearchPage { hits, total: rng.chance(80).then(|| rng.below(5000) as u64) })
    }

    fn post_url(&self, board: &str, thread: u64, post: u64) -> String {
        self.real.post_url(board, thread, post)
    }
}

// ----- one episode -----

const FILTERS: &str = "[[filter]]\npattern = \"(?i)word\"\nlabel = \"word\"\n\
    [[filter]]\npattern = \"日本\"\naction = \"highlight\"\n\
    [[filter]]\npattern = \"(OP)\"\nfield = \"subject\"\naction = \"hide\"\n";

/// A fresh app on fake sites, with its config and data in `dir`.
fn app_in(dir: &std::path::Path, seed: u64, gate: &Arc<Gate>) -> App {
    let mut rng = Rng::new(seed);
    let mut doc: toml_edit::DocumentMut = crate::config::DEFAULT_CONFIG.parse().unwrap();
    if rng.chance(50) {
        doc["favorites"] = toml_edit::value(toml_edit::Array::from_iter(["4chan/g", "lainchan/λ", "nosuch/x", "4chan/b"]));
    }
    if rng.chance(30) {
        doc["hidden_sites"] = toml_edit::value(toml_edit::Array::from_iter(["kissu", "8kun"]));
    }
    let mut text = doc.to_string();
    if rng.chance(50) {
        text.push_str(FILTERS);
    }
    let cfg: Config = toml::from_str(&text).unwrap();
    std::fs::write(dir.join("config.toml"), &text).unwrap();
    let store = Store::load(Some(dir.join("data"))).0;
    let mut app = App::new(cfg, KeyMap::default(), None, store);
    app.config_path = Some(dir.join("config.toml"));
    app.download_dir = Some(dir.join("downloads").display().to_string());
    app.images = Images::offline();
    app.clock = Clock { fixed: Some(1_790_000_000) };
    for site in &mut app.sites {
        let real = site.backend.clone();
        let name = site.cfg.name.clone();
        let calls = AtomicU64::new(0);
        site.backend = Arc::new(Fake { real, site: name, seed, gate: gate.clone(), fetched: Mutex::default(), calls });
    }
    app
}

/// Keys that do something somewhere, so they come up often.
fn hot_keys() -> Vec<KeyEvent> {
    let mut keys: Vec<KeyEvent> = crate::keys::ACTIONS.iter().map(|&(_, _, k, ..)| KeyEvent::new(k.code, k.mods)).collect();
    for code in [
        KeyCode::Char('j'),
        KeyCode::Char('k'),
        KeyCode::Char('h'),
        KeyCode::Char('l'),
        KeyCode::Char('g'),
        KeyCode::Char('G'),
        KeyCode::Char('J'),
        KeyCode::Char('K'),
        KeyCode::Char(' '),
        KeyCode::Char('q'),
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::F(5),
    ] {
        keys.push(KeyEvent::from(code));
    }
    keys
}

const SIZES: &[(u16, u16)] = &[(110, 32), (80, 24), (60, 20), (200, 60), (40, 12), (20, 8), (3, 2), (1, 1)];

/// What to paste or type after `:`.
fn place(rng: &mut Rng) -> String {
    let forms = [
        "https://boards.4chan.org/g/thread/{n}#p{m}",
        "https://boards.4chan.org/g/",
        "4chan/g/{n}",
        "lainchan/λ",
        "https://lainchan.org/%CE%BB/res/{n}.html#{m}",
        "b/{n}",
        "g",
        "nosuch/x/1",
        "https://desuarchive.org/a/thread/{n}/#{m}",
        "2ch/b/{n}",
        "",
        "  ",
        "日本",
    ];
    let s = rng.pick(&forms).replace("{n}", &(1 + rng.below(3000)).to_string()).replace("{m}", &(1 + rng.below(3000)).to_string());
    if rng.chance(15) { fuzz::html(rng, 3) } else { s }
}

fn episode(seed: u64, steps: usize) {
    let dir = tempfile::tempdir().unwrap();
    let gate = Arc::new(Gate::default());
    let mut rng = Rng::new(seed);
    let mut app = app_in(dir.path(), rng.next(), &gate);
    let hot = hot_keys();
    let (mut w, mut h) = (110, 32);
    let mut log: Vec<String> = Vec::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        for step in 0..steps {
            let calls_before = gate.calls();
            let act = act(&mut rng, &mut app, &gate, &hot, &mut (w, h), dir.path());
            (w, h) = act.1;
            log.push(format!("{step}: {}", act.0));
            app.poll();
            app.quit = false;
            draw(&mut app, w, h);
            check(&app, &log);
            // A step asks a handful of things at most (a few refreshes may fall due at once).
            let asked = gate.calls() - calls_before;
            assert!(asked <= 12, "{asked} requests from one step\n{}", tail(&log));
        }
        settle(&mut app, &gate);
        draw(&mut app, w, h);
        check(&app, &log);
        check_idle(&app, &log);
        // What was saved loads again, without complaint.
        app.save_now();
        let (_, warnings) = Store::load(Some(dir.path().join("data")));
        assert!(warnings.is_empty(), "the saved data doesn't load cleanly: {warnings:?}\n{}", tail(&log));
    }));
    gate.open();
    if let Err(panic) = result {
        eprintln!("\nfuzz_app: the steps before the failure:\n{}", tail(&log));
        resume_unwind(panic);
    }
}

fn tail(log: &[String]) -> String {
    log.iter().rev().take(40).rev().cloned().collect::<Vec<_>>().join("\n")
}

/// One random step; returns what it did and the terminal size after it.
fn act(rng: &mut Rng, app: &mut App, gate: &Arc<Gate>, hot: &[KeyEvent], size: &mut (u16, u16), dir: &std::path::Path) -> (String, (u16, u16)) {
    let (w, h) = *size;
    let roll = rng.below(100);
    let what = match roll {
        // Keys: mostly ones that mean something, then anything.
        0..40 => {
            let key = *rng.pick(hot);
            app.on_key(key);
            format!("key {}", crate::keys::Key::from_event(&key))
        }
        40..48 => {
            let c = char::from(32 + rng.below(95) as u8);
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
            format!("key {c:?}")
        }
        48..50 => {
            let c = char::from_u32(rng.below(0x3000) as u32).unwrap_or('?');
            let mods = *rng.pick(&[KeyModifiers::NONE, KeyModifiers::CONTROL, KeyModifiers::ALT, KeyModifiers::SHIFT]);
            app.on_key(KeyEvent::new(KeyCode::Char(c), mods));
            format!("key {c:?} {mods:?}")
        }
        // Answers: one waiting call, in whatever order.
        50..68 => {
            let waiting = gate.waiting();
            if waiting.is_empty() {
                return ("nothing to answer".into(), *size);
            }
            let (ticket, what) = rng.pick(&waiting).clone();
            gate.release(ticket);
            drain(app);
            format!("answer {what}")
        }
        68..71 => {
            settle(app, gate);
            "answer everything".into()
        }
        // The mouse.
        71..79 => {
            let (col, row) = (rng.below(w as usize) as u16, rng.below(h as usize) as u16);
            let kind = *rng.pick(&[
                MouseEventKind::Down(MouseButton::Left),
                MouseEventKind::Down(MouseButton::Left),
                MouseEventKind::ScrollDown,
                MouseEventKind::ScrollUp,
                MouseEventKind::Down(MouseButton::Right),
            ]);
            let now = Instant::now();
            let ev = MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE };
            app.on_mouse(ev, now);
            if rng.chance(30) {
                app.on_mouse(ev, now);
            }
            format!("mouse {kind:?} at {col},{row}")
        }
        // Going somewhere by hand.
        79..83 => {
            let text = place(rng);
            if rng.chance(50) {
                app.paste(&text);
                format!("paste {text:?}")
            } else {
                app.on_key(KeyEvent::from(KeyCode::Char(':')));
                if app.goto.is_some() {
                    app.paste(&text);
                    app.on_key(KeyEvent::from(KeyCode::Enter));
                }
                format!("goto {text:?}")
            }
        }
        83..87 => {
            *size = *rng.pick(SIZES);
            if rng.chance(30) {
                *size = (1 + rng.below(250) as u16, 1 + rng.below(80) as u16);
            }
            format!("resize {}x{}", size.0, size.1)
        }
        // Time passes: every refresh and save falls due.
        87..92 => {
            let ago = |t: Instant| t.checked_sub(Duration::from_secs(3600)).unwrap_or(t);
            app.tab.thread_checked = ago(app.tab.thread_checked);
            for t in app.watched_checked.values_mut().chain(app.generals_checked.values_mut()) {
                *t = ago(*t);
            }
            app.saved_at = ago(app.saved_at);
            if let Some((_, t)) = &mut app.status_since {
                *t = ago(*t);
            }
            app.notes_since = app.notes_since.map(ago);
            "an hour passes".into()
        }
        // Quit and start again, restoring the session.
        92..94 => {
            settle(app, gate);
            app.save_now();
            *app = app_in(dir, rng.next(), gate);
            "restart".into()
        }
        // Selecting on purpose: something in the list, then enter.
        _ => {
            if let Some((p, len)) = app.picker() {
                p.state.select(Some(rng.below(len + 1)));
            }
            app.on_key(KeyEvent::from(KeyCode::Enter));
            "select and enter".into()
        }
    };
    (what, *size)
}

/// Handle what's arrived, until a moment passes with nothing new.
fn drain(app: &mut App) {
    while app.wait(Duration::from_millis(3)) {}
}

/// Answer every call (and every call those answers lead to) until nothing is waiting.
fn settle(app: &mut App, gate: &Gate) {
    for _ in 0..50 {
        drain(app);
        let waiting = gate.waiting();
        if waiting.is_empty() {
            drain(app);
            if gate.waiting().is_empty() {
                return;
            }
            continue;
        }
        for (ticket, _) in waiting {
            gate.release(ticket);
        }
    }
    panic!("requests kept coming after 50 rounds of answers: {:?}", gate.waiting());
}

fn draw(app: &mut App, w: u16, h: u16) {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::draw(f, app)).unwrap();
}

// ----- what must always hold -----

fn check(app: &App, log: &[String]) {
    let fail = |what: String| -> ! { panic!("{what}\n{}", tail(log)) };
    if app.tabs.is_empty() || app.tabs.len() > MAX_TABS || app.active >= app.tabs.len() {
        fail(format!("tabs: {} open, active {}", app.tabs.len(), app.active));
    }
    let tabs = std::iter::once(&app.tab).chain(app.tabs.iter().enumerate().filter(|&(i, _)| i != app.active).map(|(_, t)| t));
    for (i, tab) in tabs.enumerate() {
        if let Some(t) = &tab.thread {
            check_thread(t, &|w| fail(format!("tab {i}: {w}")));
        }
        if tab.loading.is_some() && tab.req == 0 {
            fail(format!("tab {i} is loading with no request"));
        }
        if let Some(v) = &tab.viewer
            && v.index >= v.files.len()
        {
            fail(format!("tab {i}: viewer on file {} of {}", v.index, v.files.len()));
        }
        if let Some(g) = &tab.gallery
            && (g.files.is_empty() || g.state.selected().is_some_and(|k| k >= g.files.len()))
        {
            fail(format!("tab {i}: gallery selection {:?} of {}", g.state.selected(), g.files.len()));
        }
        if let Some(l) = &tab.links
            && (l.items.is_empty() || l.list.selected().is_some_and(|k| k >= l.items.len()))
        {
            fail(format!("tab {i}: links selection {:?} of {}", l.list.selected(), l.items.len()));
        }
        if let (Some(p), Some(t)) = (&tab.preview, &tab.thread)
            && p.posts.iter().any(|&k| k >= t.posts.len())
        {
            fail(format!("tab {i}: preview of posts {:?} in a thread of {}", p.posts, t.posts.len()));
        }
        // Marks left from an emptied catalog are harmless; a catalog's own must line up.
        if !tab.catalog.is_empty() && tab.catalog_marks.len() != tab.catalog.len() {
            fail(format!("tab {i}: {} catalog marks for {} threads", tab.catalog_marks.len(), tab.catalog.len()));
        }
    }
    if app.settings_popup.is_some() && app.tab.view != View::Settings {
        fail(format!("a settings popup in {:?}", app.tab.view));
    }
    if let Some(p) = &app.image_search_panel
        && p.list.selected().is_some_and(|r| !matches!(p.rows.get(r), Some(Ok(_))))
    {
        fail(format!("image search on row {:?} of {}", p.list.selected(), p.rows.len()));
    }
    let unique = |keys: Vec<&ThreadKey>| keys.len() == keys.iter().collect::<HashSet<_>>().len();
    if !unique(app.store.watched.iter().map(|w| &w.key).collect()) {
        fail("a thread watched twice".into());
    }
    if !unique(app.store.history.iter().map(|v| &v.key).collect()) {
        fail("a thread twice in history".into());
    }
}

fn check_thread(t: &ThreadView, fail: &dyn Fn(String) -> !) {
    let n = t.posts.len();
    if t.index.len() != n || t.posts.iter().enumerate().any(|(i, p)| t.index.get(&p.no) != Some(&i)) {
        fail(format!("the post index doesn't match the {n} posts"));
    }
    if t.backlinks.len() != n {
        fail(format!("{} backlink lists for {n} posts", t.backlinks.len()));
    }
    if n == 0 {
        fail("a thread without posts".into());
    }
    if t.selected >= n {
        fail(format!("post {} selected of {n}", t.selected));
    }
    // The cursor is a hint; `entry()` is the selected post's entry, whatever it says.
    if t.entries.get(t.entry()).is_none_or(|e| e.post != t.selected) {
        fail(format!("entry {} of {} doesn't hold the selected post {}", t.entry(), t.entries.len(), t.selected));
    }
    if t.entries.iter().any(|e| e.post >= n) || t.matches.iter().any(|&m| m >= n) || t.revealed.iter().any(|&m| m >= n) {
        fail(format!("an entry, match or revealed post past the {n} posts"));
    }
    if let Some(l) = &t.layout {
        let consistent = l.blocks.len() == t.entries.len()
            && l.starts.len() == l.blocks.len() + 1
            && l.starts.first() == Some(&0)
            && l.blocks.iter().zip(l.starts.windows(2)).all(|(b, s)| s[1] - s[0] == b.len());
        if !consistent {
            fail(format!("a layout of {} blocks for {} entries", l.blocks.len(), t.entries.len()));
        }
    }
}

/// With nothing in flight, nothing may still be loading or refreshing.
fn check_idle(app: &App, log: &[String]) {
    let stuck: Vec<String> = std::iter::once(&app.tab)
        .chain(app.tabs.iter().enumerate().filter(|&(i, _)| i != app.active).map(|(_, t)| t))
        .filter_map(|t| t.loading.clone())
        .collect();
    assert!(stuck.is_empty(), "still loading with nothing in flight: {stuck:?}\n{}", tail(log));
    assert!(app.refreshing.is_empty(), "refreshes stuck: {:?}\n{}", app.refreshing, tail(log));
    assert!(app.generals_searching.is_empty(), "general searches stuck\n{}", tail(log));
    assert!(app.boards_refreshing.is_empty(), "board list refreshes stuck\n{}", tail(log));
}

#[test]
fn fuzz_app() {
    fuzz::run("fuzz_app", false, 1, 4, |seed| episode(seed, 250));
}

#[test]
#[ignore]
fn fuzz_app_long() {
    let steps = std::env::var("FUZZ_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(2_000);
    fuzz::run("fuzz_app", true, 0, 200, |seed| episode(seed, steps));
}
