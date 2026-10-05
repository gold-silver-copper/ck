//! Snapshot tests: every view rendered with fixed data and a fixed clock.

use ratatui::buffer::Buffer;

use crate::app::{App, Clock, Part, Popup, Preview, SettingsPopup, ThreadView, View, Viewer};
use crate::images::Images;
use crate::model::{Board, Post};
use crate::store::{ThreadKey, Visit, Watched};
use crate::test_fixtures::*;
use crate::theme::theme;

const HOUR: i64 = 3600;

fn app(images: bool) -> App {
    let mut app = test_app();
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
    app.clock = Clock { fixed: Some(NOW), ..Default::default() };
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
    let buf = draw_at(app, w, h);
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
    let mut a = catalog_app(true);
    a.default_layout = crate::config::CatalogLayout::Compact;
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
    a.popup = Some(Popup::Help(0));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn quote_preview() {
    let mut a = thread_app(false);
    let t = a.tab.thread.as_mut().unwrap();
    t.selected = 3;
    a.tab.popup = Some(crate::app::TabPopup::Preview(Preview { posts: vec![1001], elsewhere: vec![], scroll: 0 }));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn search_highlight() {
    let mut a = thread_app(false);
    let t = a.tab.thread.as_mut().unwrap();
    t.set_search("Snapshot".into());
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

/// Saved copies: a watched one, a dead one, an exported one.
fn with_saved(a: &mut App) {
    use crate::saved::SavedMeta;
    let key = |board: &str, no| ThreadKey { site: "4chan".into(), board: board.into(), no };
    let meta = |key, subject: &str, saved, dead, posts| SavedMeta { key, subject: subject.into(), saved, dead, bytes: 40_000, posts, newest: 0, hash: 0 };
    a.store.saved = vec![
        meta(key("g", 1000), "Snapshot thread", NOW - 60, false, 5),
        meta(key("g", 900), "Old thread", NOW - 5 * HOUR, true, 300),
        meta(key("b", 5), "Random thread", NOW - 50 * HOUR, false, 9),
    ];
}

#[test]
fn saved() {
    let mut a = app(false);
    with_saved(&mut a);
    a.tab.view = View::Saved;
    insta::assert_snapshot!(snapshot(&mut a));
    // The home screen counts them.
    a.tab.view = View::Sites;
    assert!(snapshot(&mut a).contains("Saved           3 threads, 1 gone from the site"));
}

#[test]
fn saved_dead_thread() {
    let mut a = thread_app(false);
    with_saved(&mut a);
    a.tab.offline = Some(crate::app::Offline { saved: NOW - 5 * HOUR, dead: true });
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn thread_gone_offers_the_saved_copy() {
    let mut a = app(false);
    with_saved(&mut a);
    a.tab.view = View::Thread;
    a.thread_gone(&ThreadKey { site: "4chan".into(), board: "g".into(), no: 900 });
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn add_filter_popup() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    let t = a.tab.thread.as_mut().unwrap();
    t.posts[0].name = "Named !Trip".into();
    t.posts[0].files[0].md5 = Some("u8Vh17KxaDvUJ6bBcmE/eg==".into());
    a.on_key(KeyEvent::from(KeyCode::Char('X')));
    a.on_key(KeyEvent::from(KeyCode::Char('s')));
    insta::assert_snapshot!(snapshot(&mut a));
}

fn some_filters() -> Vec<crate::filter::FilterConfig> {
    let cfgs = "[[filter]]\npattern = \"(?i)crypto|nft\"\nlabel = \"crypto\"\n\
        [[filter]]\npattern = \"^Named !Trip$\"\nfield = \"name\"\naction = \"highlight\"\nlabel = \"Named !Trip\"\nsites = [\"4chan\"]\nboards = [\"g\"]\n\
        [[filter]]\npattern = \"implying\"\nfield = \"comment\"\nenabled = false\n";
    #[derive(serde::Deserialize)]
    struct C {
        filter: Vec<crate::filter::FilterConfig>,
    }
    toml::from_str::<C>(cfgs).unwrap().filter
}

#[test]
fn filter_list() {
    let mut a = thread_app(false);
    a.filter_cfgs = some_filters();
    a.tab.thread.as_mut().unwrap().posts[2].name = "Named !Trip".into();
    a.open_settings();
    a.popup = Some(Popup::Settings(a.filter_list(1)));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn filter_editor() {
    let mut a = thread_app(false);
    a.filter_cfgs = some_filters();
    a.open_settings();
    let draft = a.filter_cfgs[1].clone();
    a.popup = Some(Popup::Settings(SettingsPopup::FilterEdit { index: Some(1), draft, row: 0, typing: Some("^Named (!Trip".into()) }));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn conversation() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    a.tab.thread.as_mut().unwrap().select(1);
    a.on_key(KeyEvent::from(KeyCode::Char('c')));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn thread_from_its_last_copy() {
    let mut a = thread_app(false);
    a.tab.cached = Some(crate::app::Offline { saved: NOW - 3 * 60, dead: false });
    a.tab.loading = Some("Loading thread 1000".into());
    insta::assert_snapshot!(snapshot(&mut a));
}

/// A long thread: posts of different lengths, every one numbered in its text.
fn long_thread_app(n: u64) -> App {
    let mut a = app(false);
    a.tab.view = View::Thread;
    let posts: Vec<Post> = (0..n).map(|k| post(2000 + k, HOUR, None, &format!("post {k}<br>{}", "and a line<br>".repeat((k % 7) as usize)))).collect();
    a.tab.thread = Some(ThreadView::new("g".into(), 2000, posts));
    a
}

/// Laid out entries are what a full layout gives them, and everything on screen is laid out.
fn assert_layout_exact(a: &mut App) {
    let clock = a.clock;
    let t = a.tab.thread.as_mut().unwrap();
    let (width, thumbs) = t.layout.as_ref().map(|l| (l.width, l.thumbs_on)).unwrap();
    let full = crate::ui::layout_all(t, width, thumbs, clock);
    let l = t.layout.as_ref().unwrap();
    for e in (0..l.blocks.len()).filter(|&e| l.exact[e]) {
        assert_eq!(l.blocks[e], full.blocks[e], "entry {e}");
    }
    let (top, bottom) = (l.entry_at(t.scroll), l.entry_at(t.scroll + t.viewport - 1));
    assert!((top..=bottom).all(|e| l.exact[e]), "not laid out on screen: {top}..={bottom}");
}

#[test]
fn long_threads_are_laid_out_near_the_view_only() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = long_thread_app(400);
    render(&mut a);
    let laid_out = |a: &App| a.tab.thread.as_ref().unwrap().layout.as_ref().unwrap().exact.iter().filter(|&&x| x).count();
    assert!(laid_out(&a) < 40, "{}", laid_out(&a));
    assert_layout_exact(&mut a);
    // G: the last post, placed exactly and shown whole.
    a.on_key(KeyEvent::from(KeyCode::Char('G')));
    let (text, _) = render(&mut a);
    assert!(text.contains("post 399") && text.contains("No.2399"), "{text}");
    assert_layout_exact(&mut a);
    insta::assert_snapshot!("long_thread_end", snapshot(&mut a));
    // Scrolling up lays out what comes into view; coming back shows the same screen (the
    // selection may have moved, kept on screen while scrolling: the same apart from it).
    let unselected = |a: &mut App| render(a).0.replace('▌', " ").lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
    let before = unselected(&mut a);
    for _ in 0..40 {
        a.on_key(KeyEvent::from(KeyCode::Char('K')));
        render(&mut a);
    }
    for _ in 0..40 {
        a.on_key(KeyEvent::from(KeyCode::Char('J')));
        render(&mut a);
    }
    assert_eq!(unselected(&mut a), before);
    // A jump to a far post lands on it.
    let t = a.tab.thread.as_mut().unwrap();
    assert!(t.jump_to(2150));
    let (text, _) = render(&mut a);
    assert!(text.contains("No.2150"), "{text}");
    assert_layout_exact(&mut a);
    // A click on a post's line selects that post.
    let row = text.lines().position(|l| l.contains("No.2151")).unwrap() as u16;
    a.on_mouse(ratatui::crossterm::event::MouseEvent { kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left), column: 10, row, modifiers: ratatui::crossterm::event::KeyModifiers::NONE }, std::time::Instant::now());
    assert_eq!(a.tab.thread.as_ref().unwrap().current().unwrap().no, 2151);
    // A resize keeps the selection on screen.
    let (text, _) = render_at(&mut a, 70, 20);
    assert!(text.contains("No.2151"), "{text}");
    assert_layout_exact(&mut a);
    // Search: matches are found everywhere, and n goes to them exactly.
    a.tab.thread.as_mut().unwrap().set_search("post 37".into());
    assert_eq!(a.tab.thread.as_ref().unwrap().matches.len(), 11);
    a.on_key(KeyEvent::from(KeyCode::Char('n')));
    let (text, _) = render(&mut a);
    let no = a.tab.thread.as_ref().unwrap().current().unwrap().no;
    assert!(text.contains(&format!("No.{no}")), "{text}");
    // Reading on through everything, the estimates all turn exact, and agree with a full
    // layout to the line.
    a.tab.thread.as_mut().unwrap().set_search(String::new());
    a.on_key(KeyEvent::from(KeyCode::Char('g')));
    for _ in 0..400 {
        a.on_key(KeyEvent::from(KeyCode::Char('j')));
        render(&mut a);
    }
    assert_eq!(laid_out(&a), 400);
    let clock = a.clock;
    let t = a.tab.thread.as_mut().unwrap();
    let l = t.layout.as_ref().map(|l| (l.width, l.thumbs_on)).unwrap();
    let full = crate::ui::layout_all(t, l.0, l.1, clock);
    assert_eq!(t.layout.as_ref().unwrap().starts, full.starts);
}

/// A thread with a post three screens tall between short ones (the last quotes it).
fn tall_app() -> App {
    let mut a = app(false);
    a.tab.view = View::Thread;
    let lines: String = (1..=70).map(|k| format!("line {k}<br>")).collect();
    let posts = vec![
        post(3000, HOUR, Some("Tall"), "first"),
        post(3001, HOUR, None, &format!("<a href=\"#p3000\" class=\"quotelink\">&gt;&gt;3000</a><br>tall start<br>{lines}tall end")),
        post(3002, HOUR, None, "<a href=\"#p3001\" class=\"quotelink\">&gt;&gt;3001</a><br>after"),
    ];
    a.tab.thread = Some(ThreadView::new("g".into(), 3000, posts));
    a
}

#[test]
fn j_and_k_read_tall_posts_whole() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = tall_app();
    render(&mut a);
    let key = |a: &mut App, c: char| {
        a.on_key(KeyEvent::from(KeyCode::Char(c)));
        render(a).0
    };
    let selected = |a: &App| a.tab.thread.as_ref().unwrap().current().unwrap().no;
    // j onto the tall post: its top, with "more" below.
    let text = key(&mut a, 'j');
    assert_eq!(selected(&a), 3001);
    assert!(text.contains("tall start") && text.contains("↓ more") && !text.contains("↑") && text.contains("No.3001 (1/"), "{text}");
    // j again and again: on through it, every line seen, the same post selected.
    let numbers = |text: &str| text.lines().filter_map(|l| l.trim().trim_start_matches(['▌', '▏']).trim().strip_prefix("line ")?.split_whitespace().next()?.parse::<u32>().ok()).collect::<Vec<_>>();
    let mut seen: std::collections::BTreeSet<u32> = numbers(&text).into_iter().collect();
    let mut screens = 1;
    while render(&mut a).0.contains("↓ more") {
        let text = key(&mut a, 'j');
        assert_eq!(selected(&a), 3001, "{text}");
        seen.extend(numbers(&text));
        screens += 1;
        assert!(screens < 10);
    }
    let text = render(&mut a).0;
    assert!(text.contains("↑") && text.contains("tall end"), "{text}");
    assert!((1..=70).all(|k| seen.contains(&k)), "{seen:?}");
    assert!(text.contains(&format!("({screens}/{screens})")), "{text}");
    // Then the next post.
    key(&mut a, 'j');
    assert_eq!(selected(&a), 3002);
    // k comes back up into it at its end, and reads it backwards to its top.
    let text = key(&mut a, 'k');
    assert!(selected(&a) == 3001 && text.contains("tall end"), "{text}");
    let mut ups = 0;
    while render(&mut a).0.contains("↑") {
        key(&mut a, 'k');
        assert_eq!(selected(&a), 3001);
        ups += 1;
        assert!(ups < 10);
    }
    key(&mut a, 'k');
    assert_eq!(selected(&a), 3000);
    // In a conversation too.
    key(&mut a, 'j');
    key(&mut a, 'c');
    assert!(a.tab.thread.as_ref().unwrap().conversation.is_some());
    let text = key(&mut a, 'j');
    assert!(selected(&a) == 3001 && text.contains("↓ more") || text.contains("↑"), "{text}");
}

#[test]
fn a_post_exactly_a_screen_tall_needs_no_paging() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = tall_app();
    render(&mut a);
    let view = a.tab.thread.as_ref().unwrap().viewport;
    // As many lines as make post 3001 exactly the screen's height.
    let posts = a.tab.thread.as_ref().unwrap().posts.clone();
    let fits = (1..view).find(|&n| {
        let lines: String = (1..=n).map(|k| format!("line {k}<br>")).collect();
        let mut p = posts.clone();
        p[1] = post(3001, HOUR, None, &format!("{lines}end"));
        a.tab.thread = Some(ThreadView::new("g".into(), 3000, p));
        render(&mut a);
        let l = a.tab.thread.as_ref().unwrap().layout.as_ref().unwrap();
        l.starts[2] - 1 - l.starts[1] == view
    });
    assert!(fits.is_some());
    a.on_key(KeyEvent::from(KeyCode::Char('j')));
    let text = render(&mut a).0;
    assert!(!text.contains("↓ more") && text.contains("end"), "{text}");
    a.on_key(KeyEvent::from(KeyCode::Char('j')));
    assert_eq!(a.tab.thread.as_ref().unwrap().current().unwrap().no, 3002);
}

#[test]
fn inside_a_tall_post() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = tall_app();
    render(&mut a);
    for _ in 0..2 {
        a.on_key(KeyEvent::from(KeyCode::Char('j')));
        render(&mut a);
    }
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn the_selected_post_sits_at_the_margin_while_reading() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    for margin in [0.0f32, 0.3, 0.5] {
        let mut a = long_thread_app(120);
        a.tab.thread.as_mut().unwrap().margin = margin;
        render(&mut a);
        let view = a.tab.thread.as_ref().unwrap().viewport as isize;
        let m = ((view as f32 * margin) as isize).min((view - 1) / 2);
        // Reading down: once it scrolls, the selected post's top is at the margin (or, at
        // margin 0, its bottom at the screen's bottom), until the end of the thread.
        let mut scrolls = 0;
        for _ in 0..100 {
            let before = a.tab.thread.as_ref().unwrap().scroll;
            a.on_key(KeyEvent::from(KeyCode::Char('j')));
            render(&mut a);
            let t = a.tab.thread.as_ref().unwrap();
            let row = selected_row(&a);
            let l = t.layout.as_ref().unwrap();
            let at_end = t.scroll >= l.len().saturating_sub(view as usize);
            assert!(row >= 0 && row < view, "margin {margin}: row {row}");
            // Each time it scrolls: the post's top at the margin (margin 0: bottom-aligned).
            if t.scroll != before && !at_end {
                scrolls += 1;
                if margin > 0.0 {
                    assert_eq!(row, m, "margin {margin}");
                }
            }
        }
        assert!(scrolls > 3, "margin {margin}: {scrolls}");
        // Reading back up mirrors it: each time it scrolls, the post's bottom at the margin
        // from the screen's bottom (its top, for a post too tall for that).
        for _ in 0..40 {
            let before = a.tab.thread.as_ref().unwrap().scroll;
            a.on_key(KeyEvent::from(KeyCode::Char('k')));
            render(&mut a);
            let t = a.tab.thread.as_ref().unwrap();
            let l = t.layout.as_ref().unwrap();
            let bottom = (l.starts[t.entry() + 1] as isize) - t.scroll as isize;
            assert!(selected_row(&a) >= 0, "margin {margin}");
            if t.scroll != before && t.scroll > 0 && margin > 0.0 {
                assert!(bottom == view - m || selected_row(&a) == 0, "margin {margin}: bottom {bottom}");
            }
        }
        // A jump to a far post (up) lands with its top at the margin.
        assert!(a.tab.thread.as_mut().unwrap().jump_to(2030));
        render(&mut a);
        if margin > 0.0 {
            assert_eq!(selected_row(&a), m, "margin {margin}");
        } else {
            assert!(selected_row(&a) >= 0);
        }
        // The start of the thread: it can't scroll above, so the selection goes to the top.
        a.on_key(KeyEvent::from(KeyCode::Char('g')));
        render(&mut a);
        assert_eq!((selected_row(&a), a.tab.thread.as_ref().unwrap().scroll), (0, 0));
    }
}

#[test]
fn margin_zero_scrolls_as_before_and_small_screens_dont_loop() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    // Margin 0: each j scrolls as little as shows the post (its bottom at the screen's).
    let mut a = long_thread_app(60);
    render(&mut a);
    for _ in 0..40 {
        let before = a.tab.thread.as_ref().unwrap().scroll;
        a.on_key(KeyEvent::from(KeyCode::Char('j')));
        render(&mut a);
        let t = a.tab.thread.as_ref().unwrap();
        let l = t.layout.as_ref().unwrap();
        let (start, end) = (l.starts[t.entry()], l.starts[t.entry() + 1]);
        let want = if end > before + t.viewport { start.min(end - t.viewport) } else { before };
        assert_eq!(t.scroll, want);
    }
    // A screen of two lines, margin 0.5: j still reaches the end.
    let mut a = long_thread_app(30);
    a.tab.thread.as_mut().unwrap().margin = 0.5;
    render_at(&mut a, 60, 6);
    for _ in 0..2000 {
        a.on_key(KeyEvent::from(KeyCode::Char('j')));
        render_at(&mut a, 60, 6);
        if a.tab.thread.as_ref().unwrap().current().unwrap().no == 2029 {
            return;
        }
    }
    panic!("j didn't get to the end on a small screen");
}

#[test]
fn reading_mid_thread() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = long_thread_app(60);
    a.tab.thread.as_mut().unwrap().margin = 0.3;
    render(&mut a);
    for _ in 0..12 {
        a.on_key(KeyEvent::from(KeyCode::Char('j')));
        render(&mut a);
    }
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn hint_labels_from_before_a_refresh_dont_focus_what_is_gone() {
    use crate::app::{HintTo, Part};
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    render(&mut a);
    a.on_key(KeyEvent::from(KeyCode::Char('f')));
    render(&mut a);
    let label = a.hints().unwrap().targets.iter().find(|t| matches!(t.to, HintTo::Thread(_, Some(Part::Replies)))).unwrap().label.clone();
    // The thread comes back without any replies (found by fuzzing).
    let posts: Vec<Post> = thread().posts.into_iter().map(|p| Post { quotes: Vec::new(), ..p }).collect();
    a.tab.thread = Some(ThreadView::new("g".into(), 1000, posts));
    type_text(&mut a, &label);
    assert!(a.tab.thread.as_ref().unwrap().focus.is_none());
    assert!(a.status.as_ref().unwrap().text.contains("changed since the labels went up"));
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
    a.tab.popup = Some(crate::app::TabPopup::Viewer(Viewer::new(vec![file("op.png"), file("clip.webm")], 0, None)));
    insta::assert_snapshot!(snapshot(&mut a));
    // Zoomed: how far, and the keys that move around.
    a.tab.viewer_mut().unwrap().crop = crate::images::Crop::FIT.zoomed(true).zoomed(true);
    let text = snapshot(&mut a);
    assert!(text.contains("1 of 2  ·  200%") && text.contains("h/j/k/l move") && text.contains("0, esc fit"), "{text}");
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
    assert!(matches!(a.settings_popup(), Some(SettingsPopup::Themes { .. })));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn color_editor() {
    let mut a = app(false);
    a.open_settings();
    a.settings_list.state.select(Some(1));
    a.activate_setting();
    assert!(matches!(a.settings_popup(), Some(SettingsPopup::Colors { .. })));
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
    let keys = crate::app::settings().count() - 1;
    a.settings_list.state.select(Some(keys));
    a.activate_setting();
    insta::assert_snapshot!(snapshot(&mut a));
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Enter));
    insta::assert_snapshot!("key_editor_capturing", snapshot(&mut a));
}

#[test]
fn links_panel() {
    let mut a = thread_app(false);
    let t = a.tab.thread.as_mut().unwrap();
    t.selected = 4;
    t.posts[4].urls = vec!["https://example.com/a-long-path?with=query".into()];
    t.posts[4].files = vec![file("notes.pdf")];
    a.open_links();
    insta::assert_snapshot!(snapshot(&mut a));
}

fn with_filters(a: &mut App) {
    let cfg = "[[filter]]\npattern = \"Rust\"\naction = \"highlight\"\nlabel = \"rust\"\n[[filter]]\npattern = \"implying\"\nlabel = \"no implying\"";
    a.filters = crate::filter::tests::filters(cfg).unwrap();
}

#[test]
fn filtered_catalog_and_thread() {
    let mut a = catalog_app(true);
    with_filters(&mut a);
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
    let mut a = catalog_app(true);
    a.default_layout = crate::config::CatalogLayout::Grid;
    a.tab.catalog_list.state.select(Some(1));
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("catalog_grid_backgrounds", bg_map(&mut a));
    assert_eq!(a.grid_cols, 4);
    // Without images, the grid is drawn as cards.
    let mut b = catalog_app(false);
    b.default_layout = crate::config::CatalogLayout::Grid;
    let text = snapshot(&mut b);
    assert!(text.contains("312 replies") && b.grid_cols == 0, "{text}");
}

#[test]
fn gallery() {
    let mut a = thread_app(true);
    let t = a.tab.thread.as_mut().unwrap();
    t.posts[2].files = vec![file("code.png"), file("clip.webm")];
    a.open_gallery();
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('l')));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn archive_search_results() {
    let mut a = app(false);
    let v = crate::backend::fixture("foolfuuka_search.json");
    a.tab.site = a.sites.iter().position(|s| s.cfg.name == "desuarchive").unwrap();
    a.typing = Some(crate::app::Typing::ArchiveQuery("borrow".into()));
    a.tab.view = View::Catalog;
    insta::assert_snapshot!("archive_search_typing", snapshot(&mut a));
    a.typing = None;
    a.tab.view = View::Search;
    let page = crate::backend::foolfuuka::parse_search(&v).unwrap();
    a.tab.search = Some(crate::app::Search::for_tests("g", "rust borrow checker", page));
    a.tab.search_list.state.select(Some(0));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn image_search_panel() {
    let mut a = thread_app(false);
    let t = a.tab.thread.as_mut().unwrap();
    t.posts[0].files.push(file("second.jpg"));
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
fn help_fits_at_110x36_and_scrolls_when_small() {
    let mut a = app(false);
    a.popup = Some(Popup::Help(0));
    let (text, _) = render_at(&mut a, 110, 36);
    assert!(!text.contains("↓ more"), "{text}");
    // Two columns, everything on screen.
    for line in ["Everywhere", "Home screen", "Image viewer", "Catalog", "Thread", "mark as yours", "copy file URL / post link", "watch / quote tab / general", "favorite this board"] {
        assert!(text.contains(line), "{line} missing:\n{text}");
    }
    let (text, _) = render_at(&mut a, 60, 20);
    assert!(text.contains("Everywhere") && !text.contains("mark as yours") && text.contains("↓ more (j)"), "{text}");
    a.popup = Some(Popup::Help(100));
    let (text, _) = render_at(&mut a, 60, 20);
    assert!(text.contains("copy text / link"), "{text}");
}

#[test]
fn narrow_screens() {
    let mut a = app(false);
    // Settings scroll to the selected one.
    a.open_settings();
    let last = crate::app::settings().count() - 1;
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
    // A terminal too small for any of it still draws (the thread's scrollbar had no room).
    for (w, h) in [(1, 1), (3, 2), (20, 3)] {
        render_at(&mut a, w, h);
    }
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
    a.clock = Clock { fixed: Some(NOW + 3 * HOUR), ..Default::default() };
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
    // A post that comes back changed (its file deleted) is laid out again (found by fuzzing).
    let mut posts = a.tab.thread.as_ref().unwrap().posts.clone();
    posts[0].files.clear();
    let mut t = crate::app::ThreadView::new("g".into(), 1000, posts);
    t.cache = std::mem::take(&mut a.tab.thread.as_mut().unwrap().cache);
    a.tab.thread = Some(t);
    let (text, _) = render(&mut a);
    assert!(!text.contains("op.png") && !std::rc::Rc::ptr_eq(&after[0], &blocks(&a)[0]), "{text}");
    let after = blocks(&a);
    // A new theme lays everything out again.
    a.set_theme(crate::theme::BUILTIN[1].1);
    render(&mut a);
    assert!(!std::rc::Rc::ptr_eq(&after[1], &blocks(&a)[1]));
}

#[test]
fn focused_quote_peeks_at_its_post() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    a.tab.thread.as_mut().unwrap().select(1);
    render(&mut a);
    // tab: the post's first part, its quote of the OP, which shows the OP without taking keys.
    a.on_key(KeyEvent::from(KeyCode::Tab));
    let (text, buf) = render(&mut a);
    insta::assert_snapshot!(text);
    let t = theme();
    let focused: String = buf.content().iter().filter(|c| c.bg == t.primary).map(|c| c.symbol()).collect();
    assert!(focused.contains(">>1000"), "{focused:?}");
    // Keys still move the focus: on to the Replies label, then the reply.
    a.on_key(KeyEvent::from(KeyCode::Tab));
    a.on_key(KeyEvent::from(KeyCode::Tab));
    let focused: String = render(&mut a).1.content().iter().filter(|c| c.bg == t.primary).map(|c| c.symbol()).collect();
    assert!(focused.contains(">>1003"), "{focused:?}");
}

#[test]
fn focused_file() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    render(&mut a);
    a.on_key(KeyEvent::from(KeyCode::Tab));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn actions_menu() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    a.tab.thread.as_mut().unwrap().select(1);
    render(&mut a);
    a.on_key(KeyEvent::from(KeyCode::Char('.')));
    insta::assert_snapshot!(snapshot(&mut a));
    // On a focused quote it starts with what enter does to it.
    a.on_key(KeyEvent::from(KeyCode::Esc));
    a.on_key(KeyEvent::from(KeyCode::Tab));
    a.on_key(KeyEvent::from(KeyCode::Char('.')));
    let text = render(&mut a).0;
    assert!(text.contains("enter  go to >>1000") && text.contains("copy its address"), "{text}");
}

#[test]
fn link_hints() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    render(&mut a);
    a.on_key(KeyEvent::from(KeyCode::Char('f')));
    insta::assert_snapshot!("link_hints_thread", snapshot(&mut a));
    // A label picks its target: here, post 1001's quote of the OP, which jumps there.
    let h = a.hints().unwrap();
    let label = h.targets.iter().find(|x| matches!(&x.to, crate::app::HintTo::Thread(1, Some(_)))).unwrap().label.clone();
    type_text(&mut a, &label);
    assert!(a.hints().is_none());
    assert_eq!(a.tab.thread.as_ref().unwrap().selected, 0);
    // In a catalog: a label per thread; picking one opens it.
    let mut c = catalog_app(false);
    render(&mut c);
    c.on_key(KeyEvent::from(KeyCode::Char('f')));
    insta::assert_snapshot!("link_hints_catalog", snapshot(&mut c));
    c.on_key(KeyEvent::from(KeyCode::Char('s')));
    assert_eq!((c.tab.view, c.tab.pending_thread), (View::Thread, c.tab.catalog[1].no));
}

#[test]
fn clicking_a_part_focuses_it() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut a = thread_app(false);
    let (text, _) = render(&mut a);
    // Where ">>1000" is drawn in post 1001.
    let (row, line) = text.lines().enumerate().find(|(_, l)| l.contains(">>1000 (OP)")).unwrap();
    let col = line.find(">>1000").unwrap() as u16 + 2;
    let click = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: col, row: row as u16, modifiers: ratatui::crossterm::event::KeyModifiers::NONE };
    a.on_mouse(click, std::time::Instant::now());
    let t = a.tab.thread.as_ref().unwrap();
    assert_eq!(t.selected, 1);
    assert!(matches!(&t.focus, Some(Part::Link(_))), "{:?}", t.focus);
    // Right-click: the menu for it.
    a.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Right), ..click }, std::time::Instant::now());
    assert!(a.menu().is_some_and(|m| m.items.iter().any(|i| matches!(i, crate::app::MenuItem::Enter(l) if l == "go to >>1000"))));
}

#[test]
fn saving_the_thread_asks_first() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    a.download_dir = Some("/saves/{board}/{thread}".into());
    render(&mut a);
    a.on_key(KeyEvent::from(KeyCode::Char('.')));
    let m = a.menu_mut().unwrap();
    let row = m.items.iter().position(|it| matches!(it, crate::app::MenuItem::Act(_, l) if l == "save the thread as a page…"));
    m.list.select(row);
    a.on_key(KeyEvent::from(KeyCode::Enter));
    insta::assert_snapshot!(snapshot(&mut a));
    a.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(a.confirm().is_none() && !render(&mut a).0.contains("thread.html"));
}

#[test]
fn long_folders_wrap_at_slashes() {
    assert_eq!(crate::ui::wrap_path("to /a/bb/ccc", 20), ["to /a/bb/ccc"]);
    assert_eq!(crate::ui::wrap_path("to /aaaa/bbbb/cccc/dddd", 12), ["to /aaaa/", "bbbb/cccc/", "dddd"]);
    // No slash to break at: by width.
    assert_eq!(crate::ui::wrap_path("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    assert_eq!(crate::ui::wrap_path("日本語のフォルダ", 6), ["日本語", "のフォ", "ルダ"]);
}

#[test]
fn adding_a_site() {
    use crate::app::Adding;
    use crate::config::{BoardConfig, SiteConfig, SiteKind};
    let mut a = app(false);
    a.popup = Some(Popup::Adding(Adding::Typing("somechan.org/b/".into())));
    let text = render(&mut a).0;
    assert!(text.contains("Link  somechan.org/b/▏") && text.contains("enter look · esc cancel"), "{text}");
    a.popup = Some(Popup::Adding(Adding::Looking { id: 1, host: "somechan.org".into(), open: None }));
    assert!(render(&mut a).0.contains("Asking somechan.org what it runs…"));
    let site = SiteConfig {
        name: "somechan".into(),
        kind: SiteKind::Vichan,
        url: Some("https://somechan.org".into()),
        boards: Some(vec![BoardConfig::Uri("b".into())]),
        thumb_ext: None,
        archive: None,
        media_url: None,
    };
    a.popup = Some(Popup::Adding(Adding::Site { site: site.clone(), name: "somechan".into(), open: None }));
    insta::assert_snapshot!(snapshot(&mut a));
    // Boards read from the bar on its pages.
    let boards = ["wiz", "dep", "hob"].map(|b| BoardConfig::Full { uri: b.into(), title: String::new() }).to_vec();
    a.popup = Some(Popup::Adding(Adding::Site { site: SiteConfig { boards: Some(boards), ..site }, name: "somechan".into(), open: None }));
    let text = render(&mut a).0;
    assert!(text.contains("3 boards, from the list on its pages:") && text.contains("wiz dep hob"), "{text}");
    // Where its files are, when that isn't the usual.
    if let Some(Adding::Site { site, .. }) = a.adding_mut() {
        site.thumb_ext = Some("png".into());
        site.media_url = Some("https://media.example".into());
    }
    let text = render(&mut a).0;
    assert!(text.contains("Thumbnails are .png, files on media.example."), "{text}");
}

#[test]
fn images_off_on_a_board() {
    // The board's own setting: tiles say so, nothing's asked for, and the top bar says it.
    let mut a = catalog_app(true);
    a.tab.catalog_board = "g".into();
    a.store.board_prefs.entry("4chan/g".into()).or_default().images = Some(false);
    insta::assert_snapshot!("images_off_catalog", snapshot(&mut a));
    assert_eq!(a.images.queued(), 0);
    let mut a = thread_app(true);
    a.store.board_prefs.entry("4chan/g".into()).or_default().images = Some(false);
    insta::assert_snapshot!("images_off_thread", snapshot(&mut a));
    assert_eq!(a.images.queued(), 0);
}

#[test]
fn saved_search_results() {
    use crate::app::{SavedSearch, Search};
    let mut a = app(false);
    let key = |board: &str, no| ThreadKey { site: "4chan".into(), board: board.into(), no };
    let hit = |no, text: &str| Post { no, name: "Anonymous".into(), time: NOW - HOUR, body: vec![ratatui::text::Line::from(text.to_string())], ..Default::default() };
    let hits = vec![(1000, hit(1002, "we were talking about rust and the borrow checker today")), (900, hit(900, "Rust or C++?"))];
    let mut s = Search::for_tests("", "rust", crate::backend::SearchPage { hits, total: None });
    s.saved = Some(SavedSearch::for_tests(vec![key("g", 1000), key("g", 900)], 2, 5, false));
    a.tab.search = Some(s);
    a.tab.view = View::Search;
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn updating_a_built_in_sites_boards() {
    use crate::app::Adding;
    use crate::app::BoardsUpdate;
    use crate::config::BoardConfig;
    let mut a = app(false);
    let lain = a.sites.iter().position(|s| s.cfg.name == "lainchan").unwrap();
    let list = a.sites[lain].cfg.boards.clone().unwrap();
    let mut bar = list.clone();
    bar.push(BoardConfig::Full { uri: "mega".into(), title: "Overboard".into() });
    bar.remove(0);
    a.popup = Some(Popup::Adding(Adding::Boards { site: lain, update: BoardsUpdate::new(&list, bar), drop: false, builtin: true }));
    let text = render(&mut a).0;
    assert!(text.contains("Update lainchan's boards?") && text.contains("New: /mega/"), "{text}");
    assert!(text.contains("kept (d drops them)") && text.contains("lainchan is built in: this saves it as one of your sites"), "{text}");
}

#[test]
fn a_hidden_words_label() {
    let mut a = thread_app(false);
    a.hidden_words = vec!["implying".into()];
    a.filters = crate::filter::Filters::new(&[]).unwrap().with_words(&a.hidden_words).unwrap();
    a.remark_thread();
    a.tab.thread.as_mut().unwrap().show_hidden = true;
    let text = render(&mut a).0;
    assert!(text.contains("hidden word: implying") && !text.contains("hidden: hidden word"), "{text}");
}

#[test]
fn settings_list_popups() {
    use crate::app::MySites;
    use crate::config::{SiteConfig, SiteKind};
    let mut a = app(false);
    a.open_settings();
    let site = |name: &str| SiteConfig { name: name.into(), kind: SiteKind::Vichan, url: Some(format!("https://{name}.example")), boards: None, thumb_ext: None, archive: None, media_url: None };
    let mut shots = Vec::new();
    // Your sites: none, then three with one armed for removal.
    a.popup = Some(Popup::Settings(SettingsPopup::Sites(MySites { list: ratatui::widgets::ListState::default().with_selected(Some(0)), sites: vec![], armed: None })));
    shots.push(render_at(&mut a, 110, 20).0);
    let sites = vec![site("one"), site("lainchan"), site("three")];
    a.popup = Some(Popup::Settings(SettingsPopup::Sites(MySites { list: ratatui::widgets::ListState::default().with_selected(Some(2)), sites, armed: Some(2) })));
    shots.push(render_at(&mut a, 110, 20).0);
    // Board images.
    a.popup = Some(Popup::Settings(SettingsPopup::BoardImages { list: ratatui::widgets::ListState::default().with_selected(Some(0)) }));
    shots.push(render_at(&mut a, 90, 16).0);
    a.store.board_prefs.entry("4chan/b".into()).or_default().images = Some(false);
    a.store.board_prefs.entry("4chan/g".into()).or_default().images = Some(true);
    a.popup = Some(Popup::Settings(SettingsPopup::BoardImages { list: ratatui::widgets::ListState::default().with_selected(Some(1)) }));
    shots.push(render_at(&mut a, 90, 16).0);
    // Hidden words: none, a few, typing one, and many on a small screen, scrolled.
    a.popup = Some(Popup::Settings(SettingsPopup::HiddenWords { list: ratatui::widgets::ListState::default().with_selected(Some(0)), typing: None }));
    shots.push(render_at(&mut a, 90, 16).0);
    a.hidden_words = vec!["crypto".into(), "free money".into(), "λ".into()];
    a.popup = Some(Popup::Settings(SettingsPopup::HiddenWords { list: ratatui::widgets::ListState::default().with_selected(Some(1)), typing: None }));
    shots.push(render_at(&mut a, 90, 16).0);
    a.popup = Some(Popup::Settings(SettingsPopup::HiddenWords { list: ratatui::widgets::ListState::default().with_selected(Some(1)), typing: Some("spa".into()) }));
    shots.push(render_at(&mut a, 90, 16).0);
    a.hidden_words = (0..30).map(|i| format!("word{i}")).collect();
    for typing in [None, Some("x".to_string())] {
        a.popup = Some(Popup::Settings(SettingsPopup::HiddenWords { list: ratatui::widgets::ListState::default().with_selected(Some(25)), typing }));
        shots.push(render_at(&mut a, 90, 16).0);
    }
    insta::assert_snapshot!(shots.join("\n=====\n"));
}

#[test]
fn popups_with_more_rows_than_a_screen_can_hold() {
    // More lines than fit in a u16: sized to the screen, without overflowing.
    for n in 65_525..65_536 {
        let mut a = thread_app(false);
        let t = a.tab.thread.as_mut().unwrap();
        t.posts[1].body = (0..n).map(|_| ratatui::text::Line::from("x")).collect();
        t.selected = 3;
        a.tab.popup = Some(crate::app::TabPopup::Preview(Preview { posts: vec![1001], elsewhere: vec![], scroll: 0 }));
        render(&mut a);
    }
}

#[test]
fn every_view_draws_on_tiny_screens() {
    type Make = fn() -> App;
    let views: Vec<(&str, Make)> = vec![
        ("sites", || app(false)),
        ("boards", || {
            let mut a = app(false);
            a.tab.view = View::Boards;
            a
        }),
        ("catalog", || catalog_app(true)),
        ("catalog compact", || {
            let mut a = catalog_app(true);
            a.default_layout = crate::config::CatalogLayout::Compact;
            a
        }),
        ("catalog grid", || {
            let mut a = catalog_app(true);
            a.default_layout = crate::config::CatalogLayout::Grid;
            a
        }),
        ("thread", || thread_app(true)),
        ("quote preview", || {
            let mut a = thread_app(false);
            a.tab.popup = Some(crate::app::TabPopup::Preview(Preview { posts: vec![1001], elsewhere: vec![7], scroll: 0 }));
            a
        }),
        ("links", || {
            let mut a = thread_app(false);
            a.open_links();
            a
        }),
        ("viewer", || {
            let mut a = thread_app(true);
            a.tab.popup = Some(crate::app::TabPopup::Viewer(Viewer::new(vec![file("op.png")], 0, None)));
            a
        }),
        ("help", || {
            let mut a = app(false);
            a.popup = Some(Popup::Help(0));
            a
        }),
        ("watched", || {
            let mut a = app(false);
            a.tab.view = View::Watched;
            a
        }),
        ("history", || {
            let mut a = app(false);
            a.tab.view = View::History;
            a
        }),
        ("settings", || {
            let mut a = app(false);
            a.open_settings();
            a
        }),
        ("theme picker", || {
            let mut a = app(false);
            a.open_settings();
            a.activate_setting();
            a
        }),
    ];
    for (name, make) in views {
        for (w, h) in [(0, 0), (1, 1), (8, 40), (40, 8), (2, 200)] {
            let mut a = make();
            let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render_at(&mut a, w, h)));
            assert!(drawn.is_ok(), "{name} at {w}x{h}");
        }
    }
}

#[test]
fn thread_gone_without_a_copy_offers_the_archive() {
    let mut a = app(false);
    a.tab.view = View::Thread;
    a.thread_gone(&ThreadKey { site: "4chan".into(), board: "g".into(), no: 901 });
    let text = &a.status.as_ref().unwrap().text;
    assert_eq!(text, "Thread was deleted or archived: a opens it in desuarchive");
}

#[test]
fn counts_of_one_are_singular() {
    let mut a = catalog_app(false);
    a.tab.catalog[0].replies = Some(1);
    a.tab.catalog[0].images = Some(1);
    let text = render(&mut a).0;
    assert!(text.contains("1 reply · 1 image ·"), "{text}");
}

#[test]
fn help_opens_on_the_keys_for_where_you_are() {
    let mut a = thread_app(false);
    a.popup = Some(Popup::Help(0));
    let (text, _) = render_at(&mut a, 60, 40);
    let at = |title| text.find(&format!("  {title}\n")).unwrap_or(usize::MAX);
    assert!(at("Everywhere") < at("Thread") && at("Thread") < at("Home screen"), "{text}");
}

#[test]
fn footer_hints_drop_whole_and_keep_help() {
    for w in [30, 40, 60, 80] {
        let mut a = thread_app(false);
        let (text, _) = render_at(&mut a, w, 20);
        let footer = text.lines().last().unwrap().trim_end();
        assert!(footer.ends_with("? help"), "{w}: {footer:?}");
        // Every hint before it is whole: a key, a space, a label, then three spaces.
        assert!(footer.split("   ").all(|h| h.trim().contains(' ')), "{w}: {footer:?}");
    }
}

#[test]
fn hidden_spoilers_selection_and_focus_dont_rely_on_color() {
    // The conversation fixture has a spoiler: its text isn't on screen until revealed.
    let mut a = thread_app(false);
    let t = a.tab.thread.as_mut().unwrap();
    t.posts[2].body = crate::markup::parse_html("plain <s>secret</s> text", crate::markup::Flavor::Fourchan).lines;
    t.selected = 2;
    let (text, _) = render(&mut a);
    assert!(!text.contains("secret") && text.contains("plain ░░░░░░ text"), "{text}");
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('S')));
    assert!(render(&mut a).0.contains("plain secret text"));
    // The selected post has a bar drawn as a glyph, not only a background.
    assert!(text.lines().any(|l| l.trim_start().starts_with('▌')), "{text}");
}

#[test]
fn no_color_means_the_mono_theme() {
    use std::ffi::OsStr;
    assert_eq!(crate::theme::default_for(Some(OsStr::new("1"))), "mono");
    assert_eq!(crate::theme::default_for(Some(OsStr::new(""))), crate::theme::DEFAULT_THEME);
    assert_eq!(crate::theme::default_for(None), crate::theme::DEFAULT_THEME);
    let mut a = thread_app(false);
    a.set_theme(crate::theme::BUILTIN.iter().find(|(n, _)| *n == "mono").unwrap().1);
    let (_, buf) = render(&mut a);
    let colored = buf.content().iter().any(|c| c.fg != ratatui::style::Color::Reset || c.bg != ratatui::style::Color::Reset);
    assert!(!colored);
}

/// Render at 50x20, checking nothing is drawn with box-drawing characters.
fn narrow(app: &mut App) -> String {
    let text = render_at(app, 50, 20).0;
    assert!(!text.chars().any(|c| ('\u{2500}'..='\u{257f}').contains(&c)), "{text}");
    text
}

#[test]
fn narrow_catalog_keeps_its_counts() {
    let mut a = catalog_app(false);
    let text = narrow(&mut a);
    assert!(text.contains(" R") && text.contains(" I"), "{text}");
    insta::assert_snapshot!(text);
    a.default_layout = crate::config::CatalogLayout::Compact;
    insta::assert_snapshot!("narrow_catalog_compact", narrow(&mut a));
}

#[test]
fn narrow_thread_keeps_where_you_are() {
    let mut a = thread_app(false);
    let text = narrow(&mut a);
    let bar = text.lines().next().unwrap();
    // The thread's crumb keeps some letters; the counts gave way.
    assert!(bar.contains("Snaps"), "{bar}");
    insta::assert_snapshot!(text);
}

#[test]
fn narrow_tall_post_markers_leave_the_text() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = tall_app();
    narrow(&mut a);
    a.on_key(KeyEvent::from(KeyCode::Char('j')));
    let text = narrow(&mut a);
    // Its last row: the text whole, then the marker.
    let last = text.lines().rev().nth(1).unwrap();
    assert!(last.ends_with("↓ more") || last.ends_with('↓'), "{text}");
    insta::assert_snapshot!(text);
}

#[test]
fn narrow_home_and_help() {
    let mut a = app(false);
    insta::assert_snapshot!("narrow_home", narrow(&mut a));
    a.popup = Some(Popup::Help(0));
    insta::assert_snapshot!("narrow_help", narrow(&mut a));
}
