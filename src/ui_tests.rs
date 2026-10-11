//! Snapshot tests: every view rendered with fixed data and a fixed clock.

use ratatui::buffer::Buffer;

use crate::app::{App, Clock, Hit, Part, Popup, Preview, SettingsPopup, ThreadView, View, Viewer};
use crate::images::Images;
use crate::model::{Board, Post};
use crate::store::{Status, ThreadKey, Visit};
use crate::test_fixtures::*;
use crate::theme::theme;

const HOUR: i64 = 3600;

fn app(images: bool) -> App {
    let mut app = test_app();
    let key = |board: &str, no| ThreadKey { site: "4chan".into(), board: board.into(), no };
    app.store.watch(key("g", 1000), "Snapshot thread".into(), 5, 1002);
    app.store.watch(key("g", 900), "Old thread".into(), 300, 1199);
    app.store.watch(ThreadKey { site: "lainchan".into(), board: "λ".into(), no: 42 }, "Programming Employment".into(), 92, 77);
    app.store.watched_vec()[0].status = Status::Live { unread: 2, replies: 0 };
    app.store.watched_vec()[1].status = Status::Dead;
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
    a.tab.navigate(View::Thread);
    a.tab.thread = Some(thread());
    a
}

/// The app on the catalog fixture.
fn catalog_app(images: bool) -> App {
    let mut a = app(images);
    a.tab.navigate(View::Catalog);
    a.tab.catalog = catalog_here(&a, catalog());
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
    let mut t = ThreadView::new(ThreadKey { site: "4chan".into(), ..tkey("g", 1000) }, posts);
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
    a.tab.navigate(View::Boards);
    a.pick_row(View::Boards, 2);
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn catalog_with_thumbnail_placeholders() {
    let mut a = catalog_app(true);
    a.pick_row(View::Catalog, 1);
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("catalog_backgrounds", bg_map(&mut a));
    // Only what's on screen is asked for.
    assert!(a.images.queued() >= 2);
}

#[test]
fn catalog_compact() {
    let mut a = catalog_app(true);
    a.default_layout = crate::config::CatalogLayout::Compact;
    a.pick_row(View::Catalog, 0);
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
    a.popup = Some(Popup::Help(Default::default()));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn quote_preview() {
    let mut a = thread_app(false);
    let t = a.tab.thread.as_mut().unwrap();
    t.selected = 3;
    a.tab.popup = Some(crate::app::TabPopup::Preview(Preview { posts: vec![1001], elsewhere: vec![], scroll: Default::default() }));
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
    a.tab.navigate(View::Watched);
    insta::assert_snapshot!(snapshot(&mut a));
    // Narrow, the subjects give way: what's new and what died stay in sight.
    let (text, _) = render_at(&mut a, 60, 10);
    assert!(text.contains("2 new") && text.contains("archived/deleted") && text.contains("…"), "{text}");
}

#[test]
fn watched_threads_pages() {
    // Where each is in its board's index; the last page stands out, and a dead thread's
    // page isn't shown.
    use crate::backend::ThreadPages;
    let mut a = app(false);
    a.board_pages.insert(("4chan".into(), "g".into()), ThreadPages { page: [(1000, 3), (900, 10)].into(), of: 10 });
    a.board_pages.insert(("lainchan".into(), "λ".into()), ThreadPages { page: [(42, 13)].into(), of: 13 });
    a.tab.navigate(View::Watched);
    insta::assert_snapshot!(snapshot(&mut a));
    // The thread's bar says it too.
    let mut a = thread_app(false);
    a.board_pages.insert(("4chan".into(), "g".into()), ThreadPages { page: [(1000, 3)].into(), of: 10 });
    let bar = |a: &mut App| render(a).0.lines().next().unwrap().to_string();
    assert!(bar(&mut a).ends_with("p3/10"), "{}", bar(&mut a));
    a.board_pages.insert(("4chan".into(), "g".into()), ThreadPages { page: [(1000, 10)].into(), of: 10 });
    assert!(bar(&mut a).contains("last page 10/10") && !bar(&mut a).contains("p10/10"), "{}", bar(&mut a));
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
    a.tab.navigate(View::Saved);
    insta::assert_snapshot!(snapshot(&mut a));
    // The home screen counts them.
    a.tab.navigate(View::Sites);
    assert!(snapshot(&mut a).contains("Saved           3 threads, 1 gone from the site"));
}

#[test]
fn saved_dead_thread() {
    let mut a = thread_app(false);
    with_saved(&mut a);
    a.tab.copy = Some(crate::app::ThreadCopy::Saved(crate::app::Offline { saved: NOW - 5 * HOUR, dead: true }));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn thread_gone_offers_the_saved_copy() {
    let mut a = app(false);
    with_saved(&mut a);
    a.tab.navigate(View::Thread);
    a.thread_gone(&ThreadKey { site: "4chan".into(), board: "g".into(), no: 900 });
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn add_filter_popup() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    let t = a.tab.thread.as_mut().unwrap();
    t.posts[0].poster = "Named !Trip".into();
    t.posts[0].files[0].md5 = Some("u8Vh17KxaDvUJ6bBcmE/eg==".into());
    a.on_key(KeyEvent::from(KeyCode::Char('.')));
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
    a.tab.thread.as_mut().unwrap().posts[2].poster = "Named !Trip".into();
    a.tab.navigate(View::Settings);
    a.popup = Some(Popup::Settings(a.filter_list(1)));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn filter_editor() {
    let mut a = thread_app(false);
    a.filter_cfgs = some_filters();
    a.tab.navigate(View::Settings);
    let draft = a.filter_cfgs[1].clone();
    a.popup = Some(Popup::Settings(SettingsPopup::FilterEdit { index: Some(1), draft: draft.clone(), row: 0, typing: Some("^Named (!Trip".into()) }));
    insta::assert_snapshot!(snapshot(&mut a));
    // On a short screen the rows scroll: the last one selected is in view.
    let last = crate::app::EDIT_ROWS.len() - 1;
    a.popup = Some(Popup::Settings(SettingsPopup::FilterEdit { index: Some(1), draft, row: last, typing: None }));
    let (text, _) = render_at(&mut a, 100, 20);
    assert!(text.contains("On           yes") && text.contains("Top") && !text.contains("Pattern"), "{text}");
}

#[test]
fn conversation() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    a.tab.thread.as_mut().unwrap().select(1);
    a.on_key(KeyEvent::from(KeyCode::Char('c')));
    a.on_key(KeyEvent::from(KeyCode::Char('C')));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn only_posts_with_files_or_no_images() {
    use crate::app::Media;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let m = |a: &mut App| ['c', 'M'].into_iter().for_each(|c| a.on_key(KeyEvent::from(KeyCode::Char(c))));
    // A long thread, every third post with a file, read from the middle.
    let mut a = long_thread_app(300);
    a.images = Images::offline();
    for (k, p) in a.tab.thread.as_mut().unwrap().posts.iter_mut().enumerate() {
        if k % 3 == 1 {
            p.files = vec![file(&format!("{}.png", p.no))];
        }
    }
    render(&mut a);
    a.tab.thread.as_mut().unwrap().select(150);
    render(&mut a);
    assert_layout_exact(&mut a);
    // Files only: the OP and the posts with files, the selection on the next one that has
    // one, laid out exactly and on screen; the bar says so.
    m(&mut a);
    let text = render(&mut a).0;
    let t = a.tab.thread.as_ref().unwrap();
    assert_eq!((t.media, t.entries.len(), t.posts[t.selected].no), (Media::Files, 101, 2151));
    assert!(t.entries.iter().skip(1).all(|e| !t.posts[e.post].files.is_empty()));
    assert!(text.lines().next().unwrap().contains("with files"), "{text}");
    assert!(text.contains("No.2151"), "{text}");
    assert_layout_exact(&mut a);
    // j goes from one to the next.
    a.on_key(KeyEvent::from(KeyCode::Char('j')));
    render(&mut a);
    assert_eq!(a.tab.thread.as_ref().unwrap().current().unwrap().no, 2154);
    assert_layout_exact(&mut a);
    // Images hidden: every post again, tiles say so, and nothing's asked for.
    let asked = a.images.queued();
    m(&mut a);
    let text = render(&mut a).0;
    let t = a.tab.thread.as_ref().unwrap();
    assert_eq!((t.media, t.entries.len(), t.posts[t.selected].no), (Media::NoImages, 300, 2154));
    assert!(text.lines().next().unwrap().contains("images hidden") && text.contains("image off"), "{text}");
    assert_eq!(a.images.queued(), asked);
    assert_layout_exact(&mut a);
    // And back.
    m(&mut a);
    let text = render(&mut a).0;
    assert_eq!(a.tab.thread.as_ref().unwrap().media, Media::All);
    assert!(!text.contains("image off") && !text.contains("images hidden"), "{text}");
    assert_layout_exact(&mut a);

    // A snapshot of the fixture thread with files only: the OP alone has one.
    let mut a = thread_app(false);
    a.tab.thread.as_mut().unwrap().posts[3].files = vec![file("reply.png")];
    a.tab.thread.as_mut().unwrap().select(2);
    m(&mut a);
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn thread_from_its_last_copy() {
    let mut a = thread_app(false);
    a.tab.copy = Some(crate::app::ThreadCopy::Cached(crate::app::Offline { saved: NOW - 3 * 60, dead: false }));
    a.tab.fake_load(1, "Loading thread 1000", crate::app::Then::Show);
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn deleted_post() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    // No.1001 deleted: kept, marked, and its reply still links to it.
    a.tab.thread.as_mut().unwrap().deleted.insert(1001);
    insta::assert_snapshot!(snapshot(&mut a));
    assert_layout_exact(&mut a);
    a.tab.thread.as_mut().unwrap().select(1);
    a.on_key(KeyEvent::from(KeyCode::Char('c')));
    a.on_key(KeyEvent::from(KeyCode::Char('C')));
    let (text, _) = render(&mut a);
    assert!(text.contains(" deleted ") && text.contains("No.1003"), "{text}");
    assert_layout_exact(&mut a);
}

#[test]
fn poster_ids_and_flags() {
    use crate::app::HintTo;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = app(false);
    a.tab.navigate(View::Thread);
    a.tab.board = Some(Board { uri: "pol".into(), title: "Politically Incorrect".into(), nsfw: Some(true) });
    let posts = crate::backend::futaba::Futaba::fourchan(None).parse_thread("pol", &crate::backend::fixture("4chan_pol_thread.json"));
    a.tab.thread = Some(ThreadView::new(here(&a, "pol", 487211034), posts));
    insta::assert_snapshot!(snapshot(&mut a));
    assert_layout_exact(&mut a);
    let (text, buf) = render_at(&mut a, 100, 90);
    // Each ID's chip has its color, the same on each of its posts.
    // Where the chip's text starts on each row it's on, and its background there.
    let at = |text: &str, needle: &str| -> Vec<(u16, u16)> {
        text.lines().enumerate().filter_map(|(y, l)| Some((l.split_once(needle)?.0.chars().count() as u16, y as u16))).collect()
    };
    let chip_bg = |id: &str| -> Vec<ratatui::style::Color> { at(&text, &format!("ID:{id} ")).into_iter().map(|p| buf[p].bg).collect() };
    let (ab, zq) = (chip_bg("Ab3dEf+g"), chip_bg("Zq9Wx2Lp"));
    assert!(ab.len() == 2 && ab[0] == ab[1] && zq.len() == 2 && zq[0] == zq[1], "{ab:?} {zq:?}");
    assert_ne!(ab[0], zq[0]);
    assert!(text.contains("ID:Ab3dEf+g (2)") && text.contains("  AC  ") && text.contains(" GB "), "{text}");
    // Under mono, only the text.
    crate::theme::set(crate::theme::resolve("mono", &Default::default()).unwrap());
    a.tab.thread.as_mut().unwrap().cache.clear();
    a.tab.thread.as_mut().unwrap().layout = None;
    let (text, buf) = render(&mut a);
    assert_eq!(buf[at(&text, "ID:Zq9Wx2Lp")[0]].bg, ratatui::style::Color::Reset);
    crate::theme::set(crate::theme::resolve("material", &Default::default()).unwrap());
    a.tab.thread.as_mut().unwrap().cache.clear();
    a.tab.thread.as_mut().unwrap().layout = None;
    // A hint on an ID shows that poster's posts.
    render(&mut a);
    a.on_key(KeyEvent::from(KeyCode::Char('f')));
    render(&mut a);
    let second = a.tab.thread.as_ref().unwrap().entries[1].path.clone();
    let label = a.hints().unwrap().targets.iter().find(|t| matches!(t.to, HintTo::Thread(ref p, Some(Part::Poster)) if *p == second)).unwrap().label.clone();
    type_text(&mut a, &label);
    let (text, _) = render(&mut a);
    let t = a.tab.thread.as_ref().unwrap();
    assert_eq!(t.entries.iter().map(|e| t.posts[e.post].no).collect::<Vec<_>>(), [487211102, 487211390]);
    assert!(text.contains("Posts by ID:Zq9Wx2Lp") && text.contains("esc shows the whole thread"), "{text}");
    assert_layout_exact(&mut a);
}

/// The rows of a rendering, and the row of the first one containing `needle`.
fn row_of(text: &str, needle: &str) -> Option<usize> {
    text.lines().position(|l| l.contains(needle))
}

#[test]
fn the_unread_line_sits_between_read_and_new_posts() {
    let mut a = thread_app(false);
    let (text, buf) = render_at(&mut a, 100, 60);
    // In the gap above No.1003's card, the first post after the last visit (No.1002).
    let line = row_of(&text, "new posts").unwrap();
    assert!(row_of(&text, "No.1002").unwrap() < line && line + 2 == row_of(&text, "No.1003").unwrap(), "{text}");
    assert_eq!(text.matches("new posts").count(), 1);
    let x = text.lines().nth(line).unwrap().find("new posts").unwrap() as u16;
    assert_eq!(buf[(x, line as u16)].bg, theme().new);
    assert_layout_exact(&mut a);
    // Nothing new, or everything but the OP: no line, or the line right under the OP.
    for (after, below) in [(0, None), (1004, None), (1000, Some("No.1001"))] {
        let t = a.tab.thread.as_mut().unwrap();
        t.new_after = after;
        t.cache.clear();
        t.layout = None;
        let (text, _) = render_at(&mut a, 100, 60);
        match below {
            None => assert!(!text.contains("new posts"), "{text}"),
            Some(no) => assert_eq!(row_of(&text, "new posts").unwrap() + 2, row_of(&text, no).unwrap(), "{text}"),
        }
    }
}

#[test]
fn the_unread_line_keeps_long_threads_exact() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = long_thread_app(400);
    let t = a.tab.thread.as_mut().unwrap();
    // (A jump lands with the post at the margin, so the line above it is on screen.)
    (t.new_after, t.margin) = (2250, 0.3);
    render(&mut a);
    // U: the first new post, with the line right above it, all laid out exactly.
    a.on_key(KeyEvent::from(KeyCode::Char('.')));
    a.on_key(KeyEvent::from(KeyCode::Char('U')));
    let (text, _) = render(&mut a);
    assert_eq!(row_of(&text, "new posts").unwrap() + 2, row_of(&text, "No.2251").unwrap(), "{text}");
    assert_layout_exact(&mut a);
    // Reading on to the end and back, and from the end: the line moves with its post.
    a.on_key(KeyEvent::from(KeyCode::Char('G')));
    render(&mut a);
    assert_layout_exact(&mut a);
    for _ in 0..30 {
        a.on_key(KeyEvent::from(KeyCode::Char('K')));
        render(&mut a);
    }
    a.on_key(KeyEvent::from(KeyCode::Char('.')));
    a.on_key(KeyEvent::from(KeyCode::Char('U')));
    let (text, _) = render(&mut a);
    assert_eq!(row_of(&text, "new posts").unwrap() + 2, row_of(&text, "No.2251").unwrap(), "{text}");
    assert_layout_exact(&mut a);
    // In the end, a full layout to the line: the line took none.
    // gg: the top, as vim has it.
    a.on_key(KeyEvent::from(KeyCode::Char('g')));
    a.on_key(KeyEvent::from(KeyCode::Char('g')));
    for _ in 0..400 {
        a.on_key(KeyEvent::from(KeyCode::Char('j')));
        render(&mut a);
    }
    let clock = a.clock;
    let t = a.tab.thread.as_mut().unwrap();
    let l = t.layout.as_ref().map(|l| (l.width, l.thumbs_on)).unwrap();
    let full = crate::ui::layout_all(t, l.0, l.1, clock);
    assert_eq!(t.layout.as_ref().unwrap().starts, full.starts);
}

#[test]
fn a_tab_counts_its_watched_threads_new_posts() {
    let mut a = catalog_app(false);
    a.pick_row(View::Catalog, 1);
    a.new_tab();
    a.tab.thread = Some(thread());
    a.switch_tab(0);
    let (text, _) = render(&mut a);
    assert!(text.lines().nth(1).unwrap().contains("2 Snapshot thread (2)"), "{text}");
    // Read (or not watched): no count.
    a.store.watched_vec()[0].status = Status::READ;
    let (text, _) = render(&mut a);
    assert!(!text.lines().nth(1).unwrap().contains('('), "{text}");
    // A narrow tab keeps the count and cuts the subject.
    a.store.watched_vec()[0].status = Status::Live { unread: 12, replies: 0 };
    let (text, _) = render_at(&mut a, 30, 20);
    assert!(text.lines().nth(1).unwrap().contains("(12)"), "{text}");
}

/// A long thread: posts of different lengths, every one numbered in its text.
fn long_thread_app(n: u64) -> App {
    let mut a = app(false);
    a.tab.navigate(View::Thread);
    let posts: Vec<Post> = (0..n).map(|k| post(2000 + k, HOUR, None, &format!("post {k}<br>{}", "and a line<br>".repeat((k % 7) as usize)))).collect();
    a.tab.thread = Some(ThreadView::new(here(&a, "g", 2000), posts));
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
    // selection may have moved, kept on screen while scrolling: the same apart from it, and
    // the footer's ruler saying where it is).
    let unselected = |a: &mut App| {
        let text = render(a).0.replace('▌', " ");
        let lines: Vec<_> = text.lines().map(str::trim_end).collect();
        lines.get(..lines.len().saturating_sub(1)).unwrap_or_default().join("\n")
    };
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
    // gg: the top, as vim has it.
    a.on_key(KeyEvent::from(KeyCode::Char('g')));
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
    a.tab.navigate(View::Thread);
    let lines: String = (1..=70).map(|k| format!("line {k}<br>")).collect();
    let posts = vec![
        post(3000, HOUR, Some("Tall"), "first"),
        post(3001, HOUR, None, &format!("<a href=\"#p3000\" class=\"quotelink\">&gt;&gt;3000</a><br>tall start<br>{lines}tall end")),
        post(3002, HOUR, None, "<a href=\"#p3001\" class=\"quotelink\">&gt;&gt;3001</a><br>after"),
    ];
    a.tab.thread = Some(ThreadView::new(here(&a, "g", 3000), posts));
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
    key(&mut a, 'C');
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
        a.tab.thread = Some(ThreadView::new(here(&a, "g", 3000), p));
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
        // gg: the top, as vim has it.
        a.on_key(KeyEvent::from(KeyCode::Char('g')));
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
    a.tab.thread = Some(ThreadView::new(here(&a, "g", 1000), posts));
    type_text(&mut a, &label);
    assert!(a.tab.thread.as_ref().unwrap().focus.is_none());
    assert!(a.status().unwrap().text.contains("changed since the labels went up"));
}

#[test]
fn history() {
    let mut a = app(false);
    a.tab.navigate(View::History);
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
    a.tab.navigate(View::Settings);
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("settings_backgrounds", bg_map(&mut a));
}

#[test]
fn theme_picker() {
    let mut a = app(false);
    a.tab.navigate(View::Settings);
    a.activate_setting();
    assert!(matches!(a.settings_popup(), Some(SettingsPopup::Themes { .. })));
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn color_editor() {
    let mut a = app(false);
    a.tab.navigate(View::Settings);
    a.pick_row(View::Settings, 1);
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
    a.tab.navigate(View::Settings);
    let keys = crate::app::settings().count() - 1;
    a.pick_row(View::Settings, keys);
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
    a.rehide(|a| a.hiding.set_filters(crate::filter::tests::filters(cfg).unwrap()));
}

#[test]
fn filtered_catalog_and_thread() {
    let mut a = catalog_app(true);
    with_filters(&mut a);
    a.rehide(|a| a.store.toggle_hidden("4chan", "g", 1100));
    a.pick_row(View::Catalog, 0);
    insta::assert_snapshot!(snapshot(&mut a));
    // Z: hidden ones shown, marked.
    a.rehide(|a| a.hiding.toggle_show());
    insta::assert_snapshot!("filtered_catalog_shown", snapshot(&mut a));
    insta::assert_snapshot!("filtered_catalog_backgrounds", bg_map(&mut a));
    a.rehide(|a| a.hiding.toggle_show());
    a.tab.navigate(View::Thread);
    a.tab.thread = Some(thread());
    a.remark();
    insta::assert_snapshot!("filtered_thread", snapshot(&mut a));
}

#[test]
fn your_posts_and_replies() {
    let mut a = thread_app(false);
    a.rehide(|a| a.store.toggle_mine(&ThreadKey { site: "4chan".into(), board: "g".into(), no: 1000 }, 1001));
    a.store.watched_vec()[0].status = Status::Live { unread: 2, replies: 1 };
    insta::assert_snapshot!(snapshot(&mut a));
    a.tab.navigate(View::Watched);
    insta::assert_snapshot!("watched_with_replies", snapshot(&mut a));
}

#[test]
fn catalog_new_threads_and_replies() {
    let mut a = catalog_app(false);
    a.tab.catalog.new.insert(1100);
    a.store.opened("4chan", "g", 1000, 300, NOW);
    a.pick_row(View::Catalog, 1);
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn replies_inline() {
    let mut a = thread_app(true);
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('.')));
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('e')));
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('j')));
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('.')));
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('e')));
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("replies_inline_backgrounds", bg_map(&mut a));
}

#[test]
fn catalog_grid() {
    let mut a = catalog_app(true);
    a.default_layout = crate::config::CatalogLayout::Grid;
    a.pick_row(View::Catalog, 1);
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("catalog_grid_backgrounds", bg_map(&mut a));
    assert!(matches!(a.drawn.body, Some(Hit::Grid { cols: 4, .. })));
    // Without images, the grid is drawn as cards.
    let mut b = catalog_app(false);
    b.default_layout = crate::config::CatalogLayout::Grid;
    let text = snapshot(&mut b);
    assert!(text.contains("312 replies") && matches!(b.drawn.body, Some(Hit::List { .. })), "{text}");
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
    a.tab.site = a.site_index("desuarchive").unwrap();
    a.typing = Some(crate::app::Typing::ArchiveQuery("borrow".into()));
    a.tab.navigate(View::Catalog);
    insta::assert_snapshot!("archive_search_typing", snapshot(&mut a));
    a.typing = None;
    a.tab.navigate(View::Search);
    let page = crate::backend::foolfuuka::parse_search(&v).unwrap();
    a.tab.search = Some(crate::app::Search::for_tests("g", "rust borrow checker", page));
    a.pick_row(View::Search, 0);
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn hidden_search_results() {
    let mut a = app(false);
    a.tab.site = a.site_index("desuarchive").unwrap();
    a.tab.navigate(View::Search);
    let page = crate::backend::foolfuuka::parse_search(&crate::backend::fixture("foolfuuka_search.json")).unwrap();
    let first = page.hits[0].1.no;
    let nos: Vec<u64> = page.hits.iter().map(|(_, p)| p.no).collect();
    a.tab.search = Some(crate::app::Search::for_tests("g", "rust borrow checker", page));
    a.rehide(|a| a.store.toggle_hidden("desuarchive", "g", first));
    a.pick_row(View::Search, 0);
    let text = render(&mut a).0;
    assert!(!text.contains(&format!("No.{first}")) && text.contains("1 hidden"), "{text}");
    // Z: shown, marked.
    a.rehide(|a| a.hiding.toggle_show());
    let text = render(&mut a).0;
    assert!(text.lines().any(|l| l.contains(&format!("No.{first}")) && l.contains(" hidden ")), "{text}");
    // All hidden: says so, and how to see them.
    a.rehide(|a| a.hiding.toggle_show());
    for &no in nos.iter().skip(1) {
        a.rehide(|a| a.store.toggle_hidden("desuarchive", "g", no));
    }
    let text = render(&mut a).0;
    assert!(text.contains("All hidden (c Z shows them)"), "{text}");
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
    a.pick_row(View::Catalog, 1);
    a.new_tab();
    a.tab.thread = Some(thread());
    insta::assert_snapshot!(snapshot(&mut a));
    insta::assert_snapshot!("tabs_row_backgrounds", bg_map(&mut a));
}

#[test]
fn tab_chips_hidden_under_the_viewer_cant_be_clicked() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let mut a = catalog_app(false);
    a.pick_row(View::Catalog, 1);
    a.new_tab();
    snapshot(&mut a);
    let (chip, other) = a.drawn.tabs.iter().find(|&&(_, i)| i != a.active).copied().unwrap();
    // The viewer is drawn over the whole screen: the row the chips were on is its own.
    a.tab.popup = Some(crate::app::TabPopup::Viewer(Viewer::new(vec![file("op.png")], 0, None)));
    snapshot(&mut a);
    let active = a.active;
    let click = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: chip.x, row: chip.y, modifiers: KeyModifiers::NONE };
    a.on_mouse(click, std::time::Instant::now());
    assert!(a.active == active && active != other && a.tab.viewer().is_some());
}


#[test]
fn help_lists_the_keys_with_yours_and_scrolls_when_small() {
    let mut a = app(false);
    a.keys = toml::from_str("watch = \"Q\"\nexport = \"E\"").unwrap();
    a.popup = Some(Popup::Help(Default::default()));
    let (text, _) = render_at(&mut a, 220, 120);
    for line in ["Everywhere", "Gallery", "Image viewer", "Q                  watch / unwatch", "E                  save the thread as a page", "q, v               close"] {
        assert!(text.contains(line), "{line} missing:\n{text}");
    }
    // A command without a key is left to the menu.
    assert!(!text.contains("save all the post's files"), "{text}");
    let (text, _) = render_at(&mut a, 60, 20);
    assert!(text.contains("↓ more (j)"), "{text}");
}

#[test]
fn help_shows_its_descriptions_whole_just_above_two_columns_width() {
    let mut a = app(false);
    for w in [108, 109, 110] {
        a.popup = Some(Popup::Help(Default::default()));
        let (text, _) = render_at(&mut a, w, 30);
        for (cut, whole) in [("focus images, links, repli", "focus images, links, replies"), ("posts with files / no imag", "posts with files / no images")] {
            assert!(!text.contains(cut) || text.contains(whole), "{whole} is cut at {w} columns:\n{text}");
        }
    }
}

#[test]
fn narrow_screens() {
    let mut a = app(false);
    // Settings scroll to the selected one.
    a.tab.navigate(View::Settings);
    let last = crate::app::settings().count() - 1;
    a.pick_row(View::Settings, last);
    let (text, _) = render_at(&mut a, 60, 20);
    assert!(text.contains("Key bindings"), "{text}");
    // Long crumbs give way with an ellipsis, before the counts.
    a.tab.navigate(View::Thread);
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
    a.pick_row(View::Sites, 2);
    insta::assert_snapshot!(snapshot(&mut a));
    a.show_hidden_sites = true;
    a.pick_row(View::Sites, 8);
    insta::assert_snapshot!("home_showing_hidden_sites", snapshot(&mut a));
}

#[test]
fn watched_generals() {
    let mut a = app(false);
    a.tab.navigate(View::Watched);
    a.store.watched_vec()[0].general = Some("/lmg/".into());
    a.store.watched_vec()[0].at_limit = true;
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
    a.on_key(KeyEvent::from(KeyCode::Char('.')));
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
    let mut t = crate::app::ThreadView::new(here(&a, "g", 1000), posts);
    t.cache = std::mem::take(&mut a.tab.thread.as_mut().unwrap().cache);
    a.tab.thread = Some(t);
    render(&mut a);
    let after = blocks(&a);
    assert!(std::rc::Rc::ptr_eq(&before[1], &after[1]) && !std::rc::Rc::ptr_eq(&before[3], &after[3]));
    // A post that comes back changed (its file deleted) is laid out again (found by fuzzing).
    let mut posts = a.tab.thread.as_ref().unwrap().posts.clone();
    posts[0].files.clear();
    let mut t = crate::app::ThreadView::new(here(&a, "g", 1000), posts);
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
fn a_quote_of_a_hidden_post_peeks_at_nothing() {
    use crate::model::Target;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    with_filters(&mut a);
    a.remark();
    a.tab.thread.as_mut().unwrap().select(3);
    render(&mut a);
    // tab to No.1003's quote of No.1001, which a filter hides.
    for _ in 0..4 {
        if matches!(a.focused(), Some(Part::Link(Target::Quote(_)))) {
            break;
        }
        a.on_key(KeyEvent::from(KeyCode::Tab));
    }
    assert!(matches!(a.focused(), Some(Part::Link(Target::Quote(l))) if l.post == Some(1001)), "{:?}", a.focused());
    let text = render(&mut a).0;
    assert!(text.matches("No.1001  hidden").count() == 2 && !text.contains("implying"), "{text}");
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
    let label = h.targets.iter().find(|x| matches!(&x.to, crate::app::HintTo::Thread(p, Some(_)) if p == &[1001])).unwrap().label.clone();
    type_text(&mut a, &label);
    assert!(a.hints().is_none());
    assert_eq!(a.tab.thread.as_ref().unwrap().selected, 0);
    // In a catalog: a label per thread; picking one opens it.
    let mut c = catalog_app(false);
    render(&mut c);
    c.on_key(KeyEvent::from(KeyCode::Char('f')));
    insta::assert_snapshot!("link_hints_catalog", snapshot(&mut c));
    c.on_key(KeyEvent::from(KeyCode::Char('s')));
    assert_eq!((c.tab.view(), c.tab.pending_thread.as_ref().unwrap().no), (View::Thread, c.tab.catalog.posts()[1].no));
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
        post_url: None,
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
    let hit = |no, text: &str| Post { no, poster: "Anonymous".into(), time: NOW - HOUR, body: vec![ratatui::text::Line::from(text.to_string())], ..Default::default() };
    let hits = vec![(1000, hit(1002, "we were talking about rust and the borrow checker today")), (900, hit(900, "Rust or C++?"))];
    let mut s = Search::for_tests("", "rust", crate::backend::SearchPage { hits, total: None });
    s.saved = Some(SavedSearch::for_tests(vec![key("g", 1000), key("g", 900)], 2, 5, false));
    a.tab.search = Some(s);
    a.tab.navigate(View::Search);
    insta::assert_snapshot!(snapshot(&mut a));
}

#[test]
fn updating_a_built_in_sites_boards() {
    use crate::app::Adding;
    use crate::app::BoardsUpdate;
    use crate::config::BoardConfig;
    let mut a = app(false);
    let lain = a.site_index("lainchan").unwrap();
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
    a.rehide(|a| a.hiding.set_filters(crate::filter::Filters::new(&[]).unwrap().with_words(&a.hidden_words).unwrap()));
    a.rehide(|a| a.hiding.toggle_show());
    let text = render(&mut a).0;
    // Just "hidden": the label doesn't repeat the word it hides.
    assert!(text.contains(" hidden ") && !text.contains("hidden word"), "{text}");
}

#[test]
fn settings_list_popups() {
    use crate::app::MySites;
    use crate::config::{SiteConfig, SiteKind};
    let mut a = app(false);
    a.tab.navigate(View::Settings);
    let site = |name: &str| SiteConfig { name: name.into(), kind: SiteKind::Vichan, url: Some(format!("https://{name}.example")), boards: None, thumb_ext: None, archive: None, media_url: None, post_url: None };
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
        a.tab.popup = Some(crate::app::TabPopup::Preview(Preview { posts: vec![1001], elsewhere: vec![], scroll: Default::default() }));
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
            a.tab.navigate(View::Boards);
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
            a.tab.popup = Some(crate::app::TabPopup::Preview(Preview { posts: vec![1001], elsewhere: vec![7], scroll: Default::default() }));
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
            a.popup = Some(Popup::Help(Default::default()));
            a
        }),
        ("watched", || {
            let mut a = app(false);
            a.tab.navigate(View::Watched);
            a
        }),
        ("history", || {
            let mut a = app(false);
            a.tab.navigate(View::History);
            a
        }),
        ("settings", || {
            let mut a = app(false);
            a.tab.navigate(View::Settings);
            a
        }),
        ("theme picker", || {
            let mut a = app(false);
            a.tab.navigate(View::Settings);
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
    a.tab.navigate(View::Thread);
    a.thread_gone(&ThreadKey { site: "4chan".into(), board: "g".into(), no: 901 });
    let text = &a.status().unwrap().text;
    assert_eq!(text, "Thread was deleted or archived: enter opens it in desuarchive");
    // Not when the archive is known not to keep the board.
    let mut a = app(false);
    a.tab.navigate(View::Thread);
    let i = a.sites.iter().position(|s| s.cfg.name == "desuarchive").unwrap();
    a.sites[i].boards = Some(vec![crate::model::Board { uri: "a".into(), title: String::new(), nsfw: None }]);
    a.thread_gone(&ThreadKey { site: "4chan".into(), board: "g".into(), no: 901 });
    assert_eq!(a.status().unwrap().text, "Thread was deleted or archived");
}

#[test]
fn counts_of_one_are_singular() {
    let mut a = catalog_app(false);
    let mut posts = catalog();
    (posts[0].replies, posts[0].images) = (Some(1), Some(1));
    a.tab.catalog = catalog_here(&a, posts);
    let text = render(&mut a).0;
    assert!(text.contains("1 reply · 1 image ·"), "{text}");
}

#[test]
fn help_opens_on_the_keys_for_where_you_are() {
    let mut a = thread_app(false);
    a.popup = Some(Popup::Help(Default::default()));
    let (text, _) = render_at(&mut a, 60, 40);
    let at = |title| text.find(&format!("  {title}\n")).unwrap_or(usize::MAX);
    assert!(at("Thread") < at("Everywhere"), "{text}");
}

#[test]
fn footer_hints_drop_whole_and_keep_help() {
    for w in [30, 40, 60, 80] {
        let mut a = thread_app(false);
        let (text, _) = render_at(&mut a, w, 20);
        let line = text.lines().last().unwrap().trim_end();
        // The hints end with help; after them, only the ruler (where you are).
        let (hints, ruler) = line.split_once("? help").unwrap_or_else(|| panic!("{w}: {line:?}"));
        assert!(ruler.trim().is_empty() || ruler.trim().ends_with('%'), "{w}: {line:?}");
        let footer = format!("{hints}? help");
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
    a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('c')));
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
    a.popup = Some(Popup::Help(Default::default()));
    insta::assert_snapshot!("narrow_help", narrow(&mut a));
}

#[test]
fn huge_counts_from_the_data_files_dont_overflow() {
    // Seen by the data directory fuzzer: unread counts and sizes as a corrupt file has them.
    let mut a = app(false);
    for no in [1, 2] {
        let key = ThreadKey { site: "4chan".into(), board: "g".into(), no };
        a.store.watch(key.clone(), "t".into(), 1, 1);
        let w = a.store.watched_mut(&key).unwrap();
        w.status = Status::Live { unread: usize::MAX, replies: usize::MAX };
    }
    render(&mut a);
    a.tab.navigate(View::Watched);
    render(&mut a);
    assert!(a.terminal_title().unwrap().starts_with(&format!("ck: ({}) (You) ", usize::MAX)));
}

#[test]
fn footer_offers_what_the_selected_post_has() {
    let footer = |a: &mut App| render(a).0.lines().last().unwrap().to_string();
    let mut a = thread_app(false);
    // The OP has a file: enter views it.
    a.tab.thread.as_mut().unwrap().selected = 0;
    let f = footer(&mut a);
    assert!(f.contains("enter view image") && !f.contains("enter quote"), "{f}");
    // A reply quoting the OP, without files: enter follows the quote.
    let t = a.tab.thread.as_mut().unwrap();
    let i = t.posts.iter().position(|p| p.files.is_empty() && !p.quotes.is_empty()).unwrap();
    t.select(i);
    let f = footer(&mut a);
    assert!(f.contains("enter quote") && !f.contains("view image"), "{f}");
    // The catalog: enter opens the thread.
    let mut a = catalog_app(false);
    assert!(footer(&mut a).contains("enter open"));
}

#[test]
fn arabic_text_fits_where_it_is_drawn() {
    // unicode-width counts "لا" as one cell; it's drawn in two, and measured so.
    let mut a = thread_app(false);
    let t = a.tab.thread.as_mut().unwrap();
    t.posts[0].body = vec![ratatui::text::Line::from("لا ".repeat(36))];
    t.cache.clear();
    t.layout = None;
    let (text, _) = render_at(&mut a, 60, 30);
    assert_eq!(text.matches("لا").count(), 36, "{text}");
    let cut = crate::ui::truncate(&"لا".repeat(10), 7);
    assert_eq!((cut.as_str(), crate::markup::columns(&cut)), ("لالالا…", 7));
}

#[test]
fn a_quote_of_a_hidden_post_previews_only_hidden() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    with_filters(&mut a);
    a.remark();
    a.tab.thread.as_mut().unwrap().select(3);
    // p on No.1003, which quotes No.1001, which a filter hides.
    a.on_key(KeyEvent::from(KeyCode::Char('p')));
    assert!(matches!(a.tab.popup, Some(crate::app::TabPopup::Preview(_))));
    let text = render(&mut a).0;
    assert!(text.contains("Quoted posts") && !text.contains("implying"), "{text}");
}

#[test]
fn the_catalog_header_counts_no_hidden_thread_as_new() {
    let mut a = catalog_app(false);
    a.tab.catalog.new.insert(1100);
    a.rehide(|a| a.store.toggle_hidden("4chan", "g", 1100));
    let text = render(&mut a).0;
    let top = text.lines().next().unwrap();
    assert!(top.contains("1 hidden") && !top.contains("new"), "{top}");
}

#[test]
fn no_gallery_hint_when_only_hidden_posts_have_files() {
    let mut a = thread_app(false);
    // Only the OP has a file; with images of No.1003's instead, hidden by hand.
    let t = a.tab.thread.as_mut().unwrap();
    let file = t.posts[0].files.clone();
    t.posts[0].files.clear();
    t.posts[3].files = file;
    a.rehide(|a| a.store.toggle_hidden("4chan", "g", 1003));
    let text = render(&mut a).0;
    assert!(!text.lines().last().unwrap().contains("gallery"), "{text}");
}

/// A dead watched thread's last counts (unread posts, replies to you) aren't new anywhere:
/// not on the Sites screen's Watched row, the Watched view's bar, its own row, nor the
/// terminal title. One live thread with 2 new is all there is. The dead one comes from a
/// watched.json written when a dead thread could keep its counts: they're dropped on load.
#[test]
fn a_dead_watched_threads_counts_show_nowhere() {
    let mut a = app(false);
    let old = r#"{"site":"4chan","board":"g","no":900,"subject":"Old thread","posts":300,"last_seen":1199,"unread":3,"dead":true,"replies":2}"#;
    a.store.watched_vec()[1] = serde_json::from_str(old).unwrap();
    let line = |text: &str, has: &str| text.lines().find(|l| l.contains(has)).unwrap_or_default().to_string();
    a.tab.navigate(View::Sites);
    let sites = render(&mut a).0;
    let row = line(&sites, "Watched");
    assert!(row.contains("2 new") && !row.contains("5 new"), "Sites row: {row:?}");
    a.tab.navigate(View::Watched);
    let watched = render(&mut a).0;
    let bar = watched.lines().next().unwrap_or_default().to_string();
    assert!(bar.contains("2 new") && !bar.contains("5 new"), "Watched bar: {bar:?}");
    let old = line(&watched, "Old thread");
    assert!(old.contains("archived/deleted") && !old.contains("repl"), "dead row: {old:?}");
    // The title agrees: 2 new, and no reply to you.
    assert_eq!(a.terminal_title().as_deref(), Some("ck: (2) Watched"));
}

// ----- text measured as it's drawn: one grapheme at a time (markup::columns) -----

/// Text unicode-width measures at half its drawn cells: it counts "لا" as one cell, and
/// ratatui draws it in two.
fn lam_alef(n: usize) -> String {
    "لا".repeat(n)
}

/// The cell `needle` starts at on row `y`, as drawn.
fn cell_of(buf: &Buffer, y: u16, needle: &str) -> Option<u16> {
    let row: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
    (0..row.len()).find(|&x| row.get(x..).is_some_and(|rest| rest.concat().starts_with(needle))).map(|x| x as u16)
}

#[test]
fn narrow_catalog_keeps_its_counts_under_an_arabic_subject() {
    // Subjects that fit as unicode-width measures them, but not as they're drawn.
    for n in 12..=22 {
        for compact in [false, true] {
            let mut a = catalog_app(false);
            if compact {
                a.default_layout = crate::config::CatalogLayout::Compact;
            }
            let mut posts = catalog();
            posts[1].subject = Some(lam_alef(n));
            a.tab.catalog = catalog_here(&a, posts);
            let text = narrow(&mut a);
            let row = text.lines().find(|l| l.contains("لا")).unwrap();
            // The subject gives way to the counts, as an ASCII one does.
            assert!(row.contains("312"), "{n} compact={compact}\n{text}");
        }
    }
}

#[test]
fn a_tab_chip_keeps_its_unread_count_under_an_arabic_subject() {
    let mut a = thread_app(false);
    a.tab.thread.as_mut().unwrap().posts[0].subject = Some(lam_alef(30));
    a.tabs.push(crate::app::Tab::new(0, std::time::Instant::now()));
    assert_eq!(a.tab_unread(0), Some(2));
    let (text, _) = render(&mut a);
    // "kept whatever the label is cut to"
    let chips = text.lines().nth(1).unwrap();
    assert!(chips.contains("(2)"), "{text}");
}

#[test]
fn site_columns_line_up_under_a_japanese_site_name() {
    let mut a = app(false);
    a.sites[1].cfg.name = "ふたば".into();
    let (text, buf) = render(&mut a);
    let rows: Vec<u16> = (0..buf.area.height).filter(|&y| cell_of(&buf, y, "https://").is_some()).collect();
    let urls: Vec<_> = rows.iter().map(|&y| cell_of(&buf, y, "https://")).collect();
    // The url column starts at the same cell on every site's row.
    assert!(urls.windows(2).all(|w| w.first() == w.get(1)), "{urls:?}\n{text}");
}

#[test]
fn the_menu_is_wide_enough_for_an_arabic_file_name() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    let name = format!("{}.png", lam_alef(25));
    let t = a.tab.thread.as_mut().unwrap();
    t.posts[0].files[0].filename = name.clone();
    t.focus = Some(Part::File(0));
    render(&mut a);
    a.on_key(KeyEvent::from(KeyCode::Char('.')));
    assert!(a.menu().is_some_and(|m| m.items.iter().any(|i| matches!(i, crate::app::MenuItem::Enter(l) if l.contains(&name)))));
    let text = render(&mut a).0;
    // The panel is as wide as the label drawn in it.
    assert!(text.contains(&name), "{text}");
}

#[test]
fn wrapped_paths_fit_their_width_as_drawn() {
    // "❤️" (a heart and an emoji-style selector) is drawn in two cells; each char alone is one.
    let path = format!("/saves/{}/thread.html", "❤\u{fe0f}".repeat(12));
    for line in crate::ui::wrap_path(&path, 10) {
        assert!(crate::markup::columns(&line) <= 10, "{line:?} is {} cells", crate::markup::columns(&line));
    }
    // Every line draws something, even one too wide; a slash keeps the mark that goes with it.
    assert_eq!(crate::ui::wrap_path("\u{200b}❤\u{fe0f}ab", 1), ["\u{200b}❤\u{fe0f}", "a", "b"]);
    assert_eq!(crate::ui::wrap_path("abc/\u{301}de", 5), ["abc/\u{301}d", "e"]);
}

#[test]
fn ui_pads_columns_by_cells() {
    // std pads a `{:N}` placeholder by chars, and a char isn't a cell: "ふたば" is three chars
    // in six. Any width (`{:16}`, `{:>16}`, `{x:w$}`) counts; the `0` flag pads numbers only.
    fn width_spec(line: &str) -> bool {
        line.split('{').skip(1).filter_map(|p| p.split_once('}')).filter_map(|(inner, _)| inner.split_once(':')).any(|(arg, spec)| {
            let arg_ok = arg.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.');
            let mut rest = spec;
            let mut it = rest.chars();
            if let (Some(_), Some('<' | '^' | '>')) = (it.next(), it.next()) {
                rest = it.as_str();
            } else {
                rest = rest.trim_start_matches(['<', '^', '>']);
            }
            let rest = rest.trim_start_matches(['+', '-']).trim_start_matches('#');
            let named = rest.split_once('$').is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_'));
            arg_ok && !rest.starts_with('0') && (rest.starts_with(|c: char| c.is_ascii_digit()) || rest.starts_with('*') || named)
        })
    }
    assert!(width_spec(r#"format!("{:<16}", x)"#) && width_spec(r#"format!("{s:16}")"#) && width_spec(r#"format!("{s:>w$}")"#));
    assert!(!width_spec(r#"format!("{:02x}{:.1}{x:?}{}", a, b)"#) && !width_spec("Rect { x: 0, y: 1 }") && !width_spec("Vec::<u8>::new()"));
    // All of src but the test files, which pad expected output on purpose, and the input log,
    // which pads a number in a file.
    let mut dirs = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
    let mut checked = 0;
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy();
            if !name.ends_with(".rs") || ["tests.rs", "ui_tests.rs", "fuzz.rs", "e2e.rs", "bench.rs", "input_log.rs"].contains(&&*name) {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            // A source file's own test module sits at its end (other `#[cfg(test)]` items are
            // checked like the rest).
            let lines: Vec<&str> = src.lines().collect();
            let module = |i: usize| lines.iter().skip(i + 1).find(|l| !l.starts_with("#[")).is_some_and(|l| l.starts_with("mod ") && l.ends_with('{'));
            let tests = (0..lines.len()).find(|&i| lines.get(i) == Some(&"#[cfg(test)]") && module(i)).unwrap_or(lines.len());
            for (n, line) in lines.iter().take(tests).enumerate() {
                assert!(!width_spec(line), "{}:{}: pad by cells with ui::pad / ui::col, not a {{:N}} width:\n{line}", path.display(), n + 1);
            }
            checked += 1;
        }
    }
    assert!(checked > 20, "only {checked} files checked");
    use crate::markup;
    use crate::ui::{col, pad};
    assert_eq!(markup::columns(&pad("ふたば", 16)), 16);
    assert_eq!(pad("a long name", 4), "a long name");
    let cut = col("ふたばちゃんねるの画像掲示板", 16);
    assert!(cut.trim_end().ends_with('…') && markup::columns(&cut) == 16, "{cut:?}");
    assert_eq!(col("Watched", 16), format!("{:<16}", "Watched"));
    // A column with no room is blank, not an ellipsis sticking out of it.
    assert_eq!((col("Watched", 1), col("Watched", 0)), (" ".to_string(), String::new()));
}

#[test]
fn the_footer_message_is_drawn_exactly_when_the_app_counts_it_seen() {
    use crate::app::Typing;
    let typing = [None, Some(Typing::Goto("a/".into())), Some(Typing::ThreadSearch), Some(Typing::ListFilter)];
    for (typing, loading) in typing.into_iter().flat_map(|t| [(t.clone(), None), (t, Some("Loading".to_string()))]) {
        let mut app = catalog_app(false);
        app.error("Boom");
        app.typing = typing.clone();
        if let Some(l) = &loading {
            app.tab.fake_load(1, l, crate::app::Then::Show);
        }
        let (text, _) = render(&mut app);
        assert_eq!(text.contains("Boom"), app.status_on_screen(), "typing {typing:?}, loading {loading:?}");
    }
}

// A panel that scrolls keeps its scroll where the screen shows it: k right after
// overscrolling moves the view, the selection stays in view, and rows stay in the panel.

#[test]
fn k_after_overscrolling_help_scrolls_back_at_once() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = app(false);
    a.popup = Some(Popup::Help(Default::default()));
    render_at(&mut a, 60, 20);
    for _ in 0..100 {
        a.on_key(KeyEvent::from(KeyCode::Char('j')));
        render_at(&mut a, 60, 20);
    }
    let bottom = render_at(&mut a, 60, 20).0;
    a.on_key(KeyEvent::from(KeyCode::Char('k')));
    let after = render_at(&mut a, 60, 20).0;
    assert_ne!(bottom, after, "k at the end of the help didn't scroll:\n{after}");
}

#[test]
fn k_after_paging_past_a_quote_preview_scrolls_back_at_once() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = tall_app();
    a.tab.thread.as_mut().unwrap().selected = 2;
    a.on_key(KeyEvent::from(KeyCode::Char('p')));
    assert!(matches!(a.tab.popup, Some(crate::app::TabPopup::Preview(_))));
    render_at(&mut a, 100, 30);
    for _ in 0..20 {
        a.on_key(KeyEvent::from(KeyCode::PageDown));
        render_at(&mut a, 100, 30);
    }
    let bottom = render_at(&mut a, 100, 30).0;
    assert!(bottom.contains("tall end"), "{bottom}");
    a.on_key(KeyEvent::from(KeyCode::Char('k')));
    let after = render_at(&mut a, 100, 30).0;
    assert_ne!(bottom, after, "k at the end of the preview didn't scroll:\n{after}");
}

#[test]
fn the_selected_theme_stays_in_view_on_a_short_screen() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = app(false);
    // The built-in themes and a config's own: more than a 20-row screen's panel holds.
    for k in 0..10 {
        a.themes.insert(format!("custom-{k}"), crate::theme::ThemeDef::default());
    }
    a.tab.navigate(View::Settings);
    a.activate_setting();
    let Some(SettingsPopup::Themes { names, .. }) = a.settings_popup() else { panic!("no theme picker") };
    let names = names.clone();
    assert!(names.len() > 16, "{names:?}");
    render_at(&mut a, 100, 20);
    for (k, name) in names.iter().enumerate().skip(1) {
        a.on_key(KeyEvent::from(KeyCode::Char('j')));
        let text = render_at(&mut a, 100, 20).0;
        let Some(SettingsPopup::Themes { list, .. }) = a.settings_popup() else { panic!("no theme picker") };
        assert_eq!(list.selected(), Some(k));
        assert!(text.contains(name.as_str()), "theme {k} ({name}) is selected but not shown:\n{text}");
    }
}

#[test]
fn the_add_filter_popup_draws_only_inside_its_panel_on_a_short_screen() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    let p = &mut a.tab.thread.as_mut().unwrap().posts[0];
    p.poster = crate::model::Poster::new(Some("Named".into()), "Anonymous", Some("!Trip".into()), None);
    p.id = Some("abcd1234".into());
    p.flag = Some(crate::model::Flag { code: "US".into(), name: "United States".into() });
    p.files[0].md5 = Some("u8Vh17KxaDvUJ6bBcmE/eg==".into());
    a.on_key(KeyEvent::from(KeyCode::Char('.')));
    a.on_key(KeyEvent::from(KeyCode::Char('X')));
    let Some(Popup::AddFilter(f)) = &a.popup else { panic!("no add-filter popup") };
    assert_eq!(f.candidates.len(), 7);
    let (text, buf) = render_at(&mut a, 100, 15);
    let panel = theme().surface_highest;
    // Whatever of the popup's own rows shows sits on the panel, not over the thread.
    let shown: Vec<_> = ["hide them", "all of 4chan", "w: a word from it", "Saved in the config"].into_iter().filter_map(|n| Some((n, row_of(&text, n)?))).collect();
    assert!(!shown.is_empty(), "none of the popup's rows is drawn:\n{text}");
    for (needle, y) in shown {
        let x = text.lines().nth(y).and_then(|l| l.split(needle).next()).unwrap().chars().count();
        assert_eq!(buf[(x as u16, y as u16)].bg, panel, "{needle:?} is drawn outside the panel, on row {y}:\n{text}");
    }
}

#[test]
fn reply_box() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut a = thread_app(false);
    a.on_key(KeyEvent::from(KeyCode::Char('r')));
    type_text(&mut a, "hello");
    insta::assert_snapshot!(snapshot(&mut a));
}

