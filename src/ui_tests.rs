//! Snapshot tests: every view rendered with fixed data and a fixed clock.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use crate::app::{App, Clock, Preview, SettingsPopup, ThreadView, View, Viewer};
use crate::images::Images;
use crate::markup::{Flavor, parse_html};
use crate::model::{Attachment, Board, Post};
use crate::store::{ThreadKey, Visit, Watched};
use crate::theme::theme;

/// 2026-09-21 14:13:20 UTC.
const NOW: i64 = 1_790_000_000;
const HOUR: i64 = 3600;

fn app(images: bool) -> App {
    let mut app = crate::app::tests::test_app();
    let key = |board: &str, no| ThreadKey { site: "4chan".into(), board: board.into(), no };
    app.store.watched = vec![
        Watched { key: key("g", 1000), subject: "Snapshot thread".into(), posts: 5, last_seen: 1002, unread: 2, ..Default::default() },
        Watched { key: key("g", 900), subject: "Old thread".into(), posts: 300, last_seen: 1199, dead: true, ..Default::default() },
        Watched {
            key: ThreadKey { site: "lainchan".into(), board: "λ".into(), no: 42 },
            subject: "Programming Employment".into(),
            posts: 92,
            last_seen: 77,
            ..Default::default()
        },
    ];
    app.store.history = vec![
        Visit { key: key("g", 1000), subject: "Snapshot thread".into(), last_seen: 1004, opened: NOW - 120 },
        Visit { key: key("b", 5), subject: "Random thread".into(), last_seen: 9, opened: NOW - 30 * HOUR },
    ];
    app.clock = Clock { fixed: Some(NOW) };
    app.truecolor = true;
    if images {
        app.images = Images::offline();
    }
    app.sites[0].boards = Some(vec![
        Board { uri: "a".into(), title: "Anime & Manga".into(), nsfw: Some(false) },
        Board { uri: "b".into(), title: "Random".into(), nsfw: Some(true) },
        Board { uri: "g".into(), title: "Technology".into(), nsfw: Some(false) },
    ]);
    app.tab.board = Some(Board { uri: "g".into(), title: "Technology".into(), nsfw: Some(false) });
    app
}

fn post(no: u64, age: i64, subject: Option<&str>, html: &str) -> Post {
    let p = parse_html(html, Flavor::Fourchan);
    Post {
        no,
        name: "Anonymous".into(),
        subject: subject.map(String::from),
        time: NOW - age,
        ..p.into()
    }
}

fn file(name: &str) -> Attachment {
    Attachment {
        filename: name.into(),
        url: format!("https://i.example/{name}"),
        thumb: Some(format!("https://i.example/thumb/{name}")),
        width: Some(800),
        height: Some(600),
        size: Some(123_456),
        ..Default::default()
    }
}

/// The app on the thread fixture.
fn thread_app(images: bool) -> App {
    let mut a = app(images);
    a.tab.view = View::Thread;
    a.tab.thread = Some(thread());
    a
}

/// The app on the catalog fixture.
fn catalog_app(images: bool) -> App {
    let mut a = app(images);
    a.tab.view = View::Catalog;
    a.tab.catalog = catalog();
    a
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
    render_at(app, 100, 30)
}

fn render_at(app: &mut App, w: u16, h: u16) -> (String, Buffer) {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
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
    // Box drawing (U+2500-257F); block elements like the input cursor are fine.
    let boxy: Vec<char> = text.chars().filter(|c| ('\u{2500}'..='\u{257f}').contains(c)).collect();
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
    a.tab.view = View::Boards;
    a.tab.board_list.state.select(Some(2));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn catalog_with_thumbnail_placeholders() {
    let mut a = catalog_app(true);
    a.tab.catalog_list.state.select(Some(1));
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("catalog_backgrounds", bg_map(&mut a));
    // Only what's on screen is asked for.
    assert!(a.images.queued() >= 2);
}

#[test]
fn catalog_compact() {
    let mut a = app(true);
    a.tab.view = View::Catalog;
    a.default_layout = crate::config::CatalogLayout::Compact;
    a.tab.catalog = catalog();
    a.tab.catalog_list.state.select(Some(0));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn thread_view() {
    let mut a = thread_app(true);
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
    a.tab.view = View::Thread;
    let mut t = thread();
    t.selected = 3;
    a.tab.thread = Some(t);
    a.tab.preview = Some(Preview { posts: vec![1], elsewhere: vec![], scroll: 0 });
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn search_highlight() {
    let mut a = app(false);
    a.tab.view = View::Thread;
    let mut t = thread();
    t.set_search("Snapshot".into());
    a.tab.thread = Some(t);
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
    a.tab.view = View::Watched;
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn history() {
    let mut a = app(false);
    a.tab.view = View::History;
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn image_viewer_placeholder() {
    let mut a = thread_app(true);
    a.tab.viewer = Some(Viewer { files: vec![file("op.png"), file("clip.webm")], index: 0, link: None });
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
    assert!(matches!(a.settings_popup, Some(SettingsPopup::Themes { .. })));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn color_editor() {
    let mut a = app(false);
    a.open_settings();
    a.settings_list.state.select(Some(1));
    a.activate_setting();
    assert!(matches!(a.settings_popup, Some(SettingsPopup::Colors { .. })));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn other_themes_and_256_colors() {
    // Every built-in theme draws a thread; in 256-color mode no 24-bit color is left.
    for (name, t) in crate::theme::BUILTIN {
        let mut a = thread_app(false);
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
    a.tab.view = View::Thread;
    let mut t = thread();
    t.selected = 4;
    t.posts[4].urls = vec!["https://example.com/a-long-path?with=query".into()];
    t.posts[4].files = vec![file("notes.pdf")];
    a.tab.thread = Some(t);
    a.open_links();
    insta::assert_snapshot!(snapshot(&mut a));
}

fn with_filters(a: &mut App) {
    let cfg = "[[filter]]\npattern = \"Rust\"\naction = \"highlight\"\nlabel = \"rust\"\n[[filter]]\npattern = \"implying\"\nlabel = \"no implying\"";
    a.filters = crate::filter::tests::filters(cfg).unwrap();
}

#[test]
fn filtered_catalog_and_thread() {
    let mut a = app(true);
    with_filters(&mut a);
    a.tab.view = View::Catalog;
    a.tab.catalog = catalog();
    a.store.toggle_hidden("4chan", "g", 1100);
    a.remark_catalog();
    a.tab.catalog_list.state.select(Some(0));
    insta::assert_snapshot!(snapshot(&mut a));
    // Z: hidden ones shown, marked.
    a.show_hidden = true;
    insta::assert_snapshot!("filtered_catalog_shown", snapshot(&mut a));
    insta::assert_snapshot!("filtered_catalog_backgrounds", bg_map(&mut a));
    a.show_hidden = false;
    a.tab.view = View::Thread;
    a.tab.thread = Some(thread());
    a.remark_thread();
    insta::assert_snapshot!("filtered_thread", snapshot(&mut a));
}

#[test]
fn your_posts_and_replies() {
    let mut a = thread_app(false);
    a.tab.thread.as_mut().unwrap().mine.insert(1001);
    a.store.watched[0].replies = 1;
    insta::assert_snapshot!(snapshot(&mut a));
    a.tab.view = View::Watched;
    insta::assert_snapshot!("watched_with_replies", snapshot(&mut a));
}

#[test]
fn catalog_new_threads_and_replies() {
    let mut a = catalog_app(false);
    a.tab.catalog_new.insert(1100);
    a.store.opened("4chan", "g", 1000, 300, NOW);
    a.tab.catalog_list.state.select(Some(1));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn replies_inline() {
    let mut a = thread_app(true);
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('e')));
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('j')));
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('e')));
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("replies_inline_backgrounds", bg_map(&mut a));
}

#[test]
fn catalog_grid() {
    let mut a = app(true);
    a.tab.view = View::Catalog;
    a.default_layout = crate::config::CatalogLayout::Grid;
    a.tab.catalog = catalog();
    a.tab.catalog_list.state.select(Some(1));
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("catalog_grid_backgrounds", bg_map(&mut a));
    assert_eq!(a.grid_cols, 4);
    // Without images, the grid is drawn as cards.
    let mut b = app(false);
    b.tab.view = View::Catalog;
    b.default_layout = crate::config::CatalogLayout::Grid;
    b.tab.catalog = catalog();
    let text = snapshot(&mut b);
    assert!(text.contains("312 replies") && b.grid_cols == 0, "{text}");
}

#[test]
fn gallery() {
    let mut a = app(true);
    a.tab.view = View::Thread;
    let mut t = thread();
    t.posts[2].files = vec![file("code.png"), file("clip.webm")];
    a.tab.thread = Some(t);
    a.open_gallery();
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('l')));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn archive_search_results() {
    let mut a = app(false);
    let v = crate::backend::fixture("foolfuuka_search.json");
    a.tab.site = a.sites.iter().position(|s| s.cfg.name == "desuarchive").unwrap();
    a.search_input = Some("borrow".into());
    a.tab.view = View::Catalog;
    insta::assert_snapshot!("archive_search_typing", snapshot(&mut a));
    a.search_input = None;
    a.tab.view = View::Search;
    let page = crate::backend::foolfuuka::parse_search(&v).unwrap();
    a.tab.search = Some(crate::app::Search::for_tests("g", "rust borrow checker", page));
    a.tab.search_list.state.select(Some(0));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn image_search_panel() {
    let mut a = app(false);
    a.tab.view = View::Thread;
    let mut t = thread();
    t.posts[0].files.push(file("second.jpg"));
    a.tab.thread = Some(t);
    a.open_image_search();
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn tabs_row() {
    let mut a = catalog_app(false);
    a.tab.catalog_list.state.select(Some(1));
    a.new_tab();
    a.tab.thread = Some(thread());
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("tabs_row_backgrounds", bg_map(&mut a));
}


#[test]
fn help_fits_at_110x32_and_scrolls_when_small() {
    let mut a = app(false);
    a.show_help = true;
    let (text, _) = render_at(&mut a, 110, 32);
    // Two columns, everything on screen.
    for line in ["Everywhere", "Home screen", "Image viewer", "Catalog", "Thread", "mark as yours", "copy file URL / post link", "watch / quote tab / general", "favorite this board"] {
        assert!(text.contains(line), "{line} missing:\n{text}");
    }
    let (text, _) = render_at(&mut a, 60, 20);
    assert!(text.contains("Everywhere") && !text.contains("mark as yours"), "{text}");
    a.help_scroll = 100;
    let (text, _) = render_at(&mut a, 60, 20);
    assert!(text.contains("copy text / link"), "{text}");
}

#[test]
fn narrow_screens() {
    let mut a = app(false);
    // Settings scroll to the selected one.
    a.open_settings();
    let last = crate::app::SETTING_SECTIONS.iter().flat_map(|(_, i)| i.iter()).count() - 1;
    a.settings_list.state.select(Some(last));
    let (text, _) = render_at(&mut a, 60, 20);
    assert!(text.contains("Key bindings"), "{text}");
    // Long crumbs give way with an ellipsis, before the counts.
    a.tab.view = View::Thread;
    let mut t = thread();
    t.posts[0].subject = Some("A very long subject that can't possibly fit in a narrow terminal".into());
    a.tab.thread = Some(t);
    let (text, _) = render_at(&mut a, 60, 20);
    let bar = text.lines().next().unwrap();
    assert!(bar.contains("…  ") && bar.ends_with("5 posts  ·  2 new"), "{bar}");
    // Panels keep their title when the hint can't fit too.
    a.open_image_search();
    let (text, _) = render_at(&mut a, 40, 20);
    assert!(text.contains("Search for this image") && !text.contains("imageenter"), "{text}");
}

#[test]
fn home_with_favorites() {
    let mut a = app(false);
    a.favorites = vec![crate::app::BoardRef::parse("4chan/g").unwrap(), crate::app::BoardRef::parse("lainchan/λ").unwrap()];
    a.home_titles.insert("4chan/g".into(), "Technology".into());
    a.home_titles.insert("4chan/a".into(), "Anime & Manga".into());
    a.store.recent_boards = vec!["4chan/g".into(), "4chan/a".into()];
    // As at a start with these favorites and recent boards.
    a.load_home_titles();
    for name in ["wizchan", "uboachan", "endchan", "kohlchan", "zzzchan", "2ch", "smuglo.li", "kissu", "tvch", "sushigirl"] {
        a.hidden_sites.insert(name.into());
    }
    a.site_list.state.select(Some(2));
    insta::assert_snapshot!(snapshot(&mut a));
    a.show_hidden_sites = true;
    a.site_list.state.select(Some(8));
    insta::assert_snapshot!("home_showing_hidden_sites", snapshot(&mut a));
}

#[test]
fn watched_generals() {
    let mut a = app(false);
    a.tab.view = View::Watched;
    a.store.watched[0].general = Some("/lmg/".into());
    a.store.watched[0].at_limit = true;
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn thread_lines_are_cached_but_never_stale() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    let blocks = |a: &App| a.tab.thread.as_ref().unwrap().layout.as_ref().unwrap().blocks.clone();
    render(&mut a);
    let first = blocks(&a);
    // Nothing changed: the same lines, not laid out again.
    a.tab.thread.as_mut().unwrap().layout = None;
    render(&mut a);
    assert!(first.iter().zip(blocks(&a)).all(|(x, y)| std::rc::Rc::ptr_eq(x, &y)));
    // A search re-lays out only what it highlights, including text added to quotes ("(OP)").
    a.tab.thread.as_mut().unwrap().set_search("(op)".into());
    let (text, buf) = render(&mut a);
    let hl: String = buf.content().iter().filter(|c| c.bg == theme().search).map(|c| c.symbol()).collect();
    assert_eq!(hl, "(OP)", "{text}");
    let now = blocks(&a);
    assert!(std::rc::Rc::ptr_eq(&first[0], &now[0]) && !std::rc::Rc::ptr_eq(&first[1], &now[1]));
    a.tab.thread.as_mut().unwrap().set_search(String::new());
    // Spoilers shown on one post.
    a.tab.thread.as_mut().unwrap().selected = 3;
    a.on_key(KeyEvent::from(KeyCode::Char('s')));
    let (text, _) = render(&mut a);
    assert!(text.contains("secret and"), "{text}");
    // Times move on.
    a.clock = Clock { fixed: Some(NOW + 3 * HOUR) };
    a.tab.thread.as_mut().unwrap().layout = None;
    let (text, _) = render(&mut a);
    assert!(text.contains("7h ago") && !text.contains("4h ago"), "{text}");
    // A refresh keeps unchanged posts and redoes those with new replies.
    let before = blocks(&a);
    let mut posts = thread().posts;
    posts.push(crate::model::Post { no: 1005, quotes: vec![1003], time: NOW, ..Default::default() });
    let mut t = crate::app::ThreadView::new("g".into(), 1000, posts);
    t.cache = std::mem::take(&mut a.tab.thread.as_mut().unwrap().cache);
    a.tab.thread = Some(t);
    render(&mut a);
    let after = blocks(&a);
    assert!(std::rc::Rc::ptr_eq(&before[1], &after[1]) && !std::rc::Rc::ptr_eq(&before[3], &after[3]));
    // A new theme lays everything out again.
    a.set_theme(crate::theme::BUILTIN[1].1);
    render(&mut a);
    assert!(!std::rc::Rc::ptr_eq(&after[1], &blocks(&a)[1]));
}
