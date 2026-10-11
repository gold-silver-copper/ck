//! The app under random input: keys, clicks, pastes, resizes, time passing and restarts,
//! against fake sites whose answers the fuzzer hands out in any order (or as errors). After
//! every step it draws a frame and checks what must always hold; when nothing is left in
//! flight it checks that nothing is stuck loading. Nothing leaves the machine: the fake
//! sites answer from generated data, and their file URLs are on `fuzz.invalid`, which
//! `http` refuses in tests.
//!
//! `cargo test fuzz_app` runs a short pass; `cargo test --profile fuzz -- --ignored
//! fuzz_app_long --nocapture` a long one (`FUZZ_SEED`, `FUZZ_RUNS` or `FUZZ_SECS`,
//! `FUZZ_STEPS`, `FUZZ_SHRINK=0` to skip shrinking a failure, and `FUZZ_TRACE=1` to print
//! the app's state after every step).

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use super::*;
use ratatui::crossterm::event::MouseEventKind;
use crate::backend::{Partial, SearchPage, ThreadPages};
use crate::fuzz::{self, Rng};
use crate::markup;
use crate::http::{HttpError, lock};
use crate::model::{Attachment, FileKind};

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
    /// Background calls so far: what was asked, and the (virtual) time it was asked.
    background: Vec<(String, Instant)>,
    /// The fuzzer's clock, for stamping calls.
    now: Option<Instant>,
    /// Calls waiting to answer: (ticket, what was asked).
    waiting: Vec<(u64, String)>,
    released: HashSet<u64>,
    /// Calls answering right now (released, not yet returned).
    running: usize,
    /// Everything answers at once (the episode is over).
    open: bool,
    /// Every call so far, in order.
    log: Vec<String>,
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
        if crate::http::is_background() {
            // When the app decided on it: its thread can get here a few steps later.
            let now = crate::http::asked_at().or(s.now).unwrap_or_else(Instant::now);
            s.background.push((what.clone(), now));
        }
        s.log.push(what.clone());
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

    /// Calls asked since the log had `from` entries.
    fn log_since(&self, from: usize) -> Vec<String> {
        lock(&self.state).log.get(from..).unwrap_or_default().to_vec()
    }

    fn log_len(&self) -> usize {
        lock(&self.state).log.len()
    }

    fn set_now(&self, now: Instant) {
        lock(&self.state).now = Some(now);
    }

    /// Wait until no new call has come in for a moment (spawned work has asked).
    fn quiet(&self) {
        let mut last = self.calls();
        loop {
            std::thread::sleep(Duration::from_millis(3));
            let now = self.calls();
            if now == last {
                return;
            }
            last = now;
        }
    }

    /// The background calls so far, forgetting them (a restarted app starts afresh).
    fn take_background(&self) -> Vec<(String, Instant)> {
        std::mem::take(&mut lock(&self.state).background)
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
            filename: if rng.chance(10) { fuzz::html(rng, 3) } else { name },
            thumb: rng.chance(80).then(|| format!("https://fuzz.invalid/{}/{board}/thumb/{n}s.jpg", self.site)),
            spoiler: rng.chance(10),
            width: rng.chance(70).then(|| rng.below(5000) as u32),
            height: rng.chance(70).then(|| rng.below(5000) as u32),
            size: rng.chance(70).then(|| rng.below(1 << 30) as u64),
            md5: rng.chance(30).then(|| format!("{:x}", rng.next())),
            kind: FileKind::of(None, Some(&url), ""),
            // Sometimes the archive kept only the thumbnail.
            url: (!rng.chance(5)).then_some(url),
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
            poster: crate::model::Poster::new(
                (!rng.chance(90)).then(|| fuzz::html(rng, 2)),
                "Anonymous",
                no.is_multiple_of(11).then(|| "!!trip".into()),
                no.is_multiple_of(13).then(|| "mod".into()),
            ),
            subject: rng.chance(30).then(|| fuzz::html(rng, 3)),
            time: 1_790_000_000 - rng.below(400 * 86_400) as i64,
            files: (0..files).map(|k| self.file(rng, board, no * 10 + k as u64)).collect(),
            replies: rng.chance(90).then(|| rng.below(800) as u32),
            images: rng.chance(90).then(|| rng.below(200) as u32),
            sticky: rng.chance(5),
            locked: rng.chance(5),
            bumplimit: rng.chance(5),
            board: rng.chance(10).then(|| rng.pick(BOARD_NAMES).to_string()),
            // A few posters, by number (taking nothing from `rng`, so seeds replay as before).
            id: (!no.is_multiple_of(7)).then(|| format!("id{}", no % 4)),
            flag: no.is_multiple_of(3).then(|| crate::model::Flag { code: "US".into(), name: "United States".into() }),
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

    fn thread_unchecked(&self, board: &str, no: u64) -> Result<Vec<Post>> {
        let _pass = self.gate.enter(format!("{} thread /{board}/{no}", self.site));
        let generation = {
            let mut f = lock(&self.fetched);
            let g = f.entry((board.to_string(), no)).or_default();
            *g += 1;
            *g
        };
        let mut rng = Rng::new(hash(&[&self.seed, &self.site, &board, &no, &generation]));
        // Some threads die after a few fetches, for good (archived or deleted).
        let mut fate = Rng::new(hash(&[&self.seed, &self.site, &board, &no, &"dies"]));
        if fate.chance(25) && generation >= 2 + fate.below(4) as u64 {
            return Err(HttpError::NotFound(format!("{board}/{no}")).into());
        }
        if let Some(e) = self.fail(&mut rng, board) {
            return Err(e);
        }
        if rng.chance(3) {
            return Ok(Vec::new());
        }
        // Another thread, as a site in trouble might answer.
        if rng.chance(2) {
            return Ok(vec![self.post(&mut rng, board, no.saturating_add(1), &[])]);
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

    fn thread_pages(&self, board: &str) -> Result<Option<ThreadPages>> {
        let _pass = self.gate.enter(format!("{} pages /{board}/", self.site));
        let mut rng = self.rng(&format!("pages {board}"));
        if let Some(e) = self.fail(&mut rng, board) {
            return Err(e);
        }
        if rng.chance(20) {
            return Ok(None);
        }
        // Numbers like the catalog's, some of them twice.
        let mut pages = ThreadPages::default();
        for page in 1..=rng.below(11) as u32 {
            for _ in 0..rng.below(16) {
                pages.page.entry(1000 + rng.below(60) as u64 * 7 + rng.below(5) as u64).or_insert(page);
            }
            pages.of = page;
        }
        Ok(Some(pages))
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

/// A real backend talking to a fake site that answers badly now and then, through the gate.
struct Gated {
    data: Arc<dyn Backend>,
    urls: Arc<dyn Backend>,
    site: String,
    gate: Arc<Gate>,
}

impl Backend for Gated {
    fn boards(&self, partial: Partial<Board>) -> Result<Vec<Board>> {
        let _pass = self.gate.enter(format!("{} boards", self.site));
        self.data.boards(partial)
    }

    fn catalog(&self, board: &str, partial: Partial<Post>) -> Result<Vec<Post>> {
        // From the copies kept: no request to hold back.
        if crate::http::from_copies_only() {
            return self.data.catalog(board, partial);
        }
        let _pass = self.gate.enter(format!("{} catalog /{board}/", self.site));
        self.data.catalog(board, partial)
    }

    fn thread_unchecked(&self, board: &str, no: u64) -> Result<Vec<Post>> {
        if crate::http::from_copies_only() {
            return self.data.thread_unchecked(board, no);
        }
        let _pass = self.gate.enter(format!("{} thread /{board}/{no}", self.site));
        self.data.thread_unchecked(board, no)
    }

    fn find_thread(&self, board: &str, post: u64) -> Result<Option<u64>> {
        let _pass = self.gate.enter(format!("{} find /{board}/{post}", self.site));
        self.data.find_thread(board, post)
    }

    fn thread_pages(&self, board: &str) -> Result<Option<ThreadPages>> {
        let _pass = self.gate.enter(format!("{} pages /{board}/", self.site));
        self.data.thread_pages(board)
    }

    fn search(&self, board: &str, query: &str, page: u32) -> Result<SearchPage> {
        let _pass = self.gate.enter(format!("{} search /{board}/ {query:?} {page}", self.site));
        self.data.search(board, query, page)
    }

    fn board_url(&self, board: &str) -> String {
        self.urls.board_url(board)
    }

    fn thread_url(&self, board: &str, no: u64) -> String {
        self.urls.thread_url(board, no)
    }

    fn post_url(&self, board: &str, thread: u64, post: u64) -> String {
        self.urls.post_url(board, thread, post)
    }
}

// ----- an episode: steps, the world they act on, and what must hold -----

const FILTERS: &str = "[[filter]]\npattern = \"(?i)word\"\nlabel = \"word\"\n\
    [[filter]]\npattern = \"日本\"\naction = \"highlight\"\n\
    [[filter]]\npattern = \"(OP)\"\nfield = \"subject\"\naction = \"hide\"\n\
    [[filter]]\npattern = \"^.{1,8}$\"\nfield = \"subject\"\nrecursive = true\nboards = [\"b\", \"λ\"]\n\
    [[filter]]\npattern = \"^id[12]$\"\nfield = [\"id\", \"flag\", \"tripcode\"]\nreply = true\nnotify = true\n\
    [[filter]]\npattern = \">1MB\"\nfield = \"filesize\"\naction = \"highlight\"\nop = true\ntop = true\nnotify = true\n\
    [[filter]]\npattern = \"[02468]$\"\nfield = [\"postno\", \"dimensions\", \"capcode\"]\naction = \"highlight\"\ntop = true\n";

/// The wall clock the fuzzer starts from.
const START: i64 = 1_790_000_000;

/// One step. Choices that depend on the app's state (which call to answer, which row) are
/// raw numbers taken modulo what's there, so any subset of steps still replays.
#[derive(Clone, Debug)]
enum Act {
    Key(KeyEvent),
    Mouse(MouseEventKind, u16, u16, bool),
    Paste(String),
    Goto(String),
    Resize(u16, u16),
    Answer(usize),
    AnswerAll,
    Wait(Duration),
    Restart(u64),
    Pick(usize),
    /// Open the Saved view and a copy in it.
    Saved(usize),
    /// `X` on what's selected: candidate #k, hide or highlight, a scope, and enter.
    Filter(usize, u8),
    /// Settings › Filters, and keys in it.
    FilterList(u64),
    /// `j` until the end of the thread (drawing each time): it must get there.
    ReadToEnd,
    /// In a thread, preview a post's quotes, then let the thread refresh (the sites drop
    /// posts now and then) and draw with the preview still open.
    PreviewRefresh,
}

impl std::fmt::Display for Act {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Act::Key(k) => write!(f, "key {}", crate::keys::Key::from(*k)),
            Act::Mouse(kind, x, y, double) => write!(f, "mouse {kind:?} at {x},{y}{}", if *double { " (double)" } else { "" }),
            Act::Paste(t) => write!(f, "paste {t:?}"),
            Act::Goto(t) => write!(f, "goto {t:?}"),
            Act::Resize(w, h) => write!(f, "resize {w}x{h}"),
            Act::Answer(k) => write!(f, "answer #{k}"),
            Act::AnswerAll => write!(f, "answer everything"),
            Act::Wait(d) => write!(f, "wait {d:?}"),
            Act::Restart(_) => write!(f, "restart"),
            Act::Pick(k) => write!(f, "pick row #{k} and enter"),
            Act::Saved(k) => write!(f, "open saved copy #{k}"),
            Act::Filter(k, opts) => write!(f, "filter like this: candidate #{k}, options {opts:03b}"),
            Act::FilterList(_) => write!(f, "keys in Settings › Filters"),
            Act::ReadToEnd => write!(f, "j to the end of the thread"),
            Act::PreviewRefresh => write!(f, "preview quotes, then refresh"),
        }
    }
}

/// Keys that do something somewhere, so they come up often.
fn hot_keys() -> Vec<KeyEvent> {
    let mut keys: Vec<KeyEvent> = crate::keys::KeyMap::default().every_key().collect();
    keys.push(KeyEvent::from(KeyCode::Delete));
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
        "endchan/b/{n}",
        "zzzchan/b",
        "",
        "  ",
        "日本",
        "saved",
        "watched",
        "history",
    ];
    let s = rng.pick(&forms).replace("{n}", &(1 + rng.below(3000)).to_string()).replace("{m}", &(1 + rng.below(3000)).to_string());
    if rng.chance(15) { fuzz::html(rng, 3) } else { s }
}

/// A random step (the same seed makes the same steps, whatever the app does).
fn random_act(rng: &mut Rng, hot: &[KeyEvent]) -> Act {
    match rng.below(100) {
        0..40 => Act::Key(*rng.pick(hot)),
        40..48 => Act::Key(KeyEvent::from(KeyCode::Char(char::from(32 + rng.below(95) as u8)))),
        48..50 => {
            let c = char::from_u32(rng.below(0x3000) as u32).unwrap_or('?');
            Act::Key(KeyEvent::new(KeyCode::Char(c), *rng.pick(&[KeyModifiers::NONE, KeyModifiers::CONTROL, KeyModifiers::ALT])))
        }
        50..68 => Act::Answer(rng.below(1000)),
        68..71 => Act::AnswerAll,
        71..79 => {
            let kind = *rng.pick(&[
                MouseEventKind::Down(MouseButton::Left),
                MouseEventKind::Down(MouseButton::Left),
                MouseEventKind::ScrollDown,
                MouseEventKind::ScrollUp,
                MouseEventKind::Down(MouseButton::Right),
            ]);
            Act::Mouse(kind, rng.below(250) as u16, rng.below(80) as u16, rng.chance(30))
        }
        79..81 => Act::Paste(place(rng)),
        81..83 => Act::Goto(place(rng)),
        83..87 => {
            let (w, h) = if rng.chance(30) { (1 + rng.below(250) as u16, 1 + rng.below(80) as u16) } else { *rng.pick(SIZES) };
            Act::Resize(w, h)
        }
        87..92 => Act::Wait(Duration::from_secs(*rng.pick(&[1, 3, 9, 11, 30, 61, 300, 3600]))),
        92..93 => Act::Restart(rng.next()),
        93..95 => Act::Saved(rng.below(1000)),
        95..97 => Act::Filter(rng.below(1000), rng.below(8) as u8),
        97..98 => Act::FilterList(rng.next()),
        98..99 => if rng.chance(50) { Act::ReadToEnd } else { Act::PreviewRefresh },
        _ => Act::Pick(rng.below(1000)),
    }
}

/// Everything an episode acts on.
struct World {
    dir: tempfile::TempDir,
    gate: Arc<Gate>,
    app: App,
    size: (u16, u16),
    /// The virtual clock: when the episode started, and how far it's got.
    start: Instant,
    elapsed: Duration,
    /// Sites answer with real backends over mangled fixtures (else from generated data).
    real: bool,
    seed: u64,
    hosts: Vec<String>,
}

impl World {
    fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let real = rng.chance(50);
        let mut w = World {
            dir: tempfile::tempdir().unwrap(),
            gate: Arc::new(Gate::default()),
            app: crate::test_fixtures::test_app(),
            size: (110, 32),
            start: Instant::now(),
            elapsed: Duration::ZERO,
            real,
            seed,
            hosts: Vec::new(),
        };
        w.app = w.boot(rng.next());
        w
    }

    fn now(&self) -> Instant {
        self.start + self.elapsed
    }

    /// An app on the fake sites, with its config and data in the world's directory.
    fn boot(&mut self, seed: u64) -> App {
        let mut rng = Rng::new(seed);
        let dir = self.dir.path();
        let mut doc: toml_edit::DocumentMut = crate::config::DEFAULT_CONFIG.parse().unwrap();
        if rng.chance(50) {
            doc["favorites"] = toml_edit::value(toml_edit::Array::from_iter(["4chan/g", "lainchan/λ", "nosuch/x", "4chan/b"]));
        }
        if rng.chance(30) {
            doc["hidden_sites"] = toml_edit::value(toml_edit::Array::from_iter(["kissu", "8kun"]));
        }
        if rng.chance(40) {
            doc["nsfw_images"] = toml_edit::value("off");
        }
        if rng.chance(30) {
            doc["recursive_hiding"] = toml_edit::value(true);
        }
        if rng.chance(30) {
            doc["refresh_backoff"] = toml_edit::value(false);
        }
        if rng.chance(40) {
            doc["watched_first"] = toml_edit::value(true);
        }
        if rng.chance(30) {
            doc["hidden_words"] = toml_edit::value(toml_edit::Array::from_iter(["the", "c++", "free money", "λ"]));
        }
        let mut text = doc.to_string();
        if rng.chance(50) {
            text.push_str(FILTERS);
        }
        let cfg: Config = toml::from_str(&text).unwrap();
        std::fs::write(dir.join("config.toml"), &text).unwrap();
        let mut store = Store::load(Some(dir.join("data"))).0;
        // Some boards with images off (or on) of their own.
        for board in ["4chan/g", "4chan/b", "lainchan/λ", "zzzchan/b", "4chan/all"] {
            if rng.chance(25) {
                store.board_prefs.entry(board.into()).or_default().images = Some(rng.chance(30));
            }
        }
        let filters = crate::filter::Filters::from_config(&cfg.filters, &cfg.hidden_words).unwrap();
        let mut app = App::new(cfg, filters, None, store);
        app.config_path = Some(dir.join("config.toml"));
        app.download_dir = Some(dir.join("downloads").display().to_string());
        app.pages = Some(crate::pages::Pages::new(dir.join("cache/pages"), 4 << 20));
        app.images = Images::offline();
        for site in &mut app.sites {
            let urls = site.backend.clone();
            let name = site.cfg.name.clone();
            site.backend = if self.real {
                let kind = match site.cfg.kind {
                    crate::config::SiteKind::Fourchan => crate::config::SiteKind::Vichan,
                    k => k,
                };
                let host = format!("{name}-{}.fuzz.invalid", self.seed).replace(['.', ' '], "-").replace("-fuzz-invalid", ".fuzz.invalid");
                if !self.hosts.contains(&host) {
                    crate::http::serve_test_host(&host, Some(fuzz::fake_site(kind, rng.next(), 30)));
                    self.hosts.push(host.clone());
                }
                Arc::new(Gated { data: fuzz::backend_at(kind, &host), urls, site: name, gate: self.gate.clone() })
            } else {
                Arc::new(Fake { real: urls, site: name, seed, gate: self.gate.clone(), fetched: Mutex::default(), calls: AtomicU64::new(0) })
            };
        }
        app
    }

    fn tick(&mut self, by: Duration) {
        self.elapsed += by;
        let now = self.now();
        self.app.clock = Clock { fixed: Some(START + self.elapsed.as_secs() as i64), instant: Some(now) };
        self.gate.set_now(now);
    }

    fn apply(&mut self, act: &Act) {
        let (w, h) = self.size;
        let app = &mut self.app;
        // Input goes through `handle`, as the terminal's does, so a click after a key in one
        // batch finds the frame stale as it would.
        let input = |app: &mut App, ev| app.handle(Msg::Input(ev));
        match act {
            Act::Key(k) => input(app, Event::Key(*k)),
            Act::Mouse(kind, x, y, double) => {
                let ev = MouseEvent { kind: *kind, column: x % w, row: y % h, modifiers: KeyModifiers::NONE };
                input(app, Event::Mouse(ev));
                if *double {
                    input(app, Event::Mouse(ev));
                }
            }
            Act::Paste(t) => input(app, Event::Paste(t.clone())),
            Act::Goto(t) => {
                input(app, Event::Key(KeyEvent::from(KeyCode::Char(':'))));
                if app.goto_text().is_some() {
                    input(app, Event::Paste(t.clone()));
                    input(app, Event::Key(KeyEvent::from(KeyCode::Enter)));
                }
            }
            Act::Resize(nw, nh) => {
                self.size = (*nw, *nh);
                input(app, Event::Resize(*nw, *nh));
            }
            Act::Answer(k) => {
                let waiting = self.gate.waiting();
                if let Some((ticket, _)) = waiting.get(k % waiting.len().max(1)) {
                    self.gate.release(*ticket);
                    drain(&mut self.app);
                }
            }
            Act::AnswerAll => settle(&mut self.app, &self.gate),
            Act::Wait(d) => {
                // Whatever was asked, was asked before the time passes.
                self.gate.quiet();
                self.tick(*d);
            }
            Act::Restart(seed) => {
                settle(&mut self.app, &self.gate);
                check_etiquette(&self.gate.take_background());
                // As quitting does.
                self.app.flush_writes();
                self.app.save_now();
                // A gate of its own for the new app: anything the old one still asks just goes.
                self.gate.open();
                self.gate = Arc::new(Gate::default());
                self.gate.set_now(self.now());
                self.app = self.boot(*seed);
                let now = self.now();
                self.app.clock = Clock { fixed: Some(START + self.elapsed.as_secs() as i64), instant: Some(now) };
            }
            Act::Pick(k) => {
                let view = app.tab.view();
                app.pick_row(view, k % (app.row_keys(view).len() + 1));
                app.on_key(KeyEvent::from(KeyCode::Enter));
            }
            Act::Filter(k, opts) => {
                // (A popup already open was made from a post that may since have been replaced.)
                let fresh = app.filter_add().is_none();
                app.act(crate::keys::Action::Filter);
                let Some((n, post)) = app.filter_add().map(|a| (a.candidates.len(), a.post)) else { return };
                for _ in 0..k % n.max(1) {
                    app.on_key(KeyEvent::from(KeyCode::Char('j')));
                }
                for (bit, key) in [(1, 'a'), (2, 's'), (4, 's')] {
                    if opts & bit != 0 {
                        app.on_key(KeyEvent::from(KeyCode::Char(key)));
                    }
                }
                let before = app.filter_cfgs.len();
                app.on_key(KeyEvent::from(KeyCode::Enter));
                // A filter made from a post catches that post, at once.
                if fresh && app.filter_cfgs.len() == before + 1 {
                    let at = match app.tab.view() {
                        View::Thread => app.tab.thread.as_ref().and_then(|t| Some((&t.marks, *t.index.get(&post)?))),
                        _ => app.tab.catalog.posts().iter().position(|p| p.no == post).map(|i| (&app.tab.catalog.marks, i)),
                    };
                    let caught = at.is_some_and(|(m, i)| m.why_hidden(i).is_some() || m.highlight(i).is_some());
                    assert!(caught, "the filter {:?} added from post {post} doesn't catch it", app.filter_cfgs.last());
                }
            }
            Act::ReadToEnd => {
                let reading = |app: &App| app.tab.view() == View::Thread && app.modal_open().is_none() && app.tab.thread.is_some();
                if !reading(app) {
                    return;
                }
                // Each j moves on (the selection, or the scroll within a tall post) until the
                // end: it can't go round in circles.
                let mut last = None;
                for _ in 0..100_000 {
                    draw(app, w, h);
                    let Some(t) = app.tab.thread.as_ref().filter(|_| reading(app)) else { return };
                    if t.entry() + 1 >= t.entries.len() && t.tall().is_none_or(|(_, _, below)| !below) {
                        return;
                    }
                    let at = (t.entry(), t.scroll);
                    assert!(last.is_none_or(|l| at > l), "j didn't move on: entry {} at line {} (was {last:?})", at.0, at.1);
                    last = Some(at);
                    app.on_key(KeyEvent::from(KeyCode::Char('j')));
                }
                panic!("j never reached the end of the thread");
            }
            Act::PreviewRefresh => {
                if app.tab.view() != View::Thread || app.modal_open().is_some() {
                    return;
                }
                if let Some(t) = &mut app.tab.thread
                    && let Some(i) = t.posts.iter().rposition(|p| !p.quotes.is_empty())
                {
                    t.select(i);
                }
                app.act(Action::Preview);
                self.tick(Duration::from_secs(61));
                settle(&mut self.app, &self.gate);
                draw(&mut self.app, w, h);
            }
            Act::FilterList(seed) => {
                let mut rng = Rng::new(*seed);
                app.on_key(KeyEvent::from(KeyCode::Char(',')));
                if app.tab.view() != View::Settings || app.settings_popup().is_some() {
                    return;
                }
                app.pick_row(View::Settings, settings::position("Filters").unwrap());
                app.on_key(KeyEvent::from(KeyCode::Enter));
                let keys = [' ', 'x', 'a', 'j', 'k', 'l', 'h', '(', 'w', '|', '日'];
                for _ in 0..rng.below(16) {
                    let key = if rng.chance(25) { *rng.pick(&[KeyCode::Enter, KeyCode::Esc, KeyCode::Backspace, KeyCode::Down]) } else { KeyCode::Char(*rng.pick(&keys)) };
                    app.on_key(KeyEvent::from(key));
                }
            }
            Act::Saved(k) => {
                app.on_key(KeyEvent::from(KeyCode::Char(':')));
                if app.goto_text().is_some() {
                    app.paste("saved");
                    app.on_key(KeyEvent::from(KeyCode::Enter));
                }
                let len = app.row_keys(View::Saved).len();
                if app.tab.view() == View::Saved && len > 0 {
                    app.pick_row(View::Saved, k % len);
                    app.on_key(KeyEvent::from(KeyCode::Enter));
                }
            }
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        self.gate.open();
        for host in &self.hosts {
            crate::http::serve_test_host(host, None);
        }
    }
}

/// Handle what's arrived, until a moment passes with nothing new.
fn drain(app: &mut App) {
    while app.wait(Duration::from_millis(3)) {}
}

/// Answer every call (and every call those answers lead to) until nothing is waiting.
fn settle(app: &mut App, gate: &Gate) {
    for _ in 0..50 {
        drain(app);
        gate.quiet();
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
    let buf = crate::test_fixtures::draw_at(app, w, h);
    // Nothing that reaches the terminal may be a control character.
    if let Some(cell) = buf.content().iter().find(|c| c.symbol().chars().any(char::is_control)) {
        panic!("a control character drawn: {:?}", cell.symbol());
    }
}

// ----- panics anywhere, and where they happened -----

static WORKER_PANICS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Where this thread last panicked, to tell one failure from another while shrinking.
    static PANICKED_AT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    static QUIET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Count panics on worker threads (unnamed: the app's and the fake sites'), note where test
/// threads panic, and keep quiet while shrinking. Tests of what a panic does panic on worker
/// threads on purpose, with a message starting "deliberate"; those don't count.
fn watch_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let deliberate = info.payload().downcast_ref::<&str>().is_some_and(|m| m.starts_with("deliberate"));
            if std::thread::current().name().is_none() && !deliberate {
                WORKER_PANICS.fetch_add(1, Ordering::SeqCst);
            }
            let at = info.location().map(|l| format!("{}:{}", l.file(), l.line()));
            PANICKED_AT.with(|p| *p.borrow_mut() = at);
            if !QUIET.with(std::cell::Cell::get) {
                prev(info);
            }
        }));
    });
}

/// Run `acts` in a fresh world. `Err` holds where it failed and the message.
fn replay(seed: u64, acts: &[Act]) -> Result<(), (String, String)> {
    let mut world = World::new(seed);
    let workers = WORKER_PANICS.load(Ordering::SeqCst);
    let mut log: Vec<String> = Vec::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        world.tick(Duration::ZERO);
        let mut removed: HashSet<ThreadKey> = HashSet::new();
        for (step, act) in acts.iter().enumerate() {
            let (gate, calls_before) = (world.gate.clone(), world.gate.calls());
            let before = Before::of(&world.app, &world.gate);
            log.push(format!("{step}: {act}"));
            world.apply(act);
            world.tick(Duration::from_millis(100));
            world.app.poll();
            world.app.quit = false;
            let (w, h) = world.size;
            let asked_before = world.app.images.queued_urls();
            draw(&mut world.app, w, h);
            check_images(&world.app, &asked_before);
            if std::env::var_os("FUZZ_TRACE").is_some() {
                let a = &world.app;
                let hid = a.store.hidden_count();
                let m = a.tab.catalog.marks.hidden_count();
                eprintln!("TRACE {step} {act}: tab {} view {:?} site {} board {:?} cat_board {} cat {} hidden-marks {m} store {hid:?} filters {}", a.active, a.tab.view(), a.current_site().cfg.name, a.tab.board.as_ref().map(|b| &b.uri), a.tab.catalog.board().map_or("", |b| b.uri.as_str()), a.tab.catalog.posts().len(), a.filter_cfgs.len());
                let w: Vec<String> = a.store.all_watched().iter().map(|w| format!("{}/{}/{} dead={} seen={}", w.key.site, w.key.board, w.key.no, w.status.is_dead(), w.last_seen)).collect();
                let sv: Vec<String> = a.store.saved.iter().map(|m| format!("{}/{}/{}", m.key.site, m.key.board, m.key.no)).collect();
                eprintln!("TRACE   watched {w:?} saved {sv:?} status {:?}", a.footer.get().map(|s| &s.text));
                if let Some(t) = &a.tab.thread {
                    let l = t.layout.as_ref().map(|l| (l.starts.clone(), l.exact.clone()));
                    eprintln!("TRACE   thread entry {} selected {} scroll {} view {} entries {} focus {:?} conv {:?} layout {l:?}", t.entry(), t.selected, t.scroll, t.viewport, t.entries.len(), t.focus, t.conversation.as_ref().map(|c| c.anchor));
                }
            }
            check(&world.app);
            check_layout(&mut world.app);
            let calls = if Arc::ptr_eq(&gate, &world.gate) { world.gate.log_since(before.log) } else { Vec::new() };
            check_saved(&world.app, &before, &calls, &mut removed);
            check_marks(&world.app, &before);
            if !matches!(act, Act::Restart(_)) {
                check_deleted(&world.app, &before);
            }
            if matches!(act, Act::Answer(_) | Act::AnswerAll | Act::Wait(_)) {
                check_follow(&world.app, &before);
            }
            // A step asks a handful of things at most (a few refreshes may fall due at once).
            let asked = world.gate.calls() - if Arc::ptr_eq(&gate, &world.gate) { calls_before } else { 0 };
            assert!(asked <= 12, "{asked} requests from one step");
            assert_eq!(WORKER_PANICS.load(Ordering::SeqCst), workers, "a worker thread panicked");
        }
        settle(&mut world.app, &world.gate);
        let (w, h) = world.size;
        draw(&mut world.app, w, h);
        check(&world.app);
        check_idle(&world.app);
        check_etiquette(&world.gate.take_background());
        assert_eq!(WORKER_PANICS.load(Ordering::SeqCst), workers, "a worker thread panicked");
        // What was saved loads again, without complaint.
        world.app.flush_writes();
        world.app.save_now();
        let (_, warnings) = Store::load(Some(world.dir.path().join("data")));
        assert!(warnings.is_empty(), "the saved data doesn't load cleanly: {warnings:?}");
    }));
    result.map_err(|panic| {
        let msg = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
        let at = PANICKED_AT.with(|p| p.borrow().clone()).unwrap_or_default();
        if !QUIET.with(std::cell::Cell::get) {
            eprintln!("\nfuzz_app ({} sites): the steps before the failure:\n{}", if world.real { "real" } else { "generated" }, tail(&log));
        }
        (at, msg)
    })
}

fn tail(log: &[String]) -> String {
    log.iter().rev().take(40).rev().cloned().collect::<Vec<_>>().join("\n")
}

/// The fewest steps that still fail the same way (same place), within a time budget.
fn shrink(seed: u64, acts: Vec<Act>, at: &str, budget: Duration) -> Vec<Act> {
    let start = Instant::now();
    let fails = |acts: &[Act]| matches!(replay(seed, acts), Err((a, _)) if a == at);
    let mut acts = acts;
    let mut chunk = acts.len() / 2;
    while chunk >= 1 && start.elapsed() < budget {
        let mut i = 0;
        let mut removed = false;
        while i < acts.len() && start.elapsed() < budget {
            let end = (i + chunk).min(acts.len());
            let candidate: Vec<Act> = acts[..i].iter().chain(&acts[end..]).cloned().collect();
            if fails(&candidate) {
                acts = candidate;
                removed = true;
            } else {
                i += chunk;
            }
        }
        if !removed {
            chunk /= 2;
        }
    }
    acts
}

fn episode(seed: u64, steps: usize, shrinking: bool) {
    watch_panics();
    let hot = hot_keys();
    let mut rng = Rng::new(seed ^ 0x5eed);
    let acts: Vec<Act> = (0..steps).map(|_| random_act(&mut rng, &hot)).collect();
    let Err((at, msg)) = replay(seed, &acts) else { return };
    if shrinking {
        eprintln!("fuzz_app: failed at {at}; shrinking {} steps…", acts.len());
        QUIET.with(|q| q.set(true));
        let small = shrink(seed, acts, &at, Duration::from_secs(180));
        QUIET.with(|q| q.set(false));
        eprintln!("fuzz_app: {} steps still fail at {at}:", small.len());
        for (i, a) in small.iter().enumerate() {
            eprintln!("  {i}: {a}");
        }
    }
    panic!("{msg}");
}

// ----- what must always hold -----

fn check(app: &App) {
    let fail = |what: String| panic!("{what}");
    if app.tabs.is_empty() || app.tabs.len() > MAX_TABS || app.active >= app.tabs.len() {
        fail(format!("tabs: {} open, active {}", app.tabs.len(), app.active));
    }
    let tabs = std::iter::once(&app.tab).chain(app.tabs.iter().enumerate().filter(|&(i, _)| i != app.active).map(|(_, t)| t));
    for (i, tab) in tabs.enumerate() {
        if let Some(t) = &tab.thread
            && let Err(w) = check_thread(t)
        {
            fail(format!("tab {i}: {w}"));
        }
        // The focus is one of the selected post's parts.
        if let Some(t) = &tab.thread
            && let Some(f) = &t.focus
            && !t.parts_of(t.entry()).contains(f)
        {
            fail(format!("tab {i}: focus on {f:?}, not a part of post {}", t.selected));
        }
        // A thread place with nothing shown is loading, says why not, or `r` loads it.
        if tab.place_view() == View::Thread && tab.thread.is_none() && tab.loading().is_none() && tab.failed.is_none() && tab.pending_thread.is_none() {
            fail(format!("tab {i}: a thread with nothing shown and no way to load it"));
        }
        // A copy shown while loading is marked as one, and there's something to show.
        if tab.cached().is_some() && tab.thread.is_none() {
            fail(format!("tab {i}: marked cached with no thread"));
        }
        if tab.catalog.cached.is_some() && tab.catalog.posts().is_empty() {
            fail(format!("tab {i}: catalog marked cached with nothing in it"));
        }
        if let Some(v) = tab.viewer()
            && v.index >= v.files.len()
        {
            fail(format!("tab {i}: viewer on file {} of {}", v.index, v.files.len()));
        }
        if let Some(g) = &tab.gallery
            && (g.files.is_empty() || g.state.selected().is_some_and(|k| k >= g.files.len()))
        {
            fail(format!("tab {i}: gallery selection {:?} of {}", g.state.selected(), g.files.len()));
        }
        if let Some(crate::app::TabPopup::Links(l)) = &tab.popup
            && (l.items.is_empty() || l.list.selected().is_some_and(|k| k >= l.items.len()))
        {
            fail(format!("tab {i}: links selection {:?} of {}", l.list.selected(), l.items.len()));
        }
        // Marks left from an emptied catalog are harmless; a catalog's own must line up.
        if !tab.catalog.posts().is_empty() && tab.catalog.marks.len() != tab.catalog.posts().len() {
            fail(format!("tab {i}: {} catalog marks for {} threads", tab.catalog.marks.len(), tab.catalog.posts().len()));
        }
    }
    if let Some(m) = app.menu()
        && (m.items.is_empty() || m.list.selected().is_some_and(|k| k >= m.items.len()))
    {
        fail(format!("a menu of {} rows on row {:?}", m.items.len(), m.list.selected()));
    }
    // Hint labels: each one picks one target, so none starts another.
    if let Some(h) = app.hints() {
        let labels: Vec<&str> = h.targets.iter().map(|t| t.label.as_str()).collect();
        if labels.iter().enumerate().any(|(i, a)| labels.iter().enumerate().any(|(j, b)| i != j && b.starts_with(a))) {
            fail(format!("hint labels overlap: {labels:?}"));
        }
    }
    if app.settings_popup().is_some() && app.tab.view() != View::Settings {
        fail(format!("a settings popup in {:?}", app.tab.view()));
    }
    if let Some(p) = app.image_search_panel()
        && p.list.selected().is_some_and(|r| !matches!(p.rows.get(r), Some(Ok(_))))
    {
        fail(format!("image search on row {:?} of {}", p.list.selected(), p.rows.len()));
    }
    let unique = |keys: Vec<&ThreadKey>| keys.len() == keys.iter().collect::<HashSet<_>>().len();
    if !unique(app.store.all_watched().iter().map(|w| &w.key).collect()) {
        fail("a thread watched twice".into());
    }
    if !unique(app.store.history.iter().map(|v| &v.key).collect()) {
        fail("a thread twice in history".into());
    }
    // Quiet threads wait longer, but never under the refetch floor nor past the cap.
    let cap = |every: Duration| every.max(super::QUIET_MAX);
    let open = app.thread_every();
    if open < crate::http::MIN_REFETCH || open > cap(app.refresh_thread) {
        fail(format!("the open thread refreshes every {open:?}"));
    }
    for w in app.store.all_watched() {
        let every = app.watched_every(&w.key);
        if every < crate::http::MIN_REFETCH || every > cap(app.refresh_watched) {
            fail(format!("{:?} refreshes every {every:?}", w.key));
        }
    }
    // A thread's page is one of its board's.
    if let Some((at, p)) = app.board_pages.iter().find(|(_, p)| p.page.values().any(|&n| n == 0 || n > p.of)) {
        fail(format!("{at:?}: a thread on a page past the {} there are", p.of));
    }
    // The site's text never reaches the terminal's title as an escape.
    if let Some(t) = app.terminal_title().filter(|t| t.chars().any(char::is_control)) {
        fail(format!("a control character in the terminal's title: {t:?}"));
    }
}

fn check_thread(t: &ThreadView) -> Result<(), String> {
    let n = t.posts.len();
    if t.index.len() != n || t.posts.iter().enumerate().any(|(i, p)| t.index.get(&p.no) != Some(&i)) {
        return Err(format!("the post index doesn't match the {n} posts"));
    }
    if t.backlinks.len() != n {
        return Err(format!("{} backlink lists for {n} posts", t.backlinks.len()));
    }
    if n == 0 {
        return Err("a thread without posts".into());
    }
    if t.selected >= n {
        return Err(format!("post {} selected of {n}", t.selected));
    }
    // The cursor is a hint; `entry()` is the selected post's entry, whatever it says.
    if t.entries.get(t.entry()).is_none_or(|e| e.post != t.selected) {
        return Err(format!("entry {} of {} doesn't hold the selected post {}", t.entry(), t.entries.len(), t.selected));
    }
    if t.entries.iter().any(|e| e.post >= n) || t.matches.iter().any(|&m| m >= n) {
        return Err(format!("an entry or match past the {n} posts"));
    }
    // A conversation shows just its posts, all of them, its own post among them; without
    // one, every post is there, or (`M`) the OP and every post with files.
    let top: Vec<usize> = t.entries.iter().filter(|e| e.path.len() == 1).map(|e| e.post).collect();
    match &t.conversation {
        Some(c) => {
            let Some(&p) = t.index.get(&c.anchor) else { return Err(format!("a conversation of No.{}, which isn't in the thread", c.anchor)) };
            let set = match &c.poster {
                // A poster's posts: every post with the ID.
                Some(id) => t.posts_by(id),
                None => conversation_of(&t.posts, &t.index, &t.backlinks, p).0,
            };
            if top != set.keys().copied().collect::<Vec<_>>() {
                return Err(format!("the conversation of No.{} shows {top:?}, not {:?}", c.anchor, set.keys().collect::<Vec<_>>()));
            }
        }
        None => {
            let files = t.media == super::Media::Files;
            let want: Vec<usize> = (0..n).filter(|&i| !files || i == 0 || !t.posts[i].files.is_empty()).collect();
            if top != want {
                return Err(format!("the whole thread ({:?}) shows {} of its {} posts", t.media, top.len(), want.len()));
            }
        }
    }
    // A post hidden as a reply to a hidden post quotes it, and it's hidden; the OP never is.
    for (i, m) in t.marks.all_hidden().into_iter().enumerate() {
        let Some(crate::filter::Hidden::Reply(to)) = m else { continue };
        let parent = t.index.get(&to).and_then(|&k| t.marks.why_hidden(k));
        if i == 0 || t.posts.get(i).is_none_or(|p| !p.quotes.contains(&to)) || parent.is_none() {
            return Err(format!("post {i} is hidden as a reply to No.{to}, which it doesn't quote or isn't hidden"));
        }
    }
    if let Some(l) = &t.layout {
        let consistent = l.blocks.len() == t.entries.len()
            && l.starts.len() == l.blocks.len() + 1
            && l.starts.first() == Some(&0)
            && l.blocks.iter().zip(l.starts.windows(2)).all(|(b, s)| s[1] - s[0] == b.len());
        if !consistent {
            return Err(format!("a layout of {} blocks for {} entries", l.blocks.len(), t.entries.len()));
        }
    }
    Ok(())
}

/// What saved copies there were before a step.
struct Before {
    /// Each copy: whether it was of a dead thread nobody watches (which may be pruned).
    saved: HashMap<ThreadKey, bool>,
    /// In the Saved view, where `x` removes copies.
    in_saved_view: bool,
    /// The open tab's saved copy of a thread that's gone (and not watched alive).
    offline_dead: Option<(usize, ThreadKey)>,
    log: usize,
    filters: Vec<crate::filter::FilterConfig>,
    recursive_hiding: bool,
    /// The thread on screen: its tab, thread, post count, whether its end was being read,
    /// and the post at the top of the screen.
    reading: Option<(usize, String, u64, usize, bool, u64)>,
    /// The open tab's thread as fetched (not a copy): its board and number, its posts'
    /// numbers. (Its tab by place and the number of tabs: closing one moves the others.)
    live: Option<Live>,
}

/// The open tab's place and the number of tabs, its thread's board and number, its posts'
/// numbers.
type Live = ((usize, usize), String, u64, Vec<u64>);

impl Before {
    fn of(app: &App, gate: &Gate) -> Self {
        let watched_alive = |k: &ThreadKey| app.store.watched(k).is_some_and(|w| !w.status.is_dead());
        let saved = app.store.saved.iter().map(|m| (m.key.clone(), m.dead && app.store.watched(&m.key).is_none())).collect();
        let offline_dead = match (app.tab.saved(), &app.tab.thread) {
            (Some(o), Some(t)) if o.dead && app.tab.view() == View::Thread => {
                Some(t.key().clone()).filter(|k| !watched_alive(k)).map(|k| (app.active, k))
            }
            _ => None,
        };
        let shown = app.tab.view() == View::Thread && app.tab.gallery.is_none() && app.tab.viewer().is_none();
        let reading = app.tab.thread.as_ref().filter(|_| shown).and_then(|t| {
            let top = t.posts[t.entries.get(t.layout.as_ref()?.entry_at(t.scroll))?.post].no;
            Some((app.active, t.key().board.clone(), t.key().no, t.posts.len(), t.at_end(), top))
        });
        let live = app
            .tab
            .thread
            .as_ref()
            .filter(|_| app.tab.copy.is_none())
            .map(|t| ((app.active, app.tabs.len()), t.key().board.clone(), t.key().no, t.posts.iter().map(|p| p.no).collect()));
        Before { saved, in_saved_view: app.tab.view() == View::Saved, offline_dead, log: gate.log_len(), filters: app.filter_cfgs.clone(), recursive_hiding: app.hiding.recursive(), reading, live }
    }
}

/// Nothing drawn asked for a thumbnail of a post on a board with images off.
fn check_images(app: &App, before: &[String]) {
    let after = app.images.queued_urls();
    let asked: Vec<&String> = after.iter().filter(|u| !before.contains(u)).collect();
    if asked.is_empty() {
        return;
    }
    let off_thumbs: Vec<&str> = match app.tab.view() {
        View::Thread => app
            .tab
            .thread
            .as_ref()
            .filter(|_| !app.thread_images_on())
            .map(|t| t.posts.iter().flat_map(|p| p.files.iter().filter_map(|f| f.thumb.as_deref())).collect())
            .unwrap_or_default(),
        View::Catalog => app.tab.catalog.posts().iter().filter(|p| !app.catalog_images_on(p)).flat_map(|p| p.files.iter().filter_map(|f| f.thumb.as_deref())).collect(),
        _ => Vec::new(),
    };
    // A thumbnail shared with a post on a board that shows images can be asked for.
    let shared = |u: &str| match app.tab.view() {
        View::Catalog => app.tab.catalog.posts().iter().filter(|p| app.catalog_images_on(p)).any(|p| p.files.iter().any(|f| f.thumb.as_deref() == Some(u))),
        _ => false,
    };
    for u in asked {
        assert!(!off_thumbs.contains(&u.as_str()) || shared(u) || app.tab.viewer().is_some(), "{u} was asked for on a board with images off");
    }
}

/// A refresh that brought posts to the thread on screen: reading its end, the selected post
/// is on screen; reading elsewhere, the same post is at the top.
fn check_follow(app: &App, before: &Before) {
    let Some((tab, board, no, posts, at_end, top)) = &before.reading else { return };
    let shown = app.tab.view() == View::Thread && app.tab.gallery.is_none() && app.tab.viewer().is_none();
    let Some(t) = app.tab.thread.as_ref().filter(|t| shown && app.active == *tab && t.key().board == *board && t.key().no == *no && t.posts.len() > *posts) else { return };
    let Some(l) = &t.layout else { return };
    let e = t.entry();
    if *at_end && app.follow_new_posts {
        let (start, end) = (l.starts[e], l.starts[e + 1]);
        assert!(start < t.scroll + t.viewport && end > t.scroll, "following: the selected post (lines {start}..{end}) isn't on screen (scroll {}, {} rows)", t.scroll, t.viewport);
    } else if t.index.contains_key(top) {
        // The same post at the top (a post deleted by the refresh can't stay there), unless
        // keeping the selected post on screen moved it: posts above it can grow, with replies.
        let now = t.entries.get(l.entry_at(t.scroll)).map(|x| t.posts[x.post].no);
        let (start, end) = (l.starts[e], l.starts[e + 1]);
        let selected_shown = start < t.scroll + t.viewport && end > t.scroll;
        assert!(now == Some(*top) || selected_shown, "not following: the post at the top changed after a refresh, and the selected one isn't on screen (scroll {}, len {}, view {})", t.scroll, l.len(), t.viewport);
    }
}

/// The thread on screen is laid out where it's seen, and what's laid out is what a full
/// layout would give.
fn check_layout(app: &mut App) {
    let clock = app.clock;
    let drawn = app.tab.view() == View::Thread && app.tab.gallery.is_none() && app.tab.viewer().is_none();
    let Some(t) = app.tab.thread.as_mut().filter(|_| drawn) else { return };
    let Some((width, thumbs)) = t.layout.as_ref().map(|l| (l.width, l.thumbs_on)) else { return };
    let full = crate::ui::layout_all(t, width, thumbs, clock);
    let Some(l) = t.layout.as_ref() else { return };
    if let Some(e) = (0..l.blocks.len()).find(|&e| l.exact[e] && l.blocks[e] != full.blocks[e]) {
        let text = |b: &[ratatui::text::Line]| b.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>();
        let (have, want) = (text(&l.blocks[e]), text(&full.blocks[e]));
        let k = (0..have.len().max(want.len())).find(|&k| have.get(k) != want.get(k) || l.blocks[e].get(k) != full.blocks[e].get(k)).unwrap_or(0);
        panic!(
            "entry {e} laid out differently than a full layout would, at line {k}:\n  have {:?}\n  want {:?}\n  styles {:?}\n  vs     {:?}",
            have.get(k),
            want.get(k),
            l.blocks[e].get(k).map(|l| l.spans.iter().map(|s| s.style).collect::<Vec<_>>()),
            full.blocks[e].get(k).map(|l| l.spans.iter().map(|s| s.style).collect::<Vec<_>>())
        );
    }
    if l.blocks.is_empty() || t.viewport == 0 {
        return;
    }
    let (top, bottom) = (l.entry_at(t.scroll), l.entry_at(t.scroll + t.viewport - 1));
    if let Some(e) = (top..=bottom).find(|&e| !l.exact[e]) {
        panic!("entry {e} is on screen but not laid out (scroll {}, view {})", t.scroll, t.viewport);
    }
    if t.scroll > l.len().saturating_sub(t.viewport) {
        panic!("scrolled to line {} of {} (view {})", t.scroll, l.len(), t.viewport);
    }
    // The selected post is on screen, at least partly.
    let cursor = t.entry();
    if cursor < top || cursor > bottom {
        panic!("the selected entry {cursor} is off screen ({top}..={bottom} shown)");
    }
}

/// A post shown before a refresh of the same thread is still there after it: kept, or
/// marked deleted when the refresh left it out; unless the refresh came back too small to
/// trust. Deleted posts are never new, and the rest of the posts are what was fetched.
fn check_deleted(app: &App, before: &Before) {
    let Some(t) = &app.tab.thread else { return };
    if let Some(i) = (0..t.posts.len()).find(|&i| t.is_deleted(i) && t.is_new(i)) {
        panic!("No.{} is deleted and new", t.posts[i].no);
    }
    if t.deleted.iter().any(|no| !t.index.contains_key(no)) || t.deleted.contains(&t.key().no) {
        panic!("deleted posts {:?} aren't (reply) posts of the thread", t.deleted);
    }
    let Some((tab, board, no, nos)) = &before.live else { return };
    // (An answer cut short is shown as it came.)
    if (app.active, app.tabs.len()) != *tab || app.tab.copy.is_some() || t.key().board != *board || t.key().no != *no || crate::model::shrank(t.known, t.live_posts().len()) {
        return;
    }
    if let Some(lost) = nos.iter().find(|n| !t.index.contains_key(n)) {
        panic!("No.{lost} was shown before a refresh of /{board}/{no} and is gone after it, not marked deleted");
    }
}

/// After the filters change, the open catalog's and thread's marks are what they say.
fn check_marks(app: &App, before: &Before) {
    // Threads a `top` filter highlights come first, whatever the sort; then (`watched_first`)
    // the watched ones.
    if app.tab.catalog.marks.len() == app.tab.catalog.posts().len() {
        let firsts: Vec<(bool, bool)> = app
            .visible_catalog()
            .iter()
            .map(|&i| (app.tab.catalog.marks.top(i), app.watched_first && app.catalog_watching(&app.tab.catalog.posts()[i])))
            .collect();
        assert!(firsts.windows(2).all(|w| w[0] >= w[1]), "a top or watched thread after another (top, watched): {firsts:?}");
    }
    if app.filter_cfgs == before.filters && app.hiding.recursive() == before.recursive_hiding {
        return;
    }
    let tab = &app.tab;
    // Each marked for the site it's from (a search moves the tab to the archive).
    if !tab.catalog.posts().is_empty() {
        let want = app.catalog_marks(&tab.catalog, tab.catalog.posts());
        assert!(want == tab.catalog.marks, "catalog marks don't match the filters on {:?}: {:?}, not {:?}", tab.catalog.site(), tab.catalog.marks, want);
    }
    if let Some(t) = &tab.thread {
        assert!(app.thread_marks(t) == t.marks, "thread marks don't match the filters");
    }
    // And they're what's in the config file.
    if let Some(path) = &app.config_path {
        let cfg: Config = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(cfg.filters == app.filter_cfgs, "the config's filters aren't the ones in use");
    }
}

/// Saved copies are never lost, and reading one of a dead thread fetches nothing.
fn check_saved(app: &App, before: &Before, calls: &[String], removed: &mut HashSet<ThreadKey>) {
    for (key, prunable) in &before.saved {
        if app.store.saved(key).is_none() {
            // (A step can go back into the Saved view and press x twice there.)
            let in_saved = before.in_saved_view || app.tab.view() == View::Saved;
            assert!(in_saved || *prunable, "the saved copy of {key:?} vanished outside the Saved view");
            removed.insert(key.clone());
        }
    }
    for w in app.store.all_watched() {
        if w.status.is_dead() && w.last_seen > 0 && !removed.contains(&w.key) {
            assert!(app.store.saved(&w.key).is_some(), "watched thread {:?} died with no saved copy", w.key);
        }
    }
    if let Some((tab, key)) = &before.offline_dead
        && Before::of(app, &Gate::default()).offline_dead.as_ref().is_some_and(|(t, k)| t == tab && k == key)
    {
        let what = format!("{} thread /{}/{}", key.site, key.board, key.no);
        assert!(!calls.contains(&what), "fetched {what} while reading its saved copy");
    }
}

/// With nothing in flight, nothing may still be loading or refreshing.
fn check_idle(app: &App) {
    let stuck: Vec<String> = std::iter::once(&app.tab)
        .chain(app.tabs.iter().enumerate().filter(|&(i, _)| i != app.active).map(|(_, t)| t))
        .filter_map(|t| t.loading().map(String::from))
        .collect();
    assert!(stuck.is_empty(), "still loading with nothing in flight: {stuck:?}");
    assert!(app.refreshing.is_empty(), "refreshes stuck: {:?}", app.refreshing);
    assert!(app.generals_searching.is_empty(), "general searches stuck");
    assert!(app.boards_refreshing.is_empty(), "board list refreshes stuck");
    assert!(app.pages_asking.is_empty(), "thread pages asked for stuck");
}

/// Background requests for the same thing are never closer together than the refetch floor
/// (on the virtual clock), whatever the refresh settings were changed to meanwhile.
fn check_etiquette(calls: &[(String, Instant)]) {
    let min = crate::http::MIN_REFETCH;
    let mut last: HashMap<&str, Instant> = HashMap::new();
    for (what, at) in calls {
        if let Some(prev) = last.insert(what, *at) {
            let gap = at.saturating_duration_since(prev);
            let all: Vec<String> = calls.iter().map(|(w, t)| format!("{w} at {:?}", t.saturating_duration_since(calls[0].1))).collect();
            assert!(gap >= min, "{what} asked again in the background after {gap:?} (at least {min:?}); all: {all:?}");
        }
    }
}

#[test]
fn fuzz_app() {
    fuzz::run("fuzz_app", false, 1, 4, |seed| episode(seed, 250, false));
}

#[test]
#[ignore = "long fuzz run"]
fn fuzz_app_long() {
    let steps = std::env::var("FUZZ_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(2_000);
    let shrinking = std::env::var("FUZZ_SHRINK").map_or(true, |v| v != "0");
    fuzz::run("fuzz_app", true, 0, 200, |seed| episode(seed, steps, shrinking));
}

/// Found by fuzzing: two tabs on one thread, the first with more posts; closing it brings
/// the second to its place, which isn't a refresh that lost posts.
#[test]
fn closing_a_tab_isnt_a_refresh() {
    let mut app = crate::test_fixtures::local_app();
    let open = |app: &mut App, n: u64| {
        app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
        app.tab.navigate(View::Thread);
        app.set_thread(crate::test_fixtures::posts_upto(n));
    };
    open(&mut app, 5);
    app.tabs.push(Tab::new(0, Instant::now()));
    app.switch_tab(1);
    open(&mut app, 2);
    app.switch_tab(0);
    let before = Before::of(&app, &Gate::default());
    app.close_tab();
    assert_eq!((app.active, app.tab.thread.as_ref().unwrap().posts.len()), (0, 2));
    check_deleted(&app, &before);
}
