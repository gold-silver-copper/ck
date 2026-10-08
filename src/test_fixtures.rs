//! What the tests share: apps over known configs, posts, and ways to drive and draw an app.

use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Line;

use crate::app::{App, Clock, MenuItem, Whole};
use crate::config::Config;
use crate::keys::KeyMap;
use crate::markup::{Flavor, parse_html};
use crate::model::{Attachment, Post, Thread};
use crate::store::Store;

/// 2026-09-21 14:13:20 UTC: the snapshots' clock.
pub const NOW: i64 = 1_790_000_000;

// ----- apps -----

/// An app over `config` (TOML), with an empty data directory, that never writes the real
/// config file.
pub fn app_with(config: &str) -> App {
    let cfg: Config = toml::from_str(config).unwrap();
    let filters = crate::filter::Filters::from_config(&cfg.filters, &cfg.hidden_words).unwrap();
    let mut app = App::new(cfg, KeyMap::default(), filters, None, Store::default());
    app.config_path = None;
    app
}

/// An app over the default config; nothing here touches the network.
pub fn test_app() -> App {
    app_with(crate::config::DEFAULT_CONFIG)
}

/// Two sites on hosts that refuse connections: nothing leaves the machine. (Their own
/// port, so the requests don't take rate-limit slots other tests' hosts need.)
pub fn local_app() -> App {
    app_with(
        "[[site]]\nname = \"a\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"x\", \"xy\"]\n\
         [[site]]\nname = \"b\"\nkind = \"vichan\"\nurl = \"http://localhost:3\"\nboards = [\"y\"]",
    )
}

/// The local app with a data directory, the clock fixed at `now`.
pub fn saving_app(dir: &std::path::Path, now: i64) -> App {
    let mut app = local_app();
    app.store = Store::load(Some(dir.to_path_buf())).0;
    app.clock = Clock { fixed: Some(now), ..Default::default() };
    app
}

// ----- posts -----

/// Posts numbered `nos`, each saying so.
pub fn nos(nos: &[u64]) -> Vec<Post> {
    nos.iter().map(|&no| Post { no, body: vec![Line::from(format!("post {no}"))], ..Default::default() }).collect()
}

/// Posts as a whole thread to keep, under their first post's number.
pub fn whole(posts: &[Post]) -> Whole {
    let no = posts.first().map_or(0, |p| p.no);
    Whole::assumed(Thread::answer(no, posts.to_vec()).unwrap())
}

/// Posts 1..=n, some taller than others.
pub fn posts_upto(n: u64) -> Vec<Post> {
    (1..=n).map(|no| Post { no, body: (0..1 + no % 4).map(|k| Line::from(format!("post {no} line {k}"))).collect(), ..Default::default() }).collect()
}

/// Posts by Anonymous, each one line.
pub fn posts_saying(list: &[(u64, &str)]) -> Vec<Post> {
    list.iter().map(|&(no, text)| Post { no, poster: "Anonymous".into(), body: vec![Line::from(text.to_string())], ..Default::default() }).collect()
}

/// A post with one PNG on the local app's first site.
pub fn with_file(no: u64, board: Option<&str>) -> Post {
    let file = Attachment { filename: format!("{no}.png"), thumb: Some(format!("http://127.0.0.1:3/thumb/{no}.png")), ..Attachment::at(format!("http://127.0.0.1:3/src/{no}.png")) };
    Post { no, files: vec![file], board: board.map(String::from), body: vec![Line::from("text")], ..Default::default() }
}

/// A post made `age` seconds before `NOW`, from 4chan HTML.
pub fn post(no: u64, age: i64, subject: Option<&str>, html: &str) -> Post {
    let p = parse_html(html, Flavor::Fourchan);
    Post {
        no,
        poster: "Anonymous".into(),
        subject: subject.map(String::from),
        time: NOW - age,
        ..p.into()
    }
}

/// A file on `i.example`, with its thumbnail.
pub fn file(name: &str) -> Attachment {
    Attachment {
        filename: name.into(),
        thumb: Some(format!("https://i.example/thumb/{name}")),
        width: Some(800),
        height: Some(600),
        size: Some(123_456),
        ..Attachment::at(format!("https://i.example/{name}"))
    }
}

// ----- drivers -----

/// Draw the app once at `w`x`h` (layouts are made by drawing); what was drawn.
pub fn draw_at(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::draw(f, app)).unwrap();
    term.backend().buffer().clone()
}

/// Type `text`, a key per character.
pub fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(KeyEvent::from(KeyCode::Char(c)));
    }
}

/// Run the menu row labeled `label`.
pub fn run_menu_row(app: &mut App, label: &str) {
    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    let m = app.menu_mut().expect("a menu");
    let labels: Vec<String> = m.items.iter().map(|it| match it {
        MenuItem::Enter(l) | MenuItem::Act(_, l) => l.clone(),
    }).collect();
    let i = labels.iter().position(|l| l == label).unwrap_or_else(|| panic!("no {label:?} in {labels:?}"));
    m.list.select(Some(i));
    app.on_key(KeyEvent::from(KeyCode::Enter));
}

/// Handle messages until nothing is loading (or `until` holds).
pub fn settle_until(app: &mut App, until: impl Fn(&App) -> bool) {
    for _ in 0..500 {
        if until(app) {
            return;
        }
        app.wait(Duration::from_millis(10));
    }
    panic!("never settled");
}

/// The selected post's first row on screen.
pub fn selected_row(app: &App) -> isize {
    let t = app.tab.thread.as_ref().unwrap();
    let l = t.layout.as_ref().unwrap();
    l.starts[t.entry()] as isize - t.scroll as isize
}
