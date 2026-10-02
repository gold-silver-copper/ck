//! Snapshot tests: every view rendered with fixed data and a fixed clock.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use crate::app::{App, Clock, Preview, SettingsPopup, ThreadView, View, Viewer};
use crate::config::Config;
use crate::images::Images;
use crate::keys::KeyMap;
use crate::markup::{Flavor, parse_html};
use crate::model::{Attachment, Board, Post};
use crate::store::{Store, ThreadKey, Visit, Watched};
use crate::theme::theme;

/// 2026-09-21 14:13:20 UTC.
const NOW: i64 = 1_790_000_000;
const HOUR: i64 = 3600;

fn app(images: bool) -> App {
    let cfg: Config = toml::from_str(crate::config::DEFAULT_CONFIG).unwrap();
    let mut store = Store::default();
    let key = |board: &str, no| ThreadKey { site: "4chan".into(), board: board.into(), no };
    store.watched = vec![
        Watched { key: key("g", 1000), subject: "Snapshot thread".into(), posts: 5, last_seen: 1002, unread: 2, dead: false, mine: Vec::new(), replies: 0 },
        Watched { key: key("g", 900), subject: "Old thread".into(), posts: 300, last_seen: 1199, unread: 0, dead: true, mine: Vec::new(), replies: 0 },
        Watched {
            key: ThreadKey { site: "lainchan".into(), board: "λ".into(), no: 42 },
            subject: "Programming Employment".into(),
            posts: 92,
            last_seen: 77,
            unread: 0,
            dead: false,
            mine: Vec::new(),
            replies: 0,
        },
    ];
    store.history = vec![
        Visit { key: key("g", 1000), subject: "Snapshot thread".into(), last_seen: 1004, opened: NOW - 120 },
        Visit { key: key("b", 5), subject: "Random thread".into(), last_seen: 9, opened: NOW - 30 * HOUR },
    ];
    let mut app = App::new(cfg, KeyMap::default(), None, store);
    app.clock = Clock { fixed: Some(NOW) };
    app.truecolor = true;
    // Settings changes must never reach the real config file.
    app.config_path = None;
    if images {
        app.images = Images::offline();
    }
    app.sites[0].boards = Some(vec![
        Board { uri: "a".into(), title: "Anime & Manga".into(), nsfw: Some(false) },
        Board { uri: "b".into(), title: "Random".into(), nsfw: Some(true) },
        Board { uri: "g".into(), title: "Technology".into(), nsfw: Some(false) },
    ]);
    app.board = Some(Board { uri: "g".into(), title: "Technology".into(), nsfw: Some(false) });
    app
}

fn post(no: u64, age: i64, subject: Option<&str>, html: &str) -> Post {
    let p = parse_html(html, Flavor::Fourchan);
    Post {
        no,
        name: "Anonymous".into(),
        subject: subject.map(String::from),
        time: NOW - age,
        body: p.lines,
        quotes: p.quotes,
        links: p.links,
        ..Default::default()
    }
}

fn file(name: &str) -> Attachment {
    Attachment {
        filename: name.into(),
        url: format!("https://i.example/{name}"),
        thumb: Some(format!("https://i.example/thumb/{name}")),
        spoiler: false,
        width: Some(800),
        height: Some(600),
        size: Some(123_456),
        md5: None,
    }
}

fn catalog() -> Vec<Post> {
    let mut sticky = post(1, 400 * 24 * HOUR, Some("Welcome to /g/"), "Read the rules before posting.");
    sticky.sticky = true;
    sticky.locked = true;
    sticky.files = vec![file("rules.png")];
    sticky.replies = Some(3);
    let mut lmg = post(1000, 5 * HOUR, Some("/lmg/ - Local Models General"), "Previous threads: &gt;&gt;900 &amp; &gt;&gt;800");
    lmg.files = vec![file("miku.jpg")];
    lmg.replies = Some(312);
    lmg.images = Some(58);
    let mut green = post(1100, 40 * 60, None, "<span class=\"quote\">&gt;be me</span><br><span class=\"quote\">&gt;write tests</span>");
    green.replies = Some(12);
    let mut rust = post(1200, 2 * 24 * HOUR, Some("Rust or C++?"), "Which one and why? <s>hidden answer</s>");
    rust.files = vec![file("crab.webm")];
    rust.replies = Some(45);
    rust.images = Some(3);
    vec![sticky, lmg, green, rust]
}

fn thread() -> ThreadView {
    let mut op = post(1000, 5 * HOUR, Some("Snapshot thread"), "A thread for snapshot tests.<br>It has a few replies.");
    op.files = vec![file("op.png")];
    let posts = vec![
        op,
        post(
            1001,
            4 * HOUR,
            None,
            "<a href=\"#p1000\" class=\"quotelink\">&gt;&gt;1000</a><br><span class=\"quote\">&gt;implying</span>",
        ),
        post(1002, 3 * HOUR, None, "Code:<br><pre class=\"prettyprint\">fn main() {<br>    println!(\"snapshot\");<br>}</pre>"),
        post(1003, 2 * HOUR, None, "<s>secret</s> and <a href=\"#p1001\" class=\"quotelink\">&gt;&gt;1001</a>"),
        post(1004, HOUR, None, "See <a href=\"/g/thread/900#p901\" class=\"quotelink\">&gt;&gt;901</a> in the old thread"),
    ];
    let mut t = ThreadView::new("g".into(), 1000, posts);
    t.new_after = 1002;
    t
}

fn render(app: &mut App) -> (String, Buffer) {
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..buf.area.height {
        let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
        text.push_str(row.trim_end());
        text.push('\n');
    }
    (text, buf)
}

/// Render, checking that nothing is drawn with box-drawing characters.
fn snapshot(app: &mut App) -> String {
    let text = render(app).0;
    let boxy: Vec<char> = text.chars().filter(|c| ('\u{2500}'..='\u{259f}').contains(c)).collect();
    assert!(boxy.is_empty(), "box drawing: {boxy:?}\n{text}");
    text
}

/// The frame's backgrounds, one letter per role: the layout of surfaces at a glance.
fn bg_map(app: &mut App) -> String {
    let buf = render(app).1;
    let t = theme();
    let legend = [
        (t.background, ' '),
        (t.bar, 'b'),
        (t.surface, 's'),
        (t.surface_high, 'h'),
        (t.surface_highest, 'H'),
        (t.selection, 'S'),
        (t.primary, 'p'),
        (t.primary_container, 'c'),
        (t.code_bg, 'k'),
        (t.search, '/'),
        (t.new, 'n'),
        (t.success, '+'),
        (t.error, '!'),
    ];
    let mut out = String::new();
    for y in 0..buf.area.height {
        let row: String = (0..buf.area.width)
            .map(|x| legend.iter().find(|(c, _)| *c == buf[(x, y)].bg).map_or('?', |&(_, l)| l))
            .collect();
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

#[test]
fn sites() {
    let mut a = app(false);
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn boards() {
    let mut a = app(false);
    a.view = View::Boards;
    a.board_list.state.select(Some(2));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn catalog_with_thumbnail_placeholders() {
    let mut a = app(true);
    a.view = View::Catalog;
    a.catalog = catalog();
    a.catalog_list.state.select(Some(1));
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("catalog_backgrounds", bg_map(&mut a));
    // Only what's on screen is asked for.
    assert!(a.images.queued() >= 2);
}

#[test]
fn catalog_compact() {
    let mut a = app(true);
    a.view = View::Catalog;
    a.compact = true;
    a.catalog = catalog();
    a.catalog_list.state.select(Some(0));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn thread_view() {
    let mut a = app(true);
    a.view = View::Thread;
    a.thread = Some(thread());
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("thread_backgrounds", bg_map(&mut a));
}

#[test]
fn help() {
    let mut a = app(false);
    a.show_help = true;
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn quote_preview() {
    let mut a = app(false);
    a.view = View::Thread;
    let mut t = thread();
    t.selected = 3;
    a.thread = Some(t);
    a.preview = Some(Preview { posts: vec![1], elsewhere: vec![], scroll: 0 });
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn search_highlight() {
    let mut a = app(false);
    a.view = View::Thread;
    let mut t = thread();
    t.set_search("Snapshot".into());
    a.thread = Some(t);
    let (text, buf) = render(&mut a);
    insta::assert_snapshot!(text);
    // Every visible "snapshot" (any case) in the thread is highlighted, and nothing else.
    let body = (buf.area.width * 2) as usize..buf.content().len();
    let highlighted: String = buf.content()[body].iter().filter(|c| c.bg == theme().search).map(|c| c.symbol()).collect();
    assert_eq!(highlighted.to_lowercase(), "snapshot".repeat(highlighted.len() / 8));
    assert!(highlighted.len() >= 16, "{highlighted}");
}

#[test]
fn watched() {
    let mut a = app(false);
    a.view = View::Watched;
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn history() {
    let mut a = app(false);
    a.view = View::History;
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn image_viewer_placeholder() {
    let mut a = app(true);
    a.view = View::Thread;
    a.thread = Some(thread());
    a.viewer = Some(Viewer { files: vec![file("op.png"), file("clip.webm")], index: 0, link: None });
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn settings() {
    let mut a = app(false);
    a.open_settings();
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("settings_backgrounds", bg_map(&mut a));
}

#[test]
fn theme_picker() {
    let mut a = app(false);
    a.open_settings();
    a.activate_setting();
    assert!(matches!(a.settings.popup, Some(SettingsPopup::Themes { .. })));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn color_editor() {
    let mut a = app(false);
    a.open_settings();
    a.settings_list.state.select(Some(1));
    a.activate_setting();
    assert!(matches!(a.settings.popup, Some(SettingsPopup::Colors { .. })));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn other_themes_and_256_colors() {
    // Every built-in theme draws a thread; in 256-color mode no 24-bit color is left.
    for (name, t) in crate::theme::BUILTIN {
        let mut a = app(false);
        a.view = View::Thread;
        a.thread = Some(thread());
        a.set_theme(*t);
        a.truecolor = false;
        let (_, buf) = render(&mut a);
        let rgb = buf.content().iter().any(|c| matches!(c.fg, ratatui::style::Color::Rgb(..)) || matches!(c.bg, ratatui::style::Color::Rgb(..)));
        assert!(!rgb, "{name}");
    }
}

#[test]
fn key_editor() {
    let mut a = app(false);
    a.open_settings();
    let keys = crate::app::SETTING_SECTIONS.iter().flat_map(|(_, i)| i.iter()).count() - 1;
    a.settings_list.state.select(Some(keys));
    a.activate_setting();
    insta::assert_snapshot!(snapshot(&mut a));
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Enter));
    insta::assert_snapshot!("key_editor_capturing", snapshot(&mut a));
}

#[test]
fn links_panel() {
    let mut a = app(false);
    a.view = View::Thread;
    let mut t = thread();
    t.selected = 4;
    t.posts[4].urls = vec!["https://example.com/a-long-path?with=query".into()];
    t.posts[4].files = vec![file("notes.pdf")];
    a.thread = Some(t);
    a.open_links();
    insta::assert_snapshot!(snapshot(&mut a));
}

fn with_filters(a: &mut App) {
    #[derive(serde::Deserialize)]
    struct C {
        filter: Vec<crate::filter::FilterConfig>,
    }
    let cfg = "[[filter]]\npattern = \"Rust\"\naction = \"highlight\"\nlabel = \"rust\"\n[[filter]]\npattern = \"implying\"\nlabel = \"no implying\"";
    a.filters = crate::filter::Filters::new(&toml::from_str::<C>(cfg).unwrap().filter).unwrap();
}

#[test]
fn filtered_catalog_and_thread() {
    let mut a = app(true);
    with_filters(&mut a);
    a.view = View::Catalog;
    a.catalog = catalog();
    a.store.toggle_hidden("4chan", "g", 1100);
    a.remark_catalog();
    a.catalog_list.state.select(Some(0));
    insta::assert_snapshot!(snapshot(&mut a));
    // Z: hidden ones shown, marked.
    a.show_hidden = true;
    insta::assert_snapshot!("filtered_catalog_shown", snapshot(&mut a));
    insta::assert_snapshot!("filtered_catalog_backgrounds", bg_map(&mut a));
    a.show_hidden = false;
    a.view = View::Thread;
    a.thread = Some(thread());
    a.remark_thread();
    insta::assert_snapshot!("filtered_thread", snapshot(&mut a));
}

#[test]
fn your_posts_and_replies() {
    let mut a = app(false);
    a.view = View::Thread;
    a.thread = Some(thread());
    a.thread.as_mut().unwrap().mine.insert(1001);
    a.store.watched[0].replies = 1;
    insta::assert_snapshot!(snapshot(&mut a));
    a.view = View::Watched;
    insta::assert_snapshot!("watched_with_replies", snapshot(&mut a));
}
