//! The app's behavior, driven through keys, clicks and messages.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::*;
use crate::keys::ACTIONS;
use crate::test_fixtures::*;

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
}

#[test]
fn finds_programs_on_path() {
    assert!(super::on_path("sh"));
    assert!(!super::on_path("ck-no-such-program"));
}

#[test]
fn mouse_wheel_click_and_double_click() {
    let mut app = test_app();
    app.hit = Some(Hit::List { area: Rect::new(1, 2, 60, 10), offset: 0, item_height: 1 });
    let t0 = Instant::now();
    app.on_mouse(mouse(MouseEventKind::ScrollDown, 5, 5), t0);
    assert_eq!(app.site_list.state.selected(), Some(1));
    app.on_mouse(mouse(MouseEventKind::ScrollUp, 5, 5), t0);
    assert_eq!(app.site_list.state.selected(), Some(0));

    // Row 3 is the second item (History).
    let left = MouseEventKind::Down(MouseButton::Left);
    app.on_mouse(mouse(left, 5, 3), t0);
    assert_eq!(app.site_list.state.selected(), Some(1));
    assert_eq!(app.tab.view, View::Sites);
    // A slow second click is just another click; a quick one opens.
    app.on_mouse(mouse(left, 5, 3), t0 + Duration::from_secs(1));
    assert_eq!(app.tab.view, View::Sites);
    app.on_mouse(mouse(left, 5, 3), t0 + Duration::from_millis(1200));
    assert_eq!(app.tab.view, View::History);

    // Clicks outside the list do nothing.
    app.tab.view = View::Sites;
    app.on_mouse(mouse(left, 5, 30), t0);
    assert_eq!(app.site_list.state.selected(), Some(1));
}

#[test]
fn mouse_click_selects_thread_post() {
    let mut app = test_app();
    let post = |no| Post { no, body: vec![Line::raw("a"), Line::raw("b")], ..Default::default() };
    let mut t = ThreadView::new("g".into(), 1, vec![post(1), post(2), post(3)]);
    // Each post: header, two lines, a blank.
    let block: Rc<[Line]> = vec![Line::raw(""); 4].into();
    t.layout = Some(ThreadLayout::of_blocks(40, vec![block.clone(), block.clone(), block]));
    t.viewport = 10;
    app.tab.thread = Some(t);
    app.tab.view = View::Thread;
    app.hit = Some(Hit::Thread { area: Rect::new(0, 1, 40, 10) });
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 1 + 9), Instant::now());
    assert_eq!(app.tab.thread.as_ref().unwrap().selected, 2);
    app.on_mouse(mouse(MouseEventKind::ScrollDown, 3, 3), Instant::now());
    assert_eq!(app.tab.thread.as_ref().unwrap().scroll, 2);
}

#[test]
fn sleeps_until_the_next_thing_to_do() {
    let mut app = test_app();
    let now = Instant::now();
    // Idle: at most a second.
    assert_eq!(app.next_wake(now), Duration::from_secs(1));
    // A spinner animates.
    app.tab.loading = Some("Loading".into());
    assert_eq!(app.next_wake(now), Duration::from_millis(100));
    app.tab.loading = None;
    // A status message wakes the loop when it's due to disappear.
    app.info("hi");
    app.status_since = Some(("hi".into(), now - Duration::from_millis(1700)));
    assert_eq!(app.next_wake(now), Duration::from_millis(300));
    app.status = None;
    app.status_since = None;
    // A watched thread that was never refreshed is due now.
    let key = ThreadKey { site: "4chan".into(), board: "g".into(), no: 1 };
    app.store.watched.push(crate::store::Watched { key, posts: 1, last_seen: 1, ..Default::default() });
    assert_eq!(app.next_wake(now), Duration::ZERO);
    // But while the maximum number of refreshes is running, due ones don't spin the loop.
    for no in [2, 3] {
        app.refreshing.insert(ThreadKey { site: "4chan".into(), board: "g".into(), no });
    }
    assert_eq!(app.next_wake(now), Duration::from_millis(100));
}

#[test]
fn wakes_as_soon_as_a_message_arrives() {
    let mut app = test_app();
    let tx = app.tx.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        let _ = tx.send(Msg::Wake);
    });
    let start = Instant::now();
    assert!(app.wait(Duration::from_secs(5)));
    assert!(start.elapsed() < Duration::from_millis(500), "{:?}", start.elapsed());
    // With nothing to wait for, it times out.
    assert!(!app.wait(Duration::from_millis(10)));
}

#[test]
fn partial_pages_show_while_loading_continues() {
    let mut app = test_app();
    let board = |uri: &str| Board { uri: uri.into(), title: String::new(), nsfw: None };
    app.tab.req = 7;
    app.tab.loading = Some("Loading boards".into());
    app.handle(Msg::BoardsPartial(7, 0, vec![board("a")]));
    assert_eq!(app.sites[0].boards.as_ref().unwrap().len(), 1);
    assert!(app.tab.loading.is_some());
    // A stale request's pages are ignored.
    app.handle(Msg::BoardsPartial(6, 0, vec![board("x"), board("y"), board("z")]));
    assert_eq!(app.sites[0].boards.as_ref().unwrap().len(), 1);
    app.handle(Msg::Boards(7, 0, Ok(vec![board("a"), board("b")])));
    assert_eq!(app.sites[0].boards.as_ref().unwrap().len(), 2);
    assert!(app.tab.loading.is_none());

    app.tab.req = 8;
    app.tab.loading = Some("Loading /a/".into());
    app.handle(Msg::CatalogPartial(8, vec![Post { no: 1, ..Default::default() }]));
    assert_eq!((app.tab.catalog.len(), app.tab.loading.is_some()), (1, true));
}

#[test]
fn overboard_threads_open_on_their_board_and_back_returns() {
    // A local site that refuses connections: nothing leaves the machine.
    let mut app = app_with("[[site]]\nname = \"t\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:9\"\nboards = [\"ob\"]");
    app.tab.board = Some(Board { uri: "ob".into(), title: "Overboard".into(), nsfw: None });
    app.load_catalog();
    app.tab.catalog = vec![Post { no: 5, board: Some("tech".into()), ..Default::default() }];
    app.tab.catalog_list.state.select(Some(0));
    app.tab.view = View::Catalog;
    app.enter();
    assert_eq!((app.tab.view, app.tab.board.as_ref().unwrap().uri.as_str()), (View::Thread, "tech"));
    assert_eq!(app.tab.pending_thread, 5);
    app.back();
    assert_eq!((app.tab.view, app.tab.board.as_ref().unwrap().uri.as_str()), (View::Catalog, "ob"));
    // The overboard's catalog is still there; nothing was reloaded.
    assert_eq!(app.tab.catalog.len(), 1);
}

#[test]
fn key_editor_rebinds_saves_and_refuses_clashes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = test_app();
    app.config_path = Some(dir.path().join("config.toml"));
    let press = |app: &mut App, code| app.on_key(KeyEvent::from(code));
    app.open_settings();
    app.settings_list.state.select(Some(settings::position("Key bindings").unwrap()));
    app.activate_setting();
    // Move to `watch` and rebind it to W.
    let rows = settings::key_rows();
    let watch = rows.iter().position(|r| *r == Ok(ACTIONS.iter().position(|e| e.0 == Action::Watch).unwrap())).unwrap();
    while app.settings_popup().is_some_and(|p| !matches!(p, SettingsPopup::Keys { list, .. } if list.selected() == Some(watch))) {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('W'));
    assert_eq!(app.keys.label(Action::Watch), "W");
    // `a` adds a second key.
    press(&mut app, KeyCode::Char('a'));
    app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT));
    assert_eq!(app.keys.label(Action::Watch), "W, alt-w");
    let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(text.contains(r#"watch = ["W", "alt-w"]"#), "{text}");
    // A key another command uses in the same view is refused.
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('v'));
    assert_eq!(app.keys.label(Action::Watch), "W, alt-w");
    assert!(app.status.as_ref().is_some_and(|s| s.error && s.text.contains("'v'")), "{:?}", app.status);
    // x resets to the default, which removes the entry.
    press(&mut app, KeyCode::Char('x'));
    assert!(app.keys.is_default(Action::Watch));
    let c: Config = toml::from_str(&std::fs::read_to_string(dir.path().join("config.toml")).unwrap()).unwrap();
    assert!(!c.keys.contains_key("watch"));
    // The new keys work at once.
    app.popup = None;
    app.tab.view = View::Catalog;
    assert_eq!(app.keys.action(app.scope(), &KeyEvent::from(KeyCode::Char('w'))), Some(Action::Watch));
}

#[test]
fn copies_text_and_links() {
    let mut app = test_app();
    let html = "<a href=\"#p1\" class=\"quotelink\">&gt;&gt;1</a><br><span class=\"quote\">&gt;green</span><br><s>secret</s> text";
    let parsed = crate::markup::parse_html(html, crate::markup::Flavor::Fourchan);
    let post = |no, body: Vec<Line<'static>>| Post { no, subject: Some("Subj".into()), body, ..Default::default() };
    app.tab.board = Some(Board { uri: "g".into(), title: String::new(), nsfw: None });
    app.tab.thread = Some(ThreadView::new("g".into(), 1, vec![post(1, vec![Line::raw("op")]), post(2, parsed.lines)]));
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.tab.view = View::Thread;
    app.act(Action::Copy);
    assert_eq!(app.copied.as_deref(), Some(">>1\n>green\nsecret text"));
    assert_eq!(app.status.as_ref().unwrap().text, "Copied 22 characters");
    app.act(Action::CopyLink);
    assert_eq!(app.copied.as_deref(), Some("https://boards.4chan.org/g/thread/1#p2"));
    // The viewer copies the file's URL, or the post's link.
    app.tab.thread.as_mut().unwrap().posts[1].files = vec![Attachment { url: "https://i.4cdn.org/g/1.png".into(), ..Default::default() }];
    app.images = crate::images::Images::offline();
    app.act(Action::View);
    app.on_key(KeyEvent::from(KeyCode::Char('y')));
    assert_eq!(app.copied.as_deref(), Some("https://i.4cdn.org/g/1.png"));
    app.on_key(KeyEvent::from(KeyCode::Char('Y')));
    assert_eq!(app.copied.as_deref(), Some("https://boards.4chan.org/g/thread/1#p2"));
    app.tab.popup = None;
    // Catalog: subject and text; a thread link.
    app.tab.catalog = vec![post(7, vec![Line::raw("hello")])];
    app.tab.catalog_list.state.select(Some(0));
    app.tab.view = View::Catalog;
    app.act(Action::Copy);
    assert_eq!(app.copied.as_deref(), Some("Subj\nhello"));
    app.act(Action::CopyLink);
    assert_eq!(app.copied.as_deref(), Some("https://boards.4chan.org/g/thread/7"));
}

#[test]
fn goto_opens_places_and_u_comes_back() {
    let mut app = local_app();
    app.goto_str("b/y/5#6");
    assert_eq!((app.tab.site, app.tab.view, app.tab.pending_thread, app.tab.pending_post), (1, View::Thread, 5, Some(6)));
    assert_eq!(app.tab.board.as_ref().unwrap().uri, "y");
    // Esc from there returns to where : was typed.
    assert_eq!(app.tab.return_to, Some(View::Sites));
    // From a thread, `u` comes back across sites.
    app.tab.thread = Some(ThreadView::new("y".into(), 5, vec![Post { no: 5, ..Default::default() }, Post { no: 6, ..Default::default() }]));
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.goto_str("http://127.0.0.1:3/x/res/3.html#4");
    assert_eq!((app.tab.site, app.tab.pending_thread, app.tab.board.as_ref().unwrap().uri.as_str()), (0, 3, "x"));
    app.tab.thread = None;
    app.act(Action::JumpBack);
    assert!(app.tab.thread.is_none());
    // (JumpBack needs a loaded thread; simulate the arrival of thread 3.)
    app.tab.thread = Some(ThreadView::new("x".into(), 3, vec![Post { no: 3, ..Default::default() }]));
    app.act(Action::JumpBack);
    assert_eq!((app.tab.site, app.tab.pending_thread, app.tab.pending_post, app.tab.board.as_ref().unwrap().uri.as_str()), (1, 5, Some(6), "y"));
    // A board opens its catalog; a site its boards.
    app.goto_str("a/xy");
    assert_eq!((app.tab.site, app.tab.view, app.tab.board.as_ref().unwrap().uri.as_str()), (0, View::Catalog, "xy"));
    app.goto_str("b");
    assert_eq!((app.tab.site, app.tab.view), (1, View::Boards));
    // Errors are said, not acted on.
    app.goto_str("a/x/abc");
    assert!(app.status.as_ref().unwrap().error);
    assert_eq!(app.tab.view, View::Boards);
    // A link to a site ck doesn't have: it asks the site what it runs, to add it. Also
    // without a scheme or a path (not a board of the current site called that).
    for link in ["https://example.com/g/", "example.com"] {
        app.popup = None;
        app.goto_str(link);
        assert!(matches!(app.adding(), Some(Adding::Looking { host, .. }) if host == "example.com"), "{link}");
        assert_eq!(app.tab.view, View::Boards);
    }
    app.popup = None;
}

#[test]
fn goto_input_completes_and_takes_pastes() {
    let mut app = local_app();
    app.act(Action::Goto);
    app.paste("a/");
    app.on_key(KeyEvent::from(KeyCode::Tab));
    // x and xy: completes the common part and lists both.
    assert_eq!(app.goto.as_deref(), Some("a/x"));
    assert!(app.status.as_ref().unwrap().text.contains("xy"));
    app.on_key(KeyEvent::from(KeyCode::Char('y')));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!((app.goto.as_deref(), app.tab.view, app.tab.board.as_ref().unwrap().uri.as_str()), (None, View::Catalog, "xy"));
    // Site names complete with a slash.
    app.act(Action::Goto);
    app.on_key(KeyEvent::from(KeyCode::Char('b')));
    app.on_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(app.goto.as_deref(), Some("b/"));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    // A paste with nothing being typed starts the input.
    app.paste("http://localhost:3/y/res/1.html\n");
    assert_eq!(app.goto.as_deref(), Some("http://localhost:3/y/res/1.html"));
}

#[test]
fn links_panel_lists_and_opens() {
    let mut app = local_app();
    app.switch_site(0);
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let html = r#"<a href="/x/res/1.html#1" class="quotelink">&gt;&gt;1</a> <a href="/xy/res/9.html#10">&gt;&gt;&gt;/xy/10</a> see https://example.com/a"#;
    let parsed = crate::markup::parse_html(html, crate::markup::Flavor::Vichan);
    let reply = Post { no: 2, files: vec![Attachment { filename: "a.png".into(), url: "http://127.0.0.1:3/x/src/a.png".into(), ..Default::default() }], ..parsed.into() };
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }, reply]));
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.tab.view = View::Thread;
    app.act(Action::Links);
    // The quote of a post in this thread isn't listed; the other board's is.
    let kinds: Vec<String> = (match &app.tab.popup { Some(crate::app::TabPopup::Links(l)) => l, _ => panic!("no links panel") }).items.iter().map(|i| match i {
        LinkItem::Quote(_, label) => label.clone(),
        LinkItem::Url(u) => u.clone(),
        LinkItem::File(f) => f.filename.clone(),
    }).collect();
    assert_eq!(kinds, [">>>/xy/10  (thread 9)", "https://example.com/a", "a.png"]);
    // y copies the selected link; enter on a web link opens it.
    app.on_key(KeyEvent::from(KeyCode::Down));
    app.on_key(KeyEvent::from(KeyCode::Char('y')));
    assert_eq!(app.copied.as_deref(), Some("https://example.com/a"));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(!matches!(app.tab.popup, Some(crate::app::TabPopup::Links(_))));
    assert_eq!(app.opened.as_deref(), Some("https://example.com/a"));
    // Enter on the quote opens its thread, and `u` will come back.
    app.act(Action::Links);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!((app.tab.board.as_ref().unwrap().uri.as_str(), app.tab.pending_thread, app.tab.pending_post), ("xy", 9, Some(10)));
    assert_eq!(app.tab.trail.len(), 1);
    // A post without links says so.
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }]));
    app.act(Action::Links);
    assert!(!matches!(app.tab.popup, Some(crate::app::TabPopup::Links(_))) && app.status.as_ref().unwrap().text == "Post has no links");
}

#[test]
fn filters_and_hiding() {
    let mut app = local_app();
    let cfg = "[[filter]]\npattern = \"(?i)spam\"\nlabel = \"spam\"\n[[filter]]\npattern = \"rust\"\naction = \"highlight\"";
    app.filters = crate::filter::tests::filters(cfg).unwrap();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.catalog_board = "x".into();
    let op = |no, subject: &str| Post { no, subject: Some(subject.into()), ..Default::default() };
    app.tab.catalog = vec![op(1, "SPAM here"), op(2, "rust thread"), op(3, "other")];
    app.remark_catalog();
    app.tab.view = View::Catalog;
    assert_eq!(app.visible_catalog(), [1, 2]);
    assert_eq!(app.tab.catalog_marks[1].highlight.as_deref(), Some("rust"));
    // H hides by hand; the filter's own can't be unhidden by H.
    app.tab.catalog_list.state.select(Some(1));
    app.act(Action::Hide);
    assert_eq!(app.visible_catalog(), [1]);
    assert!(app.store.hidden_on("a", "x").contains(&3));
    // Z shows them all, keeping the selection on the same thread.
    app.act(Action::ShowHidden);
    assert_eq!(app.visible_catalog(), [0, 1, 2]);
    assert_eq!(app.selected_index(), Some(1));
    app.tab.catalog_list.state.select(Some(0));
    app.act(Action::Hide);
    assert!(app.status.as_ref().unwrap().text.contains("filter \"spam\""));
    app.tab.catalog_list.state.select(Some(2));
    app.act(Action::Hide);
    assert!(!app.store.hidden_on("a", "x").contains(&3));
    app.act(Action::ShowHidden);
    // In a thread, hidden posts collapse (never the OP).
    let mut reply = op(11, "");
    reply.body = vec![Line::raw("buy spam")];
    app.set_thread(vec![op(10, "spam OP"), reply, op(12, "")]);
    app.tab.view = View::Thread;
    let t = app.tab.thread.as_ref().unwrap();
    assert!(!t.is_collapsed(0) && t.is_collapsed(1) && !t.is_collapsed(2));
    app.tab.thread.as_mut().unwrap().selected = 2;
    app.act(Action::Hide);
    assert!(app.tab.thread.as_ref().unwrap().is_collapsed(2));
    app.act(Action::ShowHidden);
    assert!(!app.tab.thread.as_ref().unwrap().is_collapsed(1));
}

#[test]
fn notifies_about_new_posts_and_replies_to_yours() {
    let mut app = local_app();
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    let post = |no, quotes: Vec<u64>| Post { no, quotes, ..Default::default() };
    app.store.toggle_watch(key(1), "One".into(), 2, 5);
    app.store.toggle_watch(key(2), "Two".into(), 1, 20);
    app.store.watched_mut(&key(1)).unwrap().mine.push(5);
    // The first refresh of the session tells nothing.
    app.refreshed(key(1), Ok(vec![post(1, vec![]), post(5, vec![]), post(6, vec![5])]));
    app.flush_notes(Instant::now());
    assert!(app.notified.is_empty());
    assert_eq!(app.store.watched(&key(1)).unwrap().replies, 1);
    // Then: a reply to your post, and another post.
    app.refreshed(key(1), Ok(vec![post(1, vec![]), post(5, vec![]), post(6, vec![5]), post(7, vec![5]), post(8, vec![1])]));
    app.flush_notes(Instant::now());
    assert_eq!(app.notified, ["New reply to your post in /x/ One", "1 new post in /x/ One"]);
    assert_eq!(app.store.watched(&key(1)).unwrap().replies, 2);
    // Several threads at once make one notification.
    app.notified.clear();
    app.refreshed(key(2), Ok(vec![post(20, vec![])]));
    app.refreshed(key(1), Ok(vec![post(1, vec![]), post(9, vec![])]));
    app.refreshed(key(2), Ok(vec![post(20, vec![]), post(21, vec![])]));
    // ...once nothing is still refreshing.
    app.refreshing.insert(key(3));
    app.flush_notes(Instant::now());
    assert!(app.notified.is_empty());
    app.refreshing.clear();
    app.flush_notes(Instant::now());
    assert_eq!(app.notified, ["2 watched threads have new posts"]);
}

#[test]
fn marking_posts_as_yours_watches_the_thread() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }, Post { no: 2, ..Default::default() }]));
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.tab.view = View::Thread;
    app.act(Action::Mine);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    assert_eq!(app.store.watched(&key).unwrap().mine, [2]);
    assert!(app.tab.thread.as_ref().unwrap().mine.contains(&2));
    app.act(Action::Mine);
    assert!(app.store.watched(&key).unwrap().mine.is_empty());
}

#[test]
fn catalogs_mark_new_threads_and_replies() {
    let mut app = local_app();
    app.clock = Clock { fixed: Some(1000), ..Default::default() };
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let op = |no, replies| Post { no, replies: Some(replies), ..Default::default() };
    app.load_catalog();
    app.handle(Msg::Catalog(app.tab.req, Ok(vec![op(1, 3), op(2, 0)])));
    assert!(app.tab.catalog_new.is_empty());
    // Thread 1 is opened with 3 replies.
    app.set_thread(vec![Post { no: 1, ..Default::default() }, Post { no: 5, ..Default::default() }, Post { no: 6, ..Default::default() }, Post { no: 7, ..Default::default() }]);
    app.load_catalog();
    app.handle(Msg::Catalog(app.tab.req, Ok(vec![op(9, 0), op(1, 8), op(2, 1)])));
    assert_eq!(app.tab.catalog_new, [9].into());
    assert_eq!(app.new_replies(&app.tab.catalog[1]), Some(5));
    // Threads never opened don't count replies.
    assert_eq!(app.new_replies(&app.tab.catalog[2]), None);
}

#[test]
fn repeated_post_numbers_keep_the_first() {
    let post = |no, name: &str| Post { no, name: name.into(), ..Default::default() };
    let t = ThreadView::new("x".into(), 1, vec![post(1, "op"), post(2, "a"), post(2, "b"), post(3, "c")]);
    assert_eq!(t.posts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["op", "a", "c"]);
    assert_eq!((t.index[&3], t.backlinks.len(), t.entries.len()), (2, 3, 3));
}

#[test]
fn replies_expand_inline() {
    // 1 <- 2 <- 3, and 3 also quotes 1; 4 quotes 2; 5 and 6 quote each other.
    let post = |no, quotes: Vec<u64>| Post { no, quotes, ..Default::default() };
    let mut t = ThreadView::new("x".into(), 1, vec![post(1, vec![]), post(2, vec![1]), post(3, vec![2, 1]), post(4, vec![2]), post(5, vec![6]), post(6, vec![5])]);
    let shown = |t: &ThreadView| t.entries.iter().map(|e| (t.posts[e.post].no, e.depth)).collect::<Vec<_>>();
    assert_eq!(t.toggle_expanded(), Ok(true));
    assert_eq!(shown(&t)[..4], [(1, 0), (2, 1), (3, 1), (2, 0)]);
    // Expand 2 inside 1: its replies come one level deeper; the cursor moves through them.
    t.select_entry(1);
    assert_eq!((t.selected, t.toggle_expanded()), (1, Ok(true)));
    assert_eq!(shown(&t)[..6], [(1, 0), (2, 1), (3, 2), (4, 2), (3, 1), (2, 0)]);
    t.select_entry(2);
    assert_eq!((t.selected, t.entry()), (2, 2));
    // A post with no replies says so.
    assert_eq!(t.toggle_expanded(), Err("No replies to this post"));
    // Collapsing the top one removes everything under it; the selection stays on it.
    t.select_entry(0);
    assert_eq!(t.toggle_expanded(), Ok(false));
    assert_eq!(shown(&t), [(1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0)]);
    assert_eq!(t.entry(), 0);
    // Quote loops stop: 5 under 6 under 5 isn't shown again.
    t.select(4);
    t.toggle_expanded().unwrap();
    t.select_entry(5);
    t.toggle_expanded().unwrap();
    assert_eq!(shown(&t)[4..], [(5, 0), (6, 1), (6, 0)]);
    // Setting `selected` directly lands on the post's top-level entry.
    t.selected = 3;
    assert_eq!(t.entry(), 3);
}

#[test]
fn expanded_replies_survive_a_refresh() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let post = |no, quotes: Vec<u64>| Post { no, quotes, ..Default::default() };
    app.set_thread(vec![post(1, vec![]), post(2, vec![1])]);
    app.tab.view = View::Thread;
    app.act(Action::Expand);
    app.on_key(KeyEvent::from(KeyCode::Down));
    assert_eq!((app.tab.thread.as_ref().unwrap().entry(), app.tab.thread.as_ref().unwrap().selected), (1, 1));
    app.set_thread(vec![post(1, vec![]), post(2, vec![1]), post(3, vec![1])]);
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!(t.entries.len(), 5);
    assert_eq!((t.entry(), t.entries[t.entry()].depth), (1, 1));
}

#[test]
fn grid_moves_in_two_dimensions() {
    let mut app = test_app();
    app.tab.catalog = (1..=7).map(|no| Post { no, ..Default::default() }).collect();
    app.tab.view = View::Catalog;
    app.default_layout = CatalogLayout::Grid;
    app.grid_cols = 3;
    app.tab.catalog_list.state.select(Some(0));
    let press = |app: &mut App, c| app.on_key(KeyEvent::from(KeyCode::Char(c)));
    let at = |app: &App| app.tab.catalog_list.state.selected().unwrap();
    press(&mut app, 'j');
    assert_eq!(at(&app), 3);
    press(&mut app, 'l');
    press(&mut app, 'l');
    assert_eq!(at(&app), 5);
    // The end of a row stops; down from the last full row goes to the last thread.
    press(&mut app, 'l');
    assert_eq!(at(&app), 5);
    press(&mut app, 'j');
    assert_eq!(at(&app), 6);
    press(&mut app, 'k');
    assert_eq!(at(&app), 3);
    // h in the first column goes back, as in lists.
    press(&mut app, 'h');
    assert_eq!(app.tab.view, View::Boards);
    // Clicks hit the right card.
    app.tab.view = View::Catalog;
    app.hit = Some(Hit::Grid { area: Rect::new(2, 2, 66, 24), offset: 0, cols: 3, cell: (22, 12) });
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2 + 22 + 5, 2 + 12 + 3), Instant::now());
    assert_eq!(at(&app), 4);
    // c cycles this board's layout.
    app.act(Action::Compact);
    assert_eq!(app.layout(), CatalogLayout::Cards);
}

#[test]
fn gallery_of_the_threads_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.download_dir = Some(dir.path().display().to_string());
    app.images = crate::images::Images::offline();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let file = |name: &str| Attachment { filename: name.into(), url: format!("http://127.0.0.1:3/x/src/{name}"), ..Default::default() };
    let post = |no, files: Vec<Attachment>| Post { no, files, ..Default::default() };
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![post(1, vec![file("a.png")]), post(2, vec![]), post(3, vec![file("b.jpg"), file("c.gif")])]));
    app.tab.view = View::Thread;
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.act(Action::Gallery);
    // It starts at the selected post's file, or the next.
    assert_eq!(app.tab.gallery.as_ref().unwrap().state.selected(), Some(1));
    app.tab.gallery.as_mut().unwrap().cols = 2;
    let press = |app: &mut App, code| app.on_key(KeyEvent::from(code));
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(app.tab.gallery.as_ref().unwrap().state.selected(), Some(2));
    // Enter views every file of the thread, from this one; esc comes back to the grid.
    press(&mut app, KeyCode::Enter);
    assert_eq!((app.tab.viewer().unwrap().files.len(), app.tab.viewer().unwrap().index), (3, 2));
    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('Y'));
    assert_eq!(app.copied.as_deref(), Some("http://127.0.0.1:3/x/res/1.html#3"));
    press(&mut app, KeyCode::Esc);
    assert!(app.tab.viewer().is_none());
    assert_eq!(app.tab.gallery.as_ref().unwrap().state.selected(), Some(1));
    // d saves the one file.
    press(&mut app, KeyCode::Char('d'));
    assert_eq!((app.downloads.total, app.downloads.running), (1, 1));
    // Esc: back to the thread, on the file's post.
    press(&mut app, KeyCode::Esc);
    assert!(app.tab.gallery.is_none());
    assert_eq!(app.tab.thread.as_ref().unwrap().selected, 2);
}

#[test]
fn archive_search_and_back() {
    let mut app = app_with(
        "[[site]]\nname = \"chan\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"g\"]\narchive = \"arch\"\n\
         [[site]]\nname = \"arch\"\nkind = \"foolfuuka\"\nurl = \"http://localhost:3\"\nboards = [\"g\"]",
    );
    app.tab.board = Some(Board { uri: "g".into(), title: String::new(), nsfw: None });
    app.tab.view = View::Catalog;
    app.act(Action::ArchiveSearch);
    for c in "borrow".chars() {
        app.on_key(KeyEvent::from(KeyCode::Char(c)));
    }
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!((app.tab.view, app.tab.site), (View::Search, 1));
    let v = crate::backend::fixture("foolfuuka_search.json");
    app.handle(Msg::Search(app.tab.req, 1, crate::backend::foolfuuka::parse_search(&v)));
    assert_eq!(app.tab.search.as_ref().unwrap().hits.len(), 4);
    // Going down to the end asks for the next page.
    let req = app.tab.req;
    for _ in 0..4 {
        app.on_key(KeyEvent::from(KeyCode::Down));
    }
    assert_eq!(app.tab.req, req + 1);
    app.handle(Msg::Search(app.tab.req, 2, Err(anyhow::anyhow!("You're searching too fast."))));
    assert!(app.status.as_ref().is_some_and(|s| s.error && s.text.contains("too fast")));
    // Enter: the thread, on the archive, with the post selected.
    app.tab.search_list.state.select(Some(1));
    app.enter();
    assert_eq!((app.tab.view, app.tab.pending_thread, app.tab.pending_post), (View::Thread, 109912686, Some(109914413)));
    app.back();
    assert_eq!(app.tab.view, View::Search);
    app.back();
    assert_eq!((app.tab.view, app.tab.site, app.tab.search.is_none()), (View::Catalog, 0, true));
    // Sites without an archive say so.
    app.sites[0].cfg.archive = None;
    app.act(Action::ArchiveSearch);
    assert!(app.search_input.is_none() && app.status.as_ref().unwrap().text.contains("no archive"));
}

#[test]
fn reverse_image_search() {
    let mut app = local_app();
    app.images = crate::images::Images::offline();
    let file = |name: &str, thumb| Attachment { filename: name.into(), url: format!("https://i.example/{name}"), thumb, ..Default::default() };
    let files = vec![file("a.png", None), file("b.webm", Some("https://i.example/bs.jpg".into())), file("c.pdf", None)];
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, files, ..Default::default() }]));
    app.tab.view = View::Thread;
    app.act(Action::ImageSearch);
    // The image itself, and the video's thumbnail; a file with neither is left out.
    let rows = &app.image_search_panel().unwrap().rows;
    assert_eq!(rows.len(), 2 + 2 * 4);
    assert_eq!(rows[0], Err("a.png".into()));
    assert_eq!(rows[6], Ok(("https://i.example/bs.jpg".into(), 0)));
    app.on_key(KeyEvent::from(KeyCode::Down));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.opened.as_deref(), Some("https://lens.google.com/uploadbyurl?url=https%3A%2F%2Fi.example%2Fa.png"));
    assert!(app.image_search_panel().is_none());
    // In the viewer: the file shown.
    app.act(Action::View);
    app.on_key(KeyEvent::from(KeyCode::Char('R')));
    assert_eq!(app.image_search_panel().unwrap().rows.len(), 4);
    app.on_key(KeyEvent::from(KeyCode::Char('y')));
    assert_eq!(app.copied.as_deref(), Some("https://saucenao.com/search.php?url=https%3A%2F%2Fi.example%2Fa.png"));
    // Engines can be configured.
    let app = app_with("[[image_search]]\nname = \"Mine\"\nurl = \"https://s.example/?u={url}\"\n[[site]]\nname = \"a\"\nkind = \"4chan\"");
    assert_eq!(app.image_search.len(), 1);
    assert_eq!(app.image_search[0].link("http://x/y z"), "https://s.example/?u=http%3A%2F%2Fx%2Fy%20z");
}

#[test]
fn sessions_save_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.store = Store::load(Some(dir.path().to_path_buf())).0;
    // A thread on the second site, with a post selected.
    app.goto_str("b/y/5");
    app.set_thread(vec![Post { no: 5, ..Default::default() }, Post { no: 6, ..Default::default() }]);
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.tab.catalog_sort = Sort::Newest;
    app.save_session(None);
    let saved = app.store.load_session().unwrap();
    assert_eq!(saved.tabs[0], crate::store::Place {
        view: "thread".into(),
        site: "b".into(),
        board: Some("y".into()),
        thread: Some(5),
        selected: Some(6),
        sort: Some(Sort::Newest),
        filter: String::new(),
        conversation: None,
    });
    // The next run starts there.
    let mut next = local_app();
    next.store = Store::load(Some(dir.path().to_path_buf())).0;
    next.restore_session();
    assert_eq!((next.tab.site, next.tab.view, next.tab.pending_thread, next.tab.pending_post, next.tab.catalog_sort), (1, View::Thread, 5, Some(6), Sort::Newest));
    // If the thread is gone, its catalog instead.
    next.handle(Msg::Thread(next.tab.req, Err(anyhow::Error::new(http::HttpError::NotFound("x".into())))));
    assert_eq!(next.tab.view, View::Catalog);
    // A catalog with its selected thread.
    next.handle(Msg::Catalog(next.tab.req, Ok(vec![])));
    next.tab.catalog = (1..4).map(|no| Post { no, ..Default::default() }).collect();
    next.tab.catalog_list.state.select(Some(2));
    // (The board has no sort of its own, so its catalog is in bump order: index 2 is thread 3.)
    let place = next.place();
    assert_eq!((place.view.as_str(), place.selected), ("catalog", Some(3)));
    let mut third = local_app();
    third.go_to_place(&place);
    third.handle(Msg::Catalog(third.tab.req, Ok((1..4).map(|no| Post { no, time: no as i64, ..Default::default() }).collect())));
    assert_eq!(third.selected_index().map(|i| third.tab.catalog[i].no), Some(3));
}

#[test]
fn tabs_keep_their_own_place_and_responses() {
    let mut app = local_app();
    // Tab 0 loads a catalog on site a.
    app.goto_str("a/x");
    let first_req = app.tab.req;
    // Tab 1 opens a thread on site b while that's still loading.
    app.tabs.push(Tab::new(0, Instant::now()));
    app.switch_tab(1);
    assert_eq!((app.tab.view, app.tab.thread.is_none()), (View::Sites, true));
    app.goto_str("b/y/5");
    assert_eq!((app.tab.site, app.tab.view, app.tab.pending_thread), (1, View::Thread, 5));
    // Tab 0's catalog arrives: it goes to tab 0, not here.
    app.handle(Msg::Catalog(first_req, Ok(vec![Post { no: 1, ..Default::default() }])));
    assert!(app.tab.catalog.is_empty());
    app.handle(Msg::Thread(app.tab.req, Ok(vec![Post { no: 5, ..Default::default() }])));
    assert_eq!(app.tab.thread.as_ref().unwrap().no, 5);
    app.switch_tab(0);
    assert_eq!((app.tab.site, app.tab.view, app.tab.catalog.len(), app.tab.loading.is_none()), (0, View::Catalog, 1, true));
    assert!(app.tab.thread.is_none());
    // A thread with no posts at all is an error, and what was shown stays.
    app.handle(Msg::Catalog(app.tab.req, Ok(vec![])));
    app.goto_str("a/x/1");
    app.handle(Msg::Thread(app.tab.req, Ok(vec![Post { no: 1, ..Default::default() }])));
    app.act(Action::Reload);
    app.handle(Msg::Thread(app.tab.req, Ok(vec![])));
    assert!(app.tab.thread.as_ref().is_some_and(|t| t.posts.len() == 1) && app.status.as_ref().is_some_and(|s| s.error));
    // A post's thread found while the settings are open opens behind them.
    app.goto_str("a/x/1#77");
    app.act(Action::Settings);
    app.handle(Msg::Found(app.tab.req, Board { uri: "x".into(), title: String::new(), nsfw: None }, 77, Ok(Some(3))));
    assert_eq!((app.tab.view, app.tab.settings_back, app.tab.pending_thread), (View::Settings, Some(View::Thread), 3));
    // Tab chips don't switch tabs under a settings popup (it isn't the tab's).
    app.tabs.push(Tab::new(0, Instant::now()));
    app.tab_chips = vec![(Rect::new(0, 0, 5, 1), 1)];
    app.popup = Some(Popup::Settings(SettingsPopup::Folder { value: String::new() }));
    app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 1, row: 0, modifiers: KeyModifiers::NONE }, Instant::now());
    assert_eq!((app.active, app.tab.view), (0, View::Settings));
    app.popup = None;
    app.tabs.pop();
    app.tab.view = View::Catalog;
    // A response for a tab that's gone is dropped.
    let stale = app.tabs[1].req;
    app.tabs.truncate(1);
    app.handle(Msg::Thread(stale, Ok(vec![Post { no: 9, ..Default::default() }])));
    assert!(app.tab.thread.is_none());
}

#[test]
fn new_tabs_switching_closing_and_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.store = Store::load(Some(dir.path().to_path_buf())).0;
    app.goto_str("a/x");
    app.handle(Msg::Catalog(app.tab.req, Ok((1..=3).map(|no| Post { no, ..Default::default() }).collect())));
    app.tab.catalog_list.state.select(Some(1));
    let key = |c| KeyEvent::from(KeyCode::Char(c));
    // T: thread 2 in a new tab after this one.
    app.on_key(key('T'));
    assert_eq!((app.tabs.len(), app.active, app.tab.view, app.tab.pending_thread), (2, 1, View::Thread, 2));
    assert_eq!(app.tab_label(0), "/x/");
    // ] / [ switch; each tab keeps its place.
    app.on_key(key(']'));
    assert_eq!((app.active, app.tab.view, app.tab.catalog.len()), (0, View::Catalog, 3));
    app.on_key(key('['));
    assert_eq!((app.active, app.tab.view), (1, View::Thread));
    // The session has both.
    app.save_session(None);
    let s = app.store.load_session().unwrap();
    assert_eq!((s.tabs.len(), s.active, s.tabs[1].thread), (2, 1, Some(2)));
    // ctrl-w closes; the last tab stays.
    app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    assert_eq!((app.tabs.len(), app.active, app.tab.view), (1, 0, View::Catalog));
    app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    assert_eq!(app.tabs.len(), 1);
    // At most MAX_TABS.
    for _ in 0..MAX_TABS + 2 {
        app.switch_tab(0);
        app.new_tab();
    }
    assert_eq!(app.tabs.len(), MAX_TABS);
    // A new run restores the tabs.
    let mut next = local_app();
    next.store = Store::load(Some(dir.path().to_path_buf())).0;
    next.restore_session();
    assert_eq!((next.tabs.len(), next.active, next.tab.view, next.tab.pending_thread), (2, 1, View::Thread, 2));
    next.switch_tab(0);
    assert_eq!((next.tab.view, next.tab.board.as_ref().unwrap().uri.as_str()), (View::Catalog, "x"));
}

#[test]
fn favorite_boards_on_the_home_screen() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.config_path = Some(dir.path().join("config.toml"));
    // * in Boards on the selected board, and in a catalog on its board.
    app.switch_site(1);
    app.tab.view = View::Boards;
    app.tab.board_list.state.select(Some(0));
    app.act(Action::Favorite);
    app.goto_str("a/xy");
    app.act(Action::Favorite);
    assert_eq!(app.favorites.iter().map(BoardRef::key).collect::<Vec<_>>(), ["b/y", "a/xy"]);
    let c: Config = toml::from_str(&std::fs::read_to_string(dir.path().join("config.toml")).unwrap()).unwrap();
    assert_eq!(c.favorites, ["b/y", "a/xy"]);
    // They're on the home screen after Watched, History and Saved; 2 opens the second.
    app.tab.view = View::Sites;
    assert_eq!(app.visible_sites()[3..5], [SiteRow::Favorite(0), SiteRow::Favorite(1)]);
    app.on_key(KeyEvent::from(KeyCode::Char('1')));
    assert_eq!((app.tab.site, app.tab.view, app.tab.board.as_ref().unwrap().uri.as_str()), (1, View::Catalog, "y"));
    app.tab.view = View::Sites;
    app.on_key(KeyEvent::from(KeyCode::Char('2')));
    assert_eq!((app.tab.site, app.tab.board.as_ref().unwrap().uri.as_str()), (0, "xy"));
    // x on a favorite row takes it off; * again on the board does too.
    app.tab.view = View::Sites;
    app.site_list.state.select(Some(3));
    app.act(Action::Remove);
    assert_eq!(app.favorites.len(), 1);
    app.goto_str("a/xy");
    app.act(Action::Favorite);
    assert!(app.favorites.is_empty());
    app.tab.view = View::Sites;
    app.on_key(KeyEvent::from(KeyCode::Char('3')));
    assert!(app.status.as_ref().unwrap().text.contains("No favorites yet"));
}

#[test]
fn recent_boards_on_the_home_screen() {
    let mut app = local_app();
    for board in ["a/x", "b/y", "a/xy"] {
        app.goto_str(board);
        app.handle(Msg::Catalog(app.tab.req, Ok(vec![])));
    }
    assert_eq!(app.store.recent_boards, ["a/xy", "b/y", "a/x"]);
    // Favorites aren't repeated as recent.
    app.favorites.push(BoardRef::parse("b/y").unwrap());
    app.tab.view = View::Sites;
    let rows = app.visible_sites();
    assert_eq!(rows[3..6], [SiteRow::Favorite(0), SiteRow::Recent(0), SiteRow::Recent(2)]);
    // Enter opens; x forgets it.
    app.site_list.state.select(Some(5));
    app.enter();
    assert_eq!((app.tab.view, app.tab.board.as_ref().unwrap().uri.as_str()), (View::Catalog, "x"));
    app.tab.view = View::Sites;
    app.site_list.state.select(Some(4));
    app.act(Action::Remove);
    assert_eq!(app.store.recent_boards, ["b/y", "a/x"]);
}

#[test]
fn hiding_sites_from_the_home_screen() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.config_path = Some(dir.path().join("config.toml"));
    let sites = |app: &App| app.visible_sites().into_iter().filter(|r| matches!(r, SiteRow::Site(_) | SiteRow::HiddenSites)).collect::<Vec<_>>();
    assert_eq!(sites(&app), [SiteRow::Site(0), SiteRow::Site(1)]);
    app.site_list.state.select(Some(3));
    app.act(Action::Remove);
    assert_eq!(sites(&app), [SiteRow::Site(1), SiteRow::HiddenSites]);
    let c: Config = toml::from_str(&std::fs::read_to_string(dir.path().join("config.toml")).unwrap()).unwrap();
    assert_eq!(c.hidden_sites, ["a"]);
    // The last row shows them; x on one brings it back.
    app.site_list.state.select(Some(4));
    app.enter();
    assert_eq!(sites(&app), [SiteRow::Site(0), SiteRow::Site(1), SiteRow::HiddenSites]);
    app.site_list.state.select(Some(3));
    app.act(Action::Remove);
    assert_eq!(sites(&app), [SiteRow::Site(0), SiteRow::Site(1)]);
    assert!(app.hidden_sites.is_empty());
}

#[test]
fn boards_remember_their_sort_and_layout() {
    let mut app = local_app();
    app.goto_str("a/x");
    app.act(Action::Sort);
    app.act(Action::Compact);
    assert_eq!((app.tab.catalog_sort, app.layout()), (Sort::Replies, CatalogLayout::Compact));
    // Another board: the defaults.
    app.goto_str("a/xy");
    assert_eq!((app.tab.catalog_sort, app.layout()), (Sort::Bump, CatalogLayout::Cards));
    // Back on the first: its own again (also after a restart, from the data directory).
    app.goto_str("a/x");
    assert_eq!((app.tab.catalog_sort, app.layout()), (Sort::Replies, CatalogLayout::Compact));
    assert_eq!(app.store.board_prefs["a/x"], crate::store::BoardPrefs { sort: Some(Sort::Replies), layout: Some(CatalogLayout::Compact), images: None });
    // The default (Settings) applies to boards without their own.
    app.default_layout = CatalogLayout::Grid;
    app.goto_str("a/xy");
    assert_eq!(app.layout(), CatalogLayout::Grid);
}

#[test]
fn following_a_general() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let op = |no, subject: &str| Post { no, subject: Some(subject.into()), replies: Some(10), ..Default::default() };
    app.set_thread(vec![op(10, "/lmg/ - Local Models General #5"), Post { no: 11, ..Default::default() }]);
    app.tab.view = View::Thread;
    // F follows it (watching it too).
    app.act(Action::Follow);
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    assert_eq!(app.store.watched(&key(10)).unwrap().general.as_deref(), Some("/lmg/"));
    // Alive and not full: nothing to look for.
    let now = Instant::now();
    app.check_generals(now);
    assert!(app.generals_searching.is_empty());
    // At the bump limit (a refresh says so): its board is searched, once.
    let mut full = op(10, "/lmg/ - Local Models General #5");
    full.bumplimit = true;
    app.tab.view = View::Sites;
    app.refreshed(key(10), Ok(vec![full, Post { no: 11, ..Default::default() }]));
    assert!(app.store.watched(&key(10)).unwrap().at_limit);
    app.check_generals(now);
    app.check_generals(now);
    assert_eq!(app.generals_searching.len(), 1);
    // No new thread yet: tried again only after a while.
    app.general_catalog(key(10), Ok(vec![op(10, "/lmg/ - Local Models General #5"), op(12, "/ldg/ - Local Diffusion")]));
    app.check_generals(now + Duration::from_secs(60));
    assert!(app.generals_searching.is_empty());
    // The next one appears: it's watched and followed; the old one (still going) is kept.
    app.check_generals(now + Duration::from_secs(601));
    app.general_catalog(key(10), Ok(vec![op(9, "/lmg/ old"), op(13, "/lmg/ - Local Models General #6"), op(14, "/ldg/")]));
    assert_eq!(app.store.watched(&key(13)).unwrap().general.as_deref(), Some("/lmg/"));
    assert_eq!(app.store.watched(&key(10)).unwrap().general, None);
    assert!(app.notified.last().unwrap().starts_with("New /lmg/ thread on /x/"));
    // When the followed thread dies, it's replaced in Watched.
    app.store.watched_mut(&key(13)).unwrap().dead = true;
    app.check_generals(now + Duration::from_secs(1200));
    app.general_catalog(key(13), Ok(vec![op(20, "/lmg/ - Local Models General #7")]));
    assert!(app.store.watched(&key(13)).is_none());
    assert!(app.store.watched(&key(20)).is_some());
    // F again stops following.
    app.tab.view = View::Watched;
    let i = app.store.watched.iter().position(|w| w.key == key(20)).unwrap();
    app.watched_list.state.select(Some(i));
    app.act(Action::Follow);
    assert_eq!(app.store.watched(&key(20)).unwrap().general, None);
}

#[test]
fn background_changes_are_saved_together() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.store = Store::load(Some(dir.path().to_path_buf())).0;
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.clock = Clock { fixed: Some(1000), ..Default::default() };
    let posts = |n: u64| (1..=n).map(|no| Post { no, ..Default::default() }).collect::<Vec<_>>();
    app.set_thread(posts(2));
    // Opening a thread is a visit, written a little later rather than at once.
    assert!(!dir.path().join("history.json").exists());
    app.saved_at -= SAVE_EVERY;
    app.poll();
    assert!(dir.path().join("history.json").exists());
    // A refresh without new posts isn't a new visit; one with new posts is.
    app.clock = Clock { fixed: Some(2000), ..Default::default() };
    app.set_thread(posts(2));
    assert_eq!(app.store.history[0].opened, 1000);
    app.set_thread(posts(3));
    assert_eq!((app.store.history[0].opened, app.store.history[0].last_seen), (2000, 3));
    // What the user does is saved at once.
    app.tab.view = View::Thread;
    app.act(Action::Watch);
    let watched = std::fs::read_to_string(dir.path().join("watched.json")).unwrap();
    assert!(watched.contains("\"no\": 1"), "{watched}");
}

#[test]
fn tab_switches_keep_layouts_and_theme_changes_redo_them_all() {
    let mut app = local_app();
    let layout = || Some(ThreadLayout::of_blocks(40, Vec::new()));
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }]));
    app.tab.thread.as_mut().unwrap().layout = layout();
    app.tabs.push(Tab::new(0, Instant::now()));
    app.switch_tab(1);
    app.tab.thread = Some(ThreadView::new("x".into(), 2, vec![Post { no: 2, ..Default::default() }]));
    app.tab.thread.as_mut().unwrap().layout = layout();
    // Switching (and handling another tab's response) doesn't throw layouts away.
    app.switch_tab(0);
    app.switch_tab(1);
    assert!(app.tab.thread.as_ref().unwrap().layout.is_some());
    // A theme change does, in every tab.
    app.set_theme(crate::theme::theme());
    assert!(app.tab.thread.as_ref().unwrap().layout.is_none());
    app.switch_tab(0);
    assert!(app.tab.thread.as_ref().unwrap().layout.is_none());
}

#[test]
fn status_messages_expire() {
    let mut app = test_app();
    let t0 = Instant::now();
    app.info("No unread posts");
    app.expire_status(t0);
    app.expire_status(t0 + Duration::from_millis(1500));
    assert!(app.status.is_some());
    app.expire_status(t0 + Duration::from_secs(2));
    assert!(app.status.is_none());

    // Errors stay longer, and a new message restarts the timer.
    app.error("Rate limited");
    app.expire_status(t0);
    app.expire_status(t0 + Duration::from_secs(4));
    assert!(app.status.is_some());
    app.error("Thread was deleted or archived");
    app.expire_status(t0 + Duration::from_secs(4));
    app.expire_status(t0 + Duration::from_secs(8));
    assert!(app.status.is_some());
    app.expire_status(t0 + Duration::from_secs(9));
    assert!(app.status.is_none());
}

#[test]
fn filtering_a_big_catalog_is_fast() {
    let mut app = test_app();
    let text = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(20);
    app.tab.catalog = (0..300)
        .map(|i| Post { no: i, subject: Some(format!("thread {i}")), body: vec![Line::raw(text.clone())], ..Default::default() })
        .collect();
    app.tab.catalog_list.filter = "thread 29".into();
    assert_eq!(app.visible_catalog().len(), 11); // 29, 290..299
    app.tab.catalog_sort = Sort::Replies;
    let start = Instant::now();
    for _ in 0..100 {
        std::hint::black_box(app.visible_catalog());
    }
    let per_call = start.elapsed() / 100;
    eprintln!("filter + sort of 300 threads: {per_call:?}");
    // A frame is ~16ms; even unoptimized (and on a busy machine), filtering should take a
    // fraction of it.
    assert!(per_call < Duration::from_millis(25), "filtering took {per_call:?}");
}

#[test]
fn tab_focuses_parts_and_the_verbs_follow_it() {
    use crate::model::Target;
    let mut app = local_app();
    app.images = Images::offline();
    app.goto_str("a/x/1");
    let html = r##"<a href="#p1" class="quotelink">&gt;&gt;1</a> see https://example.com/a"##;
    let file = |name: &str| Attachment { filename: name.into(), url: format!("http://127.0.0.1:3/x/src/{name}"), ..Default::default() };
    let reply = Post { no: 2, files: vec![file("a.png"), file("b.webm")], ..crate::markup::parse_html(html, crate::markup::Flavor::Vichan).into() };
    app.handle(Msg::Thread(app.tab.req, Ok(vec![Post { no: 1, ..Default::default() }, reply, Post { no: 3, ..Default::default() }])));
    let tab = |app: &mut App, shift: bool| app.on_key(if shift { KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT) } else { KeyEvent::from(KeyCode::Tab) });
    let focus = |app: &App| app.tab.thread.as_ref().unwrap().focus.clone();
    let selected = |app: &App| app.tab.thread.as_ref().unwrap().selected;
    // The OP has a reply (2 quotes it): its Replies label, then the reply's number.
    tab(&mut app, false);
    assert_eq!((selected(&app), focus(&app)), (0, Some(Part::Replies)));
    tab(&mut app, false);
    tab(&mut app, false);
    // On into post 2: its files, then its quote and its URL.
    assert_eq!((selected(&app), focus(&app)), (1, Some(Part::File(0))));
    // v and d act on the focused file.
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    assert_eq!(app.downloads.total, 1);
    tab(&mut app, false);
    tab(&mut app, false);
    assert!(matches!(focus(&app), Some(Part::Link(Target::Quote(_)))));
    tab(&mut app, false);
    assert_eq!(focus(&app), Some(Part::Link(Target::Url("https://example.com/a".into()))));
    // y and o take the URL; enter opens it.
    app.on_key(KeyEvent::from(KeyCode::Char('y')));
    assert_eq!(app.copied.as_deref(), Some("https://example.com/a"));
    app.opened = None;
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.opened.as_deref(), Some("https://example.com/a"));
    // shift-tab goes back; esc goes back to the post itself.
    tab(&mut app, true);
    assert!(matches!(focus(&app), Some(Part::Link(Target::Quote(_)))));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!((selected(&app), focus(&app), app.tab.view), (1, None, View::Thread));
    // enter on the focused quote jumps to the post (and u comes back).
    tab(&mut app, false);
    tab(&mut app, false);
    tab(&mut app, false);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!((selected(&app), focus(&app)), (0, None));
    app.on_key(KeyEvent::from(KeyCode::Char('u')));
    assert_eq!(selected(&app), 1);
    // A file in the viewer, at that file.
    tab(&mut app, false);
    tab(&mut app, false);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.tab.viewer().map(|v| v.index), Some(1));
    app.tab.popup = None;
    // Past the last part with any: a message, the focus stays.
    for _ in 0..10 {
        tab(&mut app, false);
    }
    assert_eq!(app.status.as_ref().map(|s| s.text.as_str()), Some("No more images or links below"));
}

#[test]
fn the_menu_runs_what_it_lists() {
    let mut app = local_app();
    app.goto_str("a/x/1");
    app.handle(Msg::Thread(app.tab.req, Ok(vec![Post { no: 1, files: vec![Attachment { filename: "a.png".into(), url: "http://127.0.0.1:3/a.png".into(), ..Default::default() }], ..Default::default() }])));
    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    let m = app.menu().unwrap();
    let has = |a: Action| m.items.iter().any(|i| matches!(i, MenuItem::Act(x, _) if *x == a));
    assert!(has(Action::View) && has(Action::Watch) && has(Action::Gallery) && !has(Action::Preview));
    // A row's own key runs it, and the menu closes.
    app.on_key(KeyEvent::from(KeyCode::Char('w')));
    assert!(app.menu().is_none() && app.status.as_ref().is_some_and(|s| s.text.starts_with("Watching")));
    // So does enter on a row.
    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    let at = app.menu().unwrap().items.iter().position(|i| matches!(i, MenuItem::Act(Action::Watch, _))).unwrap();
    app.menu_mut().unwrap().list.select(Some(at));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.menu().is_none() && app.status.as_ref().is_some_and(|s| s.text.starts_with("Stopped watching")));
    // Esc just closes it.
    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.menu().is_none());
}

// ----- saved threads -----

fn gone() -> anyhow::Error {
    anyhow::Error::new(http::HttpError::NotFound("x".into()))
}

#[test]
fn watched_threads_are_saved_as_posts_arrive() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    let file = |no: u64| dir.path().join(format!("threads/a/x/{no}.json"));
    // Not watched: not saved.
    app.goto_str("a/x/1");
    app.handle(Msg::Thread(app.tab.req, Ok(nos(&[1, 2]))));
    assert!(!file(1).exists());
    // Watching a loaded thread saves it at once; refreshes save the changes only.
    app.act(Action::Watch);
    app.flush_writes();
    assert!(file(1).exists());
    std::fs::remove_file(file(1)).unwrap();
    app.set_thread(nos(&[1, 2]));
    app.flush_writes();
    assert!(!file(1).exists());
    app.set_thread(nos(&[1, 2, 3]));
    assert_eq!(app.store.saved(&key(1)).unwrap().posts, 3);
    // A watched thread refreshed in the background, too.
    app.store.toggle_watch(key(7), "seven".into(), 1, 7);
    app.refreshed(key(7), Ok(nos(&[7, 8])));
    app.flush_writes();
    assert!(file(7).exists());
    // The index is written with the rest of the data.
    app.save_now();
    assert_eq!(Store::load(Some(dir.path().to_path_buf())).0.saved.len(), 2);
}

#[test]
fn a_dead_thread_offers_its_saved_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 10_000);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    // A watched thread opens from its saved copy at once, and when it's gone, that's it.
    app.store.toggle_watch(key.clone(), "one".into(), 2, 2);
    app.store.keep_thread(&key, "one", "u", &nos(&[1, 2]), 10_000 - 7200);
    app.goto_str("a/x/1");
    assert_eq!(app.tab.cached, Some(tabs::Offline { saved: 10_000 - 7200, dead: false }));
    assert!(app.tab.loading.is_some() && app.tab.thread.as_ref().unwrap().posts.len() == 2);
    app.handle(Msg::Thread(app.tab.req, Err(gone())));
    assert!(app.store.watched(&key).unwrap().dead && app.store.saved(&key).unwrap().dead);
    assert_eq!((app.tab.cached, app.tab.offline.map(|o| o.dead)), (None, Some(true)));
    // An exported copy of a thread that isn't watched: offered when the thread is gone.
    app.store.toggle_watch(key.clone(), String::new(), 0, 0);
    app.goto_str("a/x");
    app.goto_str("a/x/1");
    assert!(app.tab.thread.is_none());
    app.handle(Msg::Thread(app.tab.req, Err(gone())));
    let text = &app.status.as_ref().unwrap().text;
    assert!(text.contains("A saved copy from 2h ago: enter opens it"), "{text}");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.tab.offline, Some(tabs::Offline { saved: 10_000 - 7200, dead: true }));
    assert_eq!(app.tab.thread.as_ref().unwrap().posts.len(), 2);
    // Read offline: r says so, and nothing is fetched, however long it stays open.
    app.act(Action::Reload);
    assert!(app.status.as_ref().unwrap().text.contains("saved copy from 2h ago; the thread is gone"));
    for _ in 0..3 {
        app.clock = Clock { fixed: Some(app.clock.now() + 600), instant: Some(app.clock.instant() + Duration::from_secs(600)) };
        app.poll();
        assert!(app.tab.loading.is_none() && app.refreshing.is_empty() && app.tab.req == 0);
    }
    assert!(app.next_wake(app.clock.instant()) > Duration::from_millis(500));
    // Opening it isn't a visit, nor a save (which would bring the dead copy back to life).
    assert!(app.store.saved(&key).unwrap().dead);
}

#[test]
fn a_thread_dying_on_screen_becomes_its_saved_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.goto_str("a/x/1");
    app.handle(Msg::Thread(app.tab.req, Ok(nos(&[1, 2]))));
    app.act(Action::Watch);
    app.refreshed(key.clone(), Err(gone()));
    assert_eq!(app.tab.offline, Some(tabs::Offline { saved: 1000, dead: true }));
    assert!(app.status.as_ref().unwrap().text.contains("this is its saved copy"));
    assert_eq!(app.tab.thread.as_ref().unwrap().posts.len(), 2);
    // Without a copy, as before.
    let mut app = saving_app(dir.path(), 1000);
    app.goto_str("a/x/5");
    app.handle(Msg::Thread(app.tab.req, Err(gone())));
    assert!(app.tab.saved_offer.is_none() && app.tab.offline.is_none());
    assert_eq!(app.status.as_ref().unwrap().text, "Thread was deleted or archived");
}

#[test]
fn a_saved_copy_of_a_live_thread_goes_live_with_r() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.store.keep_thread(&key, "one", "u", &nos(&[1, 2]), 900);
    app.tab.view = View::Saved;
    app.saved_list.state.select(Some(0));
    app.enter();
    assert_eq!(app.tab.offline, Some(tabs::Offline { saved: 900, dead: false }));
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.act(Action::Reload);
    assert!(app.tab.offline.is_none() && app.tab.loading.is_some());
    // The copy stays up, and the live thread arrives in its place, keeping the selection.
    app.handle(Msg::Thread(app.tab.req, Ok(nos(&[1, 2, 3]))));
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!((t.posts.len(), t.current().unwrap().no), (3, 2));
    // Back goes to the Saved view.
    app.back();
    assert_eq!(app.tab.view, View::Saved);
}

#[test]
fn the_saved_view_lists_and_removes_after_asking() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    for (no, at) in [(1, 100), (2, 300), (3, 200)] {
        app.store.keep_thread(&key(no), &format!("thread {no}"), "u", &nos(&[no]), at);
    }
    app.tab.view = View::Sites;
    app.site_list.state.select(Some(2));
    assert_eq!(app.selected_site_row(), Some(SiteRow::Saved));
    app.enter();
    assert_eq!(app.tab.view, View::Saved);
    // Newest saved first.
    let rows: Vec<u64> = app.visible_saved().iter().map(|&i| app.store.saved[i].key.no).collect();
    assert_eq!(rows, [2, 3, 1]);
    app.saved_list.filter = "thread 3".into();
    assert_eq!(app.visible_saved().len(), 1);
    app.saved_list.filter.clear();
    // x asks first; a second x removes it, file and all.
    app.saved_list.state.select(Some(1));
    app.act(Action::Remove);
    assert_eq!(app.store.saved.len(), 3);
    assert!(app.status.as_ref().unwrap().text.contains("again to remove the saved copy of thread 3"));
    app.act(Action::Remove);
    assert!(app.store.saved(&key(3)).is_none() && !dir.path().join("threads/a/x/3.json").exists());
    // Unwatching keeps a copy.
    app.store.toggle_watch(key(1), String::new(), 1, 1);
    app.store.toggle_watch(key(1), String::new(), 1, 1);
    assert!(app.store.saved(&key(1)).is_some());
    // From : too.
    app.tab.view = View::Sites;
    app.goto_str("saved");
    assert_eq!(app.tab.view, View::Saved);
}

#[test]
fn export_saves_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(&dir.path().join("data"), 1000);
    app.download_dir = Some(dir.path().join("dl").display().to_string());
    app.goto_str("a/x/1");
    app.handle(Msg::Thread(app.tab.req, Ok(nos(&[1, 2]))));
    // It asks first; enter saves.
    app.act(Action::Export);
    assert!(!dir.path().join("dl/thread.json").exists());
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(dir.path().join("dl/thread.json").exists());
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    assert_eq!(app.store.saved(&key).unwrap().posts, 2);
    assert!(app.status.as_ref().unwrap().text.contains("(and in Saved)"));
}

#[test]
fn a_saved_copy_is_remembered_in_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = ThreadKey { site: "b".into(), board: "y".into(), no: 5 };
    app.store.keep_thread(&key, "five", "u", &nos(&[5, 6]), 900);
    app.open_saved(key);
    app.tab.thread.as_mut().unwrap().selected = 1;
    let place = app.place();
    assert_eq!((place.view.as_str(), place.thread, place.selected), ("saved", Some(5), Some(6)));
    let mut next = saving_app(dir.path(), 1000);
    next.go_to_place(&place);
    assert!(next.tab.offline.is_some() && next.tab.loading.is_none());
    assert_eq!(next.tab.thread.as_ref().unwrap().current().unwrap().no, 6);
    // The Saved view itself.
    next.tab.view = View::Saved;
    assert_eq!(next.place().view, "saved");
}

#[test]
fn watching_a_saved_copy_keeps_it_under_its_own_number() {
    // (Found by fuzzing.) The site answered thread 5 with thread 9's posts; its copy, kept as
    // 5's, shows thread 9. Watching that keeps a copy as 9's too, so 9 dying loses nothing.
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    app.store.keep_thread(&key(5), "five", "u", &nos(&[9, 10]), 900);
    app.open_saved(key(5));
    assert_eq!(app.tab.thread.as_ref().unwrap().no, 9);
    app.act(Action::Watch);
    assert!(app.store.watched(&key(9)).is_some_and(|w| w.last_seen > 0));
    assert_eq!(app.store.saved(&key(9)).unwrap().posts, 2);
}

// ----- filters from where you are -----

const FILTER_CONFIG: &str = "# my config\n[[site]]\nname = \"a\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"x\"]\n\n\
    # spam goes\n[[filter]]\npattern = \"(?i)spam\" # case-insensitive\nlabel = \"spam\"\n";

/// The local app over a config file with a commented filter, on a thread of named posts.
fn filter_app(dir: &std::path::Path) -> App {
    let path = dir.join("config.toml");
    std::fs::write(&path, FILTER_CONFIG).unwrap();
    let mut app = app_with(FILTER_CONFIG);
    app.config_path = Some(path);
    app.goto_str("a/x/1");
    let named = |no, name: &str| Post { no, name: name.into(), ..Default::default() };
    app.handle(Msg::Thread(app.tab.req, Ok(vec![named(1, "Anonymous"), named(2, "Named !Trip"), named(3, "Anonymous"), named(4, "Named !Trip")])));
    app
}

fn config_text(app: &App) -> String {
    std::fs::read_to_string(app.config_path.as_ref().unwrap()).unwrap()
}

#[test]
fn x_filters_posts_like_the_selected_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = filter_app(dir.path());
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.act(Action::Filter);
    let a = app.filter_add().unwrap();
    assert_eq!(a.candidates[0].what, "posts by Named !Trip");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    // Written to the config, the rest kept as it was.
    let text = config_text(&app);
    assert!(text.starts_with(FILTER_CONFIG), "{text}");
    let added = "[[filter]]\npattern = \"^Named !Trip$\"\nfield = \"name\"\naction = \"hide\"\nlabel = \"Named !Trip\"\nsites = [\"a\"]\nboards = [\"x\"]\n";
    assert!(text.ends_with(added), "{text}");
    // Applied at once: both posts by the name collapse.
    let t = app.tab.thread.as_ref().unwrap();
    assert!(t.marks[1].hidden.is_some() && t.marks[3].hidden.is_some() && t.marks[2].hidden.is_none());
    assert!(app.status.as_ref().unwrap().text.contains("Hiding Named !Trip (2 here) · u undoes"));
    // u takes it back, from the file too.
    app.on_key(KeyEvent::from(KeyCode::Char('u')));
    assert_eq!(config_text(&app), FILTER_CONFIG);
    assert!(app.tab.thread.as_ref().unwrap().marks[1].hidden.is_none() && app.filter_cfgs.len() == 1);
    // Highlight, everywhere, with a label of its own; u isn't undo after another key.
    app.act(Action::Filter);
    for k in ['a', 's', 's', 'e'] {
        app.on_key(KeyEvent::from(KeyCode::Char(k)));
    }
    for _ in 0.."Named !Trip".len() {
        app.on_key(KeyEvent::from(KeyCode::Backspace));
    }
    // Pasted into the label.
    app.paste("him");
    assert!(app.goto.is_none());
    app.on_key(KeyEvent::from(KeyCode::Enter));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let f = app.filter_cfgs.last().unwrap();
    assert_eq!((f.action, f.sites.len(), f.boards.len(), f.label.as_deref()), (crate::filter::FilterAction::Highlight, 0, 0, Some("him")));
    assert_eq!(app.tab.thread.as_ref().unwrap().marks[3].highlight.as_deref(), Some("him"));
    app.on_key(KeyEvent::from(KeyCode::Char('j')));
    app.on_key(KeyEvent::from(KeyCode::Char('u')));
    assert_eq!(app.filter_cfgs.len(), 2);
    // The anonymous name isn't offered: nothing to filter post 1 by but a word from it, which
    // it asks for straight away (esc: out).
    app.tab.thread.as_mut().unwrap().selected = 0;
    app.act(Action::Filter);
    let a = app.filter_add().unwrap();
    assert!(a.candidates.is_empty() && a.word.as_deref() == Some(""));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.filter_add().is_none());
}

#[test]
fn filters_from_a_catalog_by_subject_and_image() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = filter_app(dir.path());
    app.goto_str("a/x");
    let file = Attachment { filename: "cat.png".into(), md5: Some("q1w2e3==".into()), ..Default::default() };
    let op = |no, subject: &str| Post { no, subject: Some(subject.into()), name: "Anonymous".into(), files: vec![file.clone()], ..Default::default() };
    app.handle(Msg::Catalog(app.tab.req, Ok(vec![op(1, "Daily (thread)"), op(2, "Other"), op(3, "Daily (thread)")])));
    app.act(Action::Filter);
    let a = app.filter_add().unwrap();
    let fields: Vec<_> = a.candidates.iter().map(|c| c.field).collect();
    assert_eq!(fields, [crate::filter::Field::Md5, crate::filter::Field::Filename, crate::filter::Field::Subject]);
    app.on_key(KeyEvent::from(KeyCode::Char('G')));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    // Both threads with the subject are hidden; the selection stays on one that's shown.
    assert_eq!(app.visible_catalog(), [1]);
    assert_eq!(app.selected_index(), Some(1));
    // By the image: everything goes.
    app.act(Action::Filter);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.visible_catalog().is_empty());
    assert!(config_text(&app).contains("pattern = \"q1w2e3==\"\nfield = \"md5\""));
}

#[test]
fn the_filter_list_edits_turns_off_and_removes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = filter_app(dir.path());
    app.open_settings();
    app.settings_list.state.select(settings::position("Filters"));
    app.enter();
    let Some(SettingsPopup::Filters { counts, .. }) = app.settings_popup() else { panic!("no list") };
    assert_eq!(counts, &[(0, 0)]);
    // a: a new one, typing its pattern; a bad regex is refused, and isn't saved.
    app.on_key(KeyEvent::from(KeyCode::Char('a')));
    app.paste("(Named");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.status.as_ref().unwrap().error && app.filter_cfgs.len() == 1);
    app.on_key(KeyEvent::from(KeyCode::Home));
    for _ in 0..7 {
        app.on_key(KeyEvent::from(KeyCode::Backspace));
    }
    for c in "Named".chars() {
        app.on_key(KeyEvent::from(KeyCode::Char(c)));
    }
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.filter_cfgs.len(), 2);
    assert!(config_text(&app).ends_with("[[filter]]\npattern = \"Named\"\naction = \"hide\"\n"), "{}", config_text(&app));
    // Fields: the name too (row 5), then not the subject or comment (rows 3, 4).
    for (row, _) in [(5, ()), (3, ()), (4, ())] {
        if let Some(SettingsPopup::FilterEdit { row: r, .. }) = app.settings_popup_mut() {
            *r = row;
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
    }
    assert_eq!(app.filter_cfgs[1].fields(), [crate::filter::Field::Name]);
    assert!(app.tab.thread.as_ref().unwrap().marks[1].hidden.is_some());
    // The last field can't go.
    app.on_key(KeyEvent::from(KeyCode::Up));
    app.on_key(KeyEvent::from(KeyCode::Down));
    if let Some(SettingsPopup::FilterEdit { row: r, .. }) = app.settings_popup_mut() {
        *r = 5;
    }
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.status.as_ref().unwrap().text, "A filter needs at least one field");
    // Back to the list: it counts what it catches; space turns it off (kept in the file).
    app.on_key(KeyEvent::from(KeyCode::Esc));
    let Some(SettingsPopup::Filters { counts, list }) = app.settings_popup() else { panic!("not the list") };
    assert_eq!((counts[1], list.selected()), ((2, 0), Some(1)));
    app.on_key(KeyEvent::from(KeyCode::Char(' ')));
    assert!(config_text(&app).contains("enabled = false"));
    assert!(app.tab.thread.as_ref().unwrap().marks[1].hidden.is_none());
    let reloaded: Config = toml::from_str(&config_text(&app)).unwrap();
    assert!(!reloaded.filters[1].enabled && reloaded.filters[0].enabled);
    // x removes it; the first filter and its comments are as they were.
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    assert_eq!(config_text(&app), FILTER_CONFIG);
    // A filter changed in the file meanwhile isn't overwritten.
    std::fs::write(app.config_path.as_ref().unwrap(), FILTER_CONFIG.replace("(?i)spam", "eggs")).unwrap();
    app.on_key(KeyEvent::from(KeyCode::Char(' ')));
    assert!(app.status.as_ref().unwrap().text.contains("changed since ck read it"));
    assert!(config_text(&app).contains("eggs") && !config_text(&app).contains("enabled"));
}

// ----- conversations -----

/// A thread of `(no, quotes)`.
fn talk(posts: &[(u64, &[u64])]) -> ThreadView {
    let posts = posts.iter().map(|&(no, quotes)| Post { no, quotes: quotes.to_vec(), ..Default::default() }).collect();
    ThreadView::new("x".into(), 1, posts)
}

/// The conversation of post `no`: `(post, depth)` in thread order, and whether it was capped.
fn conv(t: &ThreadView, no: u64) -> (Vec<(u64, i32)>, bool) {
    let (depth, capped) = conversation_of(&t.posts, &t.index, &t.backlinks, t.index[&no]);
    (depth.into_iter().map(|(i, d)| (t.posts[i].no, d)).collect(), capped)
}

#[test]
fn conversations_go_up_and_down_never_sideways() {
    // 1 is the OP; 3 and 5 both reply to 2; 4 replies to 3, 6 to 4 and the OP, 7 to 5.
    let t = talk(&[(1, &[]), (2, &[1]), (3, &[2]), (4, &[3]), (5, &[2]), (6, &[4, 1]), (7, &[5]), (8, &[99])]);
    // Up to the OP (shown, not gone through), down to 6; not 5 or 7 (other replies to 2).
    assert_eq!(conv(&t, 3), (vec![(1, -2), (2, -1), (3, 0), (4, 1), (6, 2)], false));
    assert_eq!(conv(&t, 6).0, [(1, -1), (2, -3), (3, -2), (4, -1), (6, 0)]);
    // The OP's own: everything that replies to it, on down; 8 quotes another thread.
    assert_eq!(conv(&t, 1).0.len(), 7);
    // An OP quoting a later post is in that post's conversation, but nothing beyond it.
    let t = talk(&[(1, &[3]), (2, &[1]), (3, &[]), (4, &[1])]);
    assert_eq!(conv(&t, 3).0, [(1, 1), (3, 0)]);
    // Quote loops end.
    let t = talk(&[(1, &[]), (10, &[11]), (11, &[10, 11])]);
    assert_eq!(conv(&t, 10).0, [(10, 0), (11, -1)]);
    // At most 500, the nearest first.
    let chain: Vec<(u64, Vec<u64>)> = (1..=700u64).map(|no| (no, if no > 1 { vec![no - 1] } else { vec![] })).collect();
    let chain: Vec<(u64, &[u64])> = chain.iter().map(|(n, q)| (*n, q.as_slice())).collect();
    let t = talk(&chain);
    let (posts, capped) = conv(&t, 350);
    assert!(capped && posts.len() == CONVERSATION_MAX);
    // All 349 above (up to the OP), then the 150 nearest below.
    assert_eq!((posts.first().copied(), posts.last().copied()), (Some((1, -349)), Some((500, 150))));
}

#[test]
fn c_shows_a_conversation_until_esc() {
    let mut app = local_app();
    app.goto_str("a/x/1");
    let posts = [(1, vec![]), (2, vec![1]), (3, vec![2]), (4, vec![3]), (5, vec![2]), (6, vec![99])];
    let post = |&(no, ref quotes): &(u64, Vec<u64>)| Post { no, quotes: quotes.clone(), ..Default::default() };
    app.handle(Msg::Thread(app.tab.req, Ok(posts.iter().map(post).collect())));
    let t = app.tab.thread.as_mut().unwrap();
    t.scroll = 7;
    t.select(2);
    app.act(Action::Conversation);
    let shown = |app: &App| app.tab.thread.as_ref().unwrap().entries.iter().map(|e| (app.tab.thread.as_ref().unwrap().posts[e.post].no, e.depth)).collect::<Vec<_>>();
    assert_eq!(shown(&app), [(1, 0), (2, 0), (3, 0), (4, 1)]);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 3);
    // A refresh keeps it, with a new reply that belongs (and not one that doesn't).
    let mut more: Vec<Post> = posts.iter().map(post).collect();
    more.push(post(&(7, vec![4])));
    more.push(post(&(8, vec![5])));
    app.set_thread(more);
    assert_eq!(shown(&app), [(1, 0), (2, 0), (3, 0), (4, 1), (7, 2)]);
    // esc: the whole thread, the post still selected, scrolled where it was.
    app.on_key(KeyEvent::from(KeyCode::Esc));
    let t = app.tab.thread.as_ref().unwrap();
    assert!(t.conversation.is_none() && t.entries.len() == 8);
    assert_eq!((t.current().unwrap().no, t.scroll), (3, 7));
    // A post with nothing to talk about has none.
    app.tab.thread.as_mut().unwrap().select(5);
    app.act(Action::Conversation);
    assert!(app.tab.thread.as_ref().unwrap().conversation.is_none());
    assert!(app.status.as_ref().unwrap().text.contains("isn't part of a conversation"));
    // Jumping to a post outside it leaves it.
    app.tab.thread.as_mut().unwrap().select(3);
    app.act(Action::Conversation);
    assert!(app.tab.thread.as_mut().unwrap().jump_to(8));
    let t = app.tab.thread.as_ref().unwrap();
    assert!(t.conversation.is_none() && t.current().unwrap().no == 8);
    // u comes back (into the whole thread).
    app.act(Action::JumpBack);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 4);
}

#[test]
fn a_conversation_is_remembered_in_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.store = Store::load(Some(dir.path().to_path_buf())).0;
    app.goto_str("a/x/1");
    let posts = || vec![Post { no: 1, ..Default::default() }, Post { no: 2, quotes: vec![1], ..Default::default() }, Post { no: 3, quotes: vec![2], ..Default::default() }, Post { no: 4, quotes: vec![1], ..Default::default() }];
    app.handle(Msg::Thread(app.tab.req, Ok(posts())));
    app.tab.thread.as_mut().unwrap().select(1);
    app.act(Action::Conversation);
    app.tab.thread.as_mut().unwrap().select(2);
    app.save_session(None);
    let place = app.store.load_session().unwrap().tabs[0].clone();
    assert_eq!((place.conversation, place.selected), (Some(2), Some(3)));
    // An old session without it still loads.
    let old: crate::store::Place = serde_json::from_str(r#"{"view":"thread","site":"a","board":"x","thread":1}"#).unwrap();
    assert_eq!(old.conversation, None);
    let mut next = local_app();
    next.go_to_place(&place);
    next.handle(Msg::Thread(next.tab.req, Ok(posts())));
    let t = next.tab.thread.as_ref().unwrap();
    assert_eq!((t.conversation.as_ref().map(|c| c.anchor), t.current().unwrap().no, t.entries.len()), (Some(2), 3, 3));
}

#[test]
fn back_to_a_catalog_of_the_same_board_name_on_another_site_loads_it() {
    // (Found by fuzzing.) a/g's catalog, then a thread on b/g: back shows b/g's catalog, not a's.
    let mut app = app_with(
        "[[site]]\nname = \"a\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"g\"]\n\
         [[site]]\nname = \"b\"\nkind = \"vichan\"\nurl = \"http://localhost:3\"\nboards = [\"g\"]",
    );
    app.goto_str("a/g");
    app.handle(Msg::Catalog(app.tab.req, Ok(vec![Post { no: 1, ..Default::default() }])));
    app.goto_str("b/g/5");
    app.back();
    assert_eq!((app.tab.view, app.tab.site, app.tab.catalog_site), (View::Catalog, 1, 1));
    assert!(app.tab.catalog.is_empty() && app.tab.loading.is_some());
}

#[test]
fn the_viewer_goes_through_the_whole_thread_and_zooms() {
    use crate::images::Crop;
    let mut app = local_app();
    app.images = crate::images::Images::offline();
    app.goto_str("a/x/1");
    let file = |name: &str| Attachment { filename: name.into(), url: format!("http://127.0.0.1:9/{name}"), ..Default::default() };
    let post = |no, files: Vec<Attachment>| Post { no, files, ..Default::default() };
    app.handle(Msg::Thread(app.tab.req, Ok(vec![post(1, vec![file("a.png")]), post(2, vec![]), post(3, vec![file("b.png"), file("c.png")]), post(4, vec![file("d.png")])])));
    app.tab.thread.as_mut().unwrap().select(2);
    // v: every file of the thread, from the selected post's.
    app.act(Action::View);
    let v = app.tab.viewer().unwrap();
    assert_eq!((v.files.len(), v.index, v.posts.clone()), (4, 1, vec![1, 3, 3, 4]));
    let key = |app: &mut App, c: KeyCode| app.on_key(KeyEvent::from(c));
    key(&mut app, KeyCode::Char('l'));
    key(&mut app, KeyCode::Char('l'));
    assert_eq!(app.tab.viewer().unwrap().index, 3);
    // Zoomed, h/j/k/l move instead; page down is the next file (fitted again).
    key(&mut app, KeyCode::Char('+'));
    key(&mut app, KeyCode::Char('='));
    key(&mut app, KeyCode::Char('l'));
    let v = app.tab.viewer().unwrap();
    assert_eq!((v.index, v.crop.zoom), (3, 200));
    assert!(v.crop.x > 500);
    key(&mut app, KeyCode::Char('-'));
    assert_eq!(app.tab.viewer().unwrap().crop.zoom, 150);
    // esc fits first, then closes, on the post of the file last viewed.
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.tab.viewer().unwrap().crop, Crop::FIT);
    key(&mut app, KeyCode::PageDown);
    assert_eq!(app.tab.viewer().unwrap().index, 0);
    key(&mut app, KeyCode::Esc);
    assert!(app.tab.viewer().is_none());
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 1);
    // In a catalog, it's still the one thread's files.
    app.goto_str("a/x");
    app.handle(Msg::Catalog(app.tab.req, Ok(vec![post(1, vec![file("a.png"), file("e.png")])])));
    app.act(Action::View);
    let v = app.tab.viewer().unwrap();
    assert!(v.files.len() == 2 && v.posts.is_empty());
}

// ----- opening from the last copy -----

/// A vichan site on a test host: what was asked (with `If-Modified-Since`), how it answers
/// (200, 304 when asked if modified, 404 or 500), and a gate holding answers back.
/// Each request: its URL, and its `If-Modified-Since`.
type Asked = Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;

struct PageSite {
    asked: Asked,
    mode: Arc<std::sync::Mutex<u16>>,
    gate: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

impl PageSite {
    fn serve(host: &str) -> Self {
        let site = PageSite { asked: Default::default(), mode: Arc::new(std::sync::Mutex::new(200)), gate: Arc::new((std::sync::Mutex::new(true), std::sync::Condvar::new())) };
        let (asked, mode, gate) = (site.asked.clone(), site.mode.clone(), site.gate.clone());
        crate::http::serve_test_host(
            host,
            Some(Arc::new(move |url: &str, since: Option<&str>| {
                let mut open = http::lock(&gate.0);
                while !*open {
                    open = gate.1.wait(open).unwrap();
                }
                drop(open);
                http::lock(&asked).push((url.to_string(), since.map(String::from)));
                let fixture = if url.contains("/res/") { "vichan_thread.json" } else { "vichan_catalog.json" };
                let body = std::fs::read_to_string(format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"))).unwrap();
                let status = match *http::lock(&mode) {
                    304 if since == Some("day 1") => 304,
                    304 => 200,
                    m => m,
                };
                http::Raw { status, last_modified: Some("day 1".into()), body }
            })),
        );
        site
    }

    fn hold(&self, held: bool) {
        *http::lock(&self.gate.0) = !held;
        self.gate.1.notify_all();
    }

    fn asked(&self) -> Vec<Option<String>> {
        http::lock(&self.asked).iter().map(|(_, s)| s.clone()).collect()
    }
}

fn page_app(host: &str, dir: &std::path::Path) -> App {
    let mut app = app_with(&format!("[[site]]\nname = \"c\"\nkind = \"vichan\"\nurl = \"http://{host}\"\nboards = [\"g\"]\n"));
    app.store = Store::load(Some(dir.join("data"))).0;
    app.pages = Some(crate::pages::Pages::new(dir.join("pages"), 1 << 24));
    app.clock = Clock { fixed: Some(5000), ..Default::default() };
    app
}

#[test]
fn threads_open_from_their_last_copy_then_refresh() {
    let host = "pages-thread.invalid";
    let site = PageSite::serve(host);
    let dir = tempfile::tempdir().unwrap();
    // The first time: fetched, and kept.
    let mut app = page_app(host, dir.path());
    app.goto_str("c/g/30364");
    settle_until(&mut app, |a| a.tab.loading.is_none());
    let n = app.tab.thread.as_ref().unwrap().posts.len();
    assert!(n > 3 && app.tab.cached.is_none());
    assert_eq!(site.asked(), [None]);
    // A restart: shown at once from the copy (no request for that), marked cached.
    http::forget_host(host);
    let mut app = page_app(host, dir.path());
    site.hold(true);
    app.goto_str("c/g/30364");
    settle_until(&mut app, |a| a.tab.thread.is_some());
    assert_eq!(app.tab.cached, Some(tabs::Offline { saved: 5000, dead: false }));
    assert!(app.tab.loading.is_some());
    // Nothing is new against the last visit; reading on while it loads.
    let t = app.tab.thread.as_mut().unwrap();
    assert!((0..n).all(|i| !t.is_new(i)));
    t.select(3);
    // The refresh asks If-Modified-Since; unchanged, it's a 304, and the place stays.
    *http::lock(&site.mode) = 304;
    site.hold(false);
    settle_until(&mut app, |a| a.tab.loading.is_none());
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!((t.selected, t.posts.len(), app.tab.cached), (3, n, None));
    assert_eq!(site.asked(), [None, Some("day 1".into())]);
    // A failing refresh leaves the copy shown, still marked.
    for (mode, dead) in [(500, false), (404, true)] {
        http::forget_host(host);
        let mut app = page_app(host, dir.path());
        *http::lock(&site.mode) = mode;
        app.goto_str("c/g/30364");
        settle_until(&mut app, |a| a.tab.loading.is_none());
        assert_eq!(app.tab.cached.map(|c| c.dead), Some(dead), "{mode}");
        assert!(app.status.as_ref().unwrap().error);
    }
    // One request per open, whatever came from the copy.
    assert_eq!(site.asked().len(), 4);
    crate::http::serve_test_host(host, None);
}

#[test]
fn catalogs_open_from_their_last_copy_keeping_the_selection() {
    let host = "pages-catalog.invalid";
    let site = PageSite::serve(host);
    let dir = tempfile::tempdir().unwrap();
    let mut app = page_app(host, dir.path());
    app.goto_str("c/g");
    settle_until(&mut app, |a| a.tab.loading.is_none());
    let n = app.tab.catalog.len();
    assert!(n > 3);
    http::forget_host(host);
    let mut app = page_app(host, dir.path());
    site.hold(true);
    app.goto_str("c/g");
    settle_until(&mut app, |a| !a.tab.catalog.is_empty());
    assert!(app.tab.catalog_cached.is_some() && app.tab.catalog.len() == n);
    app.tab.catalog_list.state.select(Some(2));
    let picked = app.selected_index().map(|i| app.tab.catalog[i].no);
    site.hold(false);
    settle_until(&mut app, |a| a.tab.loading.is_none());
    assert!(app.tab.catalog_cached.is_none());
    assert_eq!(app.selected_index().map(|i| app.tab.catalog[i].no), picked);
    assert_eq!(site.asked().len(), 2);
    crate::http::serve_test_host(host, None);
}

#[test]
fn saving_needs_a_target_or_asks_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.download_dir = Some(dir.path().display().to_string());
    app.images = Images::offline();
    app.goto_str("a/x/1");
    let file = |name: &str| Attachment { filename: name.into(), url: format!("http://127.0.0.1:3/x/src/{name}"), size: Some(1 << 20), ..Default::default() };
    let posts = vec![
        Post { no: 1, files: vec![file("a.png")], ..Default::default() },
        Post { no: 2, files: vec![file("b.png"), file("c.png")], ..Default::default() },
        Post { no: 3, ..Default::default() },
    ];
    app.handle(Msg::Thread(app.tab.req, Ok(posts)));
    let press = |app: &mut App, c: char| app.on_key(KeyEvent::from(KeyCode::Char(c)));
    let total = |app: &App| app.downloads.total;
    // d on a post with nothing focused saves nothing, and says how.
    press(&mut app, 'd');
    assert_eq!(total(&app), 0);
    assert!(app.status.as_ref().unwrap().text.starts_with("tab to a file, then d saves it (the . menu saves"), "{:?}", app.status);
    // D and E aren't keys any more.
    press(&mut app, 'D');
    press(&mut app, 'E');
    assert!(app.confirm().is_none() && total(&app) == 0);
    // Focused, d saves the file.
    app.on_key(KeyEvent::from(KeyCode::Tab));
    press(&mut app, 'd');
    assert_eq!(total(&app), 1);
    app.on_key(KeyEvent::from(KeyCode::Esc));
    // All the thread's files: from the menu, which asks, saying what and where.
    run_menu_row(&mut app, "save all the thread's files…");
    let c = app.confirm().unwrap();
    assert_eq!(c.lines[0], "3 files (3.0 MB in all)");
    assert!(c.lines[1].starts_with("to ") && c.lines[1].ends_with(&dir.path().display().to_string()));
    // Anything but enter cancels.
    press(&mut app, 'j');
    assert!(app.confirm().is_none() && total(&app) == 1);
    assert_eq!(app.status.as_ref().unwrap().text, "Not saved");
    // So does a click.
    run_menu_row(&mut app, "save all the thread's files…");
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0), Instant::now());
    assert!(app.confirm().is_none() && total(&app) == 1);
    run_menu_row(&mut app, "save all the thread's files…");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.confirm().is_none() && total(&app) == 4);
    // One post's files, from the menu: no question.
    app.tab.thread.as_mut().unwrap().select(1);
    run_menu_row(&mut app, "save the post's files");
    assert_eq!(total(&app), 6);
    // In the viewer, d saves the file shown.
    press(&mut app, 'v');
    assert!(app.tab.viewer().is_some());
    press(&mut app, 'd');
    assert_eq!(total(&app), 7);
    run_menu_row(&mut app, "save it");
    assert_eq!(total(&app), 8);
}

#[test]
fn an_action_without_a_key_is_in_the_menu() {
    let dir = tempfile::tempdir().unwrap();
    let overrides = HashMap::from([
        ("download".to_string(), crate::keys::Binding::Many(vec![])),
        ("export".to_string(), crate::keys::Binding::One("E".into())),
    ]);
    let mut app = local_app();
    app.keys = KeyMap::new(&overrides).unwrap();
    app.download_dir = Some(dir.path().display().to_string());
    app.goto_str("a/x/1");
    let file = Attachment { filename: "a.png".into(), url: "http://127.0.0.1:3/x/src/a.png".into(), ..Default::default() };
    app.handle(Msg::Thread(app.tab.req, Ok(vec![Post { no: 1, files: vec![file], ..Default::default() }])));
    app.on_key(KeyEvent::from(KeyCode::Tab));
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    assert_eq!(app.downloads.total, 0);
    // The menu still runs it, with no key shown.
    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    let row = app.menu().unwrap().items.iter().find(|it| matches!(it, MenuItem::Act(Action::Download, _))).cloned().unwrap();
    assert_eq!(app.menu_key(&row), "");
    app.on_key(KeyEvent::from(KeyCode::Esc));
    run_menu_row(&mut app, "save this file");
    assert_eq!(app.downloads.total, 1);
    // A key given back still asks first.
    app.on_key(KeyEvent::from(KeyCode::Char('E')));
    assert!(app.confirm().is_some());
}

#[test]
fn a_link_to_a_new_site_adds_it_then_goes_there() {
    use crate::backend::detect::tests::serve;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = local_app();
    app.config_path = Some(path.clone());
    serve("newchan.invalid", vec![("/boards.json", crate::backend::fixture("jschan_boards.json"))]);
    app.act(Action::Goto);
    type_text(&mut app, "https://newchan.invalid/v/thread/5.html#7");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(matches!(app.adding(), Some(Adding::Looking { host, .. }) if host == "newchan.invalid"));
    settle_until(&mut app, |a| matches!(a.adding(), Some(Adding::Site { .. })));
    assert!(matches!(app.adding(), Some(Adding::Site { site, name, .. }) if name == "newchan" && site.kind == crate::config::SiteKind::Jschan));
    // The name can be changed; a taken one (or one with a slash) is refused.
    for _ in 0..7 {
        app.on_key(KeyEvent::from(KeyCode::Backspace));
    }
    type_text(&mut app, "A");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.adding().is_some() && app.status.as_ref().unwrap().text == "A site is already called A");
    app.on_key(KeyEvent::from(KeyCode::Backspace));
    type_text(&mut app, "my/chan");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.adding().is_some() && app.status.as_ref().unwrap().text.contains("can't have /"));
    for _ in 0..5 {
        app.on_key(KeyEvent::from(KeyCode::Backspace));
    }
    type_text(&mut app, "chan");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    // Added, saved, and the link opened on it.
    assert!(app.adding().is_none());
    let new = app.sites.len() - 1;
    assert_eq!((app.sites[new].cfg.name.as_str(), app.sites[new].cfg.url.as_deref()), ("mychan", Some("https://newchan.invalid")));
    let saved: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved.sites, [app.sites[new].cfg.clone()]);
    assert_eq!((app.tab.site, app.tab.view, app.tab.pending_thread, app.tab.pending_post), (new, View::Thread, 5, Some(7)));
    assert!(app.visible_sites().contains(&SiteRow::Site(new)));
    // From now on its links just open.
    app.goto_str("https://newchan.invalid/tech/");
    assert!(app.adding().is_none());
    assert_eq!((app.tab.site, app.tab.view), (new, View::Catalog));
    // A site that doesn't answer like any engine: said, nothing added. (After the catalog
    // load above has answered, so its error doesn't take the status.)
    settle_until(&mut app, |a| a.tab.loading.is_none());
    serve("blank.invalid", vec![]);
    app.goto_str("blank.invalid/b/");
    settle_until(&mut app, |a| a.adding().is_none());
    assert!(app.status.as_ref().unwrap().text.starts_with("blank.invalid doesn't answer like"), "{:?}", app.status);
    assert_eq!(app.sites.len(), new + 1);
    crate::http::serve_test_host("newchan.invalid", None);
    crate::http::serve_test_host("blank.invalid", None);
}

#[test]
fn settings_add_sites_and_vichan_boards_and_remove_them() {
    use crate::backend::detect::tests::serve;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = local_app();
    app.config_path = Some(path.clone());
    let catalog = crate::backend::fixture("vichan_catalog.json");
    serve("vi2.invalid", vec![("/tech/catalog.json", catalog.clone()), ("/b/catalog.json", catalog)]);
    app.tab.view = View::Sites;
    // From the home screen's menu (it has no key).
    run_menu_row(&mut app, "add a site…");
    app.paste("vi2.invalid/tech/");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    settle_until(&mut app, |a| matches!(a.adding(), Some(Adding::Site { .. })));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let i = app.sites.len() - 1;
    assert_eq!(app.status.as_ref().unwrap().text, "Added vi2 (vichan): it's on the home screen");
    assert_eq!(app.tab.view, View::Sites);
    // A link to a board it doesn't list adds the board; one it doesn't have is refused.
    app.popup = Some(Popup::Adding(Adding::Typing(String::new())));
    app.paste("https://vi2.invalid/b/res/1.html");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    settle_until(&mut app, |a| matches!(a.adding(), Some(Adding::Board { .. })));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let boards = |app: &App| app.sites[i].cfg.boards.clone().unwrap().iter().map(|b| crate::backend::to_board(b).uri).collect::<Vec<_>>();
    assert_eq!(boards(&app), ["tech", "b"]);
    let saved: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved.sites[0].boards, app.sites[i].cfg.boards);
    app.popup = Some(Popup::Adding(Adding::Typing("vi2.invalid/zz/".into())));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    settle_until(&mut app, |a| a.adding().is_none());
    assert!(app.status.as_ref().unwrap().text.contains("has no /zz/"));
    // One it has: nothing to do.
    app.popup = Some(Popup::Adding(Adding::Typing("vi2.invalid/b/".into())));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.adding().is_none() && app.status.as_ref().unwrap().text == "vi2 is already one of your sites");
    // esc while asking: the answer is dropped.
    app.popup = Some(Popup::Adding(Adding::Typing("vi2.invalid/b2/".into())));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    std::thread::sleep(Duration::from_millis(50));
    app.poll();
    assert!(app.adding().is_none());
    // Settings › Your sites lists it; x twice takes it out of the config and off the home screen.
    app.open_settings();
    let mine = settings::position("Your sites").unwrap();
    app.settings_list.state.select(Some(mine));
    app.activate_setting();
    assert!(matches!(app.settings_popup(), Some(SettingsPopup::Sites(m)) if m.sites.len() == 1 && m.sites[0].name == "vi2"));
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    assert_eq!(app.status.as_ref().unwrap().text, "x again removes vi2 from your config");
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    assert!(matches!(app.settings_popup(), Some(SettingsPopup::Sites(m)) if m.sites.is_empty()));
    assert!(!app.visible_sites().contains(&SiteRow::Site(i)));
    assert!(toml::from_str::<Config>(&std::fs::read_to_string(&path).unwrap()).unwrap().sites.is_empty());
    crate::http::serve_test_host("vi2.invalid", None);
}

fn thread_app_of(n: u64) -> App {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.view = View::Thread;
    app.set_thread(posts_upto(n));
    draw_at(&mut app, 100, 30);
    app
}

#[test]
fn reading_the_end_new_posts_come_into_view() {
    let mut app = thread_app_of(40);
    app.on_key(KeyEvent::from(KeyCode::Char('G')));
    draw_at(&mut app, 100, 30);
    assert!(app.tab.thread.as_ref().unwrap().at_end());
    // A refresh brings 41-45: the first new one is selected, at the margin, as `j` would.
    app.set_thread(posts_upto(45));
    draw_at(&mut app, 100, 30);
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!(t.current().unwrap().no, 41);
    let m = ((t.viewport as f32 * t.margin) as isize).min((t.viewport as isize - 1) / 2);
    assert_eq!(selected_row(&app), m);
    // They're new now (a first visit had nothing new), and nothing is left below.
    assert!(t.is_new(t.selected) && t.new_below() < 5);
    // Reading on to the end, the next refresh follows again.
    for _ in 0..5 {
        app.on_key(KeyEvent::from(KeyCode::Char('j')));
        draw_at(&mut app, 100, 30);
    }
    assert!(app.tab.thread.as_ref().unwrap().at_end());
    app.set_thread(posts_upto(46));
    draw_at(&mut app, 100, 30);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 46);
}

#[test]
fn reading_higher_up_nothing_moves() {
    let mut app = thread_app_of(40);
    app.on_key(KeyEvent::from(KeyCode::Char('G')));
    draw_at(&mut app, 100, 30);
    for _ in 0..12 {
        app.on_key(KeyEvent::from(KeyCode::Char('k')));
        draw_at(&mut app, 100, 30);
    }
    let (selected, scroll) = (app.tab.thread.as_ref().unwrap().selected, app.tab.thread.as_ref().unwrap().scroll);
    assert!(!app.tab.thread.as_ref().unwrap().at_end());
    app.set_thread(posts_upto(45));
    draw_at(&mut app, 100, 30);
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!((t.selected, t.scroll), (selected, scroll));
    // The top bar says how many are below; U goes to the first.
    assert_eq!(t.new_below(), 5);
    app.act(Action::Unread);
    draw_at(&mut app, 100, 30);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 41);
}

#[test]
fn following_edge_cases() {
    // Nothing new, or only changed posts: nothing moves.
    let mut app = thread_app_of(40);
    app.on_key(KeyEvent::from(KeyCode::Char('G')));
    draw_at(&mut app, 100, 30);
    let scroll = app.tab.thread.as_ref().unwrap().scroll;
    let mut changed = posts_upto(40);
    changed[39].body.push(Line::from("edited"));
    app.set_thread(changed);
    draw_at(&mut app, 100, 30);
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!((t.current().unwrap().no, t.new_below()), (40, 0));
    assert!(t.scroll >= scroll);
    // New posts that are hidden are passed over; all hidden: nothing moves.
    app.store.toggle_hidden("a", "x", 41);
    app.set_thread(posts_upto(42));
    draw_at(&mut app, 100, 30);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 42);
    // Turned off: nothing moves.
    let mut app = thread_app_of(40);
    app.follow_new_posts = false;
    app.on_key(KeyEvent::from(KeyCode::Char('G')));
    draw_at(&mut app, 100, 30);
    app.set_thread(posts_upto(45));
    draw_at(&mut app, 100, 30);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 40);
    // In a conversation, only posts that belong to it count.
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.view = View::Thread;
    let quoting = |no, q: u64| Post { no, quotes: vec![q], body: vec![Line::from("r")], ..Default::default() };
    app.set_thread(vec![Post { no: 1, ..Default::default() }, quoting(2, 1), Post { no: 3, ..Default::default() }]);
    draw_at(&mut app, 100, 30);
    app.act(Action::Conversation);
    app.on_key(KeyEvent::from(KeyCode::Char('G')));
    draw_at(&mut app, 100, 30);
    app.set_thread(vec![Post { no: 1, ..Default::default() }, quoting(2, 1), Post { no: 3, ..Default::default() }, Post { no: 4, ..Default::default() }]);
    draw_at(&mut app, 100, 30);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 2);
    app.set_thread(vec![Post { no: 1, ..Default::default() }, quoting(2, 1), Post { no: 3, ..Default::default() }, Post { no: 4, ..Default::default() }, quoting(5, 1)]);
    draw_at(&mut app, 100, 30);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 5);
}

#[test]
fn following_a_tall_last_post_waits_for_its_end() {
    // The last post is taller than the screen: while its start is shown, the end isn't on
    // screen, so a refresh doesn't move; at its last screenful it does.
    let mut posts = posts_upto(10);
    posts[9].body = (0..80).map(|k| Line::from(format!("tall line {k}"))).collect();
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.view = View::Thread;
    app.set_thread(posts.clone());
    draw_at(&mut app, 100, 30);
    app.act(Action::Unread);
    for _ in 0..9 {
        app.on_key(KeyEvent::from(KeyCode::Char('j')));
        draw_at(&mut app, 100, 30);
    }
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 10);
    let more = |posts: &Vec<Post>| {
        let mut p = posts.clone();
        p.push(Post { no: 11, body: vec![Line::from("eleven")], ..Default::default() });
        p
    };
    if !app.tab.thread.as_ref().unwrap().at_end() {
        app.set_thread(more(&posts));
        draw_at(&mut app, 100, 30);
        assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 10);
        app.set_thread(posts.clone());
        draw_at(&mut app, 100, 30);
    }
    for _ in 0..10 {
        if app.tab.thread.as_ref().unwrap().at_end() {
            break;
        }
        app.on_key(KeyEvent::from(KeyCode::Char('j')));
        draw_at(&mut app, 100, 30);
    }
    assert!(app.tab.thread.as_ref().unwrap().at_end());
    app.set_thread(more(&posts));
    draw_at(&mut app, 100, 30);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 11);
}

/// A site whose board list marks /x/ NSFW and /xy/ not; /z/ isn't in it.
fn nsfw_app() -> App {
    let mut app = local_app();
    app.images = Images::offline();
    app.sites[0].boards = Some(vec![
        Board { uri: "x".into(), title: String::new(), nsfw: Some(true) },
        Board { uri: "xy".into(), title: String::new(), nsfw: Some(false) },
    ]);
    app
}

#[test]
fn which_image_setting_applies_to_a_board() {
    use crate::config::NsfwImages;
    let mut app = nsfw_app();
    // The default shows everything.
    assert!(app.images_on(0, "x") && app.images_on(0, "xy") && app.images_on(0, "z"));
    // NSFW boards off: only the board the site marks; one it says nothing of is safe.
    app.nsfw_images = NsfwImages::Off;
    assert!(!app.images_on(0, "x") && app.images_on(0, "xy") && app.images_on(0, "z"));
    // A board's own setting comes first, either way.
    app.store.board_prefs.entry("a/x".into()).or_default().images = Some(true);
    app.store.board_prefs.entry("a/xy".into()).or_default().images = Some(false);
    assert!(app.images_on(0, "x") && !app.images_on(0, "xy"));
    // The other site's boards of the same name are their own.
    assert!(app.images_on(1, "xy"));
}

#[test]
fn the_menu_switch_sets_and_resets_a_boards_own_setting() {
    use crate::config::NsfwImages;
    let dir = tempfile::tempdir().unwrap();
    let mut app = nsfw_app();
    app.store = Store::load(Some(dir.path().to_path_buf())).0;
    app.sites[0].boards = nsfw_app().sites[0].boards.clone();
    app.goto_str("a/xy");
    run_menu_row(&mut app, "images on this board: on → off");
    assert!(!app.images_on(0, "xy"));
    assert_eq!(app.store.board_prefs["a/xy"].images, Some(false));
    run_menu_row(&mut app, "images on this board: off → on");
    // Back to what the default gives: no setting of its own.
    assert_eq!(app.store.board_prefs["a/xy"].images, None);
    // On an NSFW board with NSFW boards off, turning images on is its own setting.
    app.nsfw_images = NsfwImages::Off;
    app.goto_str("a/x");
    run_menu_row(&mut app, "images on this board: off → on");
    assert_eq!(app.store.board_prefs["a/x"].images, Some(true));
    assert_eq!(app.boards_with_images_set(), [("a/x".to_string(), true)]);
    // Kept in the data directory; older files without it load.
    app.save_now();
    assert_eq!(Store::load(Some(dir.path().to_path_buf())).0.board_prefs["a/x"].images, Some(true));
    let old: crate::store::BoardPrefs = serde_json::from_str(r#"{"sort": "newest"}"#).unwrap();
    assert_eq!(old.images, None);
}

#[test]
fn no_images_are_asked_for_on_a_board_with_images_off() {
    use crate::config::NsfwImages;
    let mut app = nsfw_app();
    app.nsfw_images = NsfwImages::Off;
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: Some(true) });
    app.tab.view = View::Thread;
    app.set_thread((1..=60).map(|no| with_file(no, None)).collect());
    draw_at(&mut app, 100, 30);
    // Nothing on screen, nothing prefetched below it, nothing in the gallery.
    assert_eq!(app.images.queued_urls(), Vec::<String>::new());
    app.act(Action::Gallery);
    draw_at(&mut app, 100, 30);
    assert!(app.images.queued_urls().is_empty());
    // The viewer says why instead of opening.
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.tab.viewer().is_none() && app.status.as_ref().unwrap().text.starts_with("Images are off on /x/"));
    // The same thread on a board with images: asked for.
    app.tab.gallery = None;
    app.tab.board = Some(Board { uri: "xy".into(), title: String::new(), nsfw: Some(false) });
    app.tab.thread = None;
    app.set_thread((1..=60).map(|no| with_file(no, None)).collect());
    app.tab.thread.as_mut().unwrap().board = "xy".into();
    draw_at(&mut app, 100, 30);
    assert!(!app.images.queued_urls().is_empty());
}

#[test]
fn an_overboard_follows_each_threads_board() {
    use crate::config::NsfwImages;
    let mut app = nsfw_app();
    app.nsfw_images = NsfwImages::Off;
    app.tab.view = View::Catalog;
    app.tab.catalog_site = 0;
    app.tab.catalog_board = "all".into();
    app.tab.catalog = (1..=6).map(|no| with_file(no, Some(if no % 2 == 0 { "x" } else { "xy" }))).collect();
    app.tab.catalog_marks = vec![Default::default(); 6];
    app.tab.catalog_list.state.select(Some(0));
    draw_at(&mut app, 100, 40);
    let asked = app.images.queued_urls();
    assert!(!asked.is_empty());
    assert!(asked.iter().all(|u| ["1", "3", "5"].iter().any(|n| u.ends_with(&format!("/{n}.png")))), "{asked:?}");
    // The overboard's own setting covers all of it.
    app.store.board_prefs.entry("a/all".into()).or_default().images = Some(false);
    let mut fresh = nsfw_app();
    std::mem::swap(&mut fresh.images, &mut app.images);
    draw_at(&mut app, 100, 40);
    assert!(app.images.queued_urls().is_empty());
}

#[test]
fn an_unknown_nsfw_flag_loads_the_board_list_once() {
    use crate::config::NsfwImages;
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.store = Store::load(Some(dir.path().to_path_buf())).0;
    // Showing NSFW boards: nothing is loaded for this.
    app.know_nsfw(0);
    assert!(app.boards_refreshing.is_empty());
    // Off: the saved list if there is one, else the site's list, once.
    app.nsfw_images = NsfwImages::Off;
    app.know_nsfw(0);
    assert_eq!(app.boards_refreshing.len(), 1);
    app.boards_refreshing.clear();
    app.know_nsfw(0);
    assert!(app.boards_refreshing.is_empty());
    app.store.save_boards("b", &[Board { uri: "y".into(), title: String::new(), nsfw: Some(true) }], 5).unwrap();
    app.know_nsfw(1);
    assert!(app.boards_refreshing.is_empty() && !app.images_on(1, "y"));
}

#[test]
fn searching_inside_saved_threads() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = |board: &str, no| ThreadKey { site: "a".into(), board: board.into(), no };
    app.store.keep_thread(&key("x", 1), "one", "u", &posts_saying(&[(1, "about rust"), (2, "nothing here"), (3, "Rust again")]), 900);
    app.store.keep_thread(&key("xy", 7), "seven", "u", &posts_saying(&[(7, "no match"), (8, "a crab: RUST")]), 950);
    app.store.keep_thread(&key("x", 9), "nine", "u", &posts_saying(&[(9, "quiet")]), 980);
    app.flush_writes();
    app.tab.view = View::Saved;
    // From the Saved view's menu: `:` with "saved " typed.
    run_menu_row(&mut app, "search inside the saved threads…");
    assert_eq!(app.goto.as_deref(), Some("saved "));
    type_text(&mut app, "rust");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.tab.view, View::Search);
    settle_until(&mut app, |a| a.tab.search.as_ref().unwrap().saved.as_ref().unwrap().finished);
    let s = app.tab.search.as_ref().unwrap();
    // Newest copy first; case doesn't matter.
    let found: Vec<(u64, u64)> = s.hits.iter().map(|(t, p)| (*t, p.no)).collect();
    assert_eq!(found, [(7, 8), (1, 1), (1, 3)]);
    assert_eq!(s.saved.as_ref().unwrap().done, 3);
    draw_at(&mut app, 100, 30);
    // Enter: the saved copy, on the post, with the search set (n / N go through it).
    app.tab.search_list.state.select(Some(2));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let t = app.tab.thread.as_ref().unwrap();
    assert!(app.tab.offline.is_some());
    assert_eq!((t.no, t.current().unwrap().no, t.search.as_str(), t.matches.len()), (1, 3, "rust", 2));
    // esc: back to the results, then where the search started.
    app.on_key(KeyEvent::from(KeyCode::Esc));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.tab.view, View::Search);
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.tab.view, View::Saved);
    // A copy that can't be read is skipped and said; the rest still count.
    std::fs::write(crate::saved::path(dir.path(), &key("x", 9)), b"not json").unwrap();
    app.goto_str("saved quiet");
    settle_until(&mut app, |a| a.tab.search.as_ref().unwrap().saved.as_ref().unwrap().finished);
    let s = app.tab.search.as_ref().unwrap();
    assert_eq!((s.hits.len(), s.saved.as_ref().unwrap().skipped), (0, 1));
    assert_eq!(app.status.as_ref().unwrap().text, "No saved post matches \"quiet\"");
    // Another search stops the one running: only its answers count.
    app.goto_str("saved rust");
    app.goto_str("saved crab");
    settle_until(&mut app, |a| a.tab.search.as_ref().unwrap().saved.as_ref().unwrap().finished);
    let s = app.tab.search.as_ref().unwrap();
    assert_eq!((s.query.as_str(), s.hits.len()), ("crab", 1));
}

#[test]
fn searching_saved_threads_with_none_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    app.goto_str("saved anything");
    settle_until(&mut app, |a| a.tab.search.as_ref().is_some_and(|s| s.saved.as_ref().unwrap().finished));
    assert!(app.tab.search.as_ref().unwrap().hits.is_empty());
    assert!(app.status.as_ref().unwrap().text.starts_with("Nothing is saved yet"));
    // `saved` alone is still the Saved view.
    app.goto_str("saved");
    assert_eq!(app.tab.view, View::Saved);
}

#[test]
fn a_board_list_update_is_what_the_bar_says() {
    use crate::app::BoardsUpdate;
    use crate::config::BoardConfig;
    let full = |u: &str, t: &str| BoardConfig::Full { uri: u.into(), title: t.into() };
    let list = vec![full("wiz", "Wizardry"), full("old", "Old board"), BoardConfig::Uri("dep".into())];
    let bar = vec![full("wiz", "Wizards"), full("dep", "Depression"), full("new", "New board")];
    let u = BoardsUpdate::new(&list, bar);
    assert_eq!(u.added, [full("new", "New board")]);
    assert_eq!(u.missing, [full("old", "Old board")]);
    // A title where there was none counts; so does a changed one.
    assert_eq!(u.renamed, [("wiz".into(), "Wizardry".into(), "Wizards".into()), ("dep".into(), String::new(), "Depression".into())]);
    let uris = |l: Vec<BoardConfig>| l.iter().map(|b| b.uri().to_string()).collect::<Vec<_>>();
    assert_eq!(uris(u.list(false)), ["wiz", "dep", "new", "old"]);
    assert_eq!(uris(u.list(true)), ["wiz", "dep", "new"]);
    assert!(BoardsUpdate::new(&u.bar, u.bar.clone()).is_empty());
}

#[test]
fn updating_a_vichan_sites_boards() {
    use crate::backend::detect::tests::serve_text;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let site = "[[site]]\nname = \"vb\"\nkind = \"vichan\"\nurl = \"https://vb.invalid\"\n# my boards\nboards = [{ uri = \"wiz\", title = \"Wizardry\" }, \"gone\"]\n";
    std::fs::write(&path, format!("# my config\n{site}")).unwrap();
    let mut app = app_with(site);
    app.config_path = Some(path.clone());
    let bar = r#"<div class="boardlist">[ <a href="/wiz/index.html" title="Wizardry">wiz</a> / <a href="/dep/index.html" title="Depression">dep</a> ]</div>"#;
    serve_text("vb.invalid", vec![("/", bar.into())]);
    // Settings › Your sites › r.
    app.open_settings();
    let mine = settings::position("Your sites").unwrap();
    app.settings_list.state.select(Some(mine));
    app.activate_setting();
    app.on_key(KeyEvent::from(KeyCode::Char('r')));
    settle_until(&mut app, |a| matches!(a.adding(), Some(Adding::Boards { .. })));
    let Some(Adding::Boards { update, drop: false, .. }) = app.adding() else { panic!("{:?}", app.status) };
    assert_eq!((update.added.len(), update.missing.len()), (1, 1));
    // d drops what the bar doesn't have; enter writes it, comments kept, and it's in use.
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# my config\n") && text.contains("# my boards"), "{text}");
    let saved: Config = toml::from_str(&text).unwrap();
    let uris: Vec<String> = saved.sites[0].boards.as_ref().unwrap().iter().map(|b| b.uri().to_string()).collect();
    assert_eq!(uris, ["wiz", "dep"]);
    assert_eq!(app.sites[0].cfg.boards, saved.sites[0].boards);
    // Again: nothing to change.
    app.refresh_board_list(0);
    settle_until(&mut app, |a| a.adding().is_none());
    assert_eq!(app.status.as_ref().unwrap().text, "vb's board list is up to date");
    // Pages without a bar: said, nothing changes.
    serve_text("vb.invalid", vec![("/", "<html></html>".into())]);
    crate::http::forget_host("vb.invalid");
    app.refresh_board_list(0);
    settle_until(&mut app, |a| a.adding().is_none());
    assert!(app.status.as_ref().unwrap().text.contains("have no board list to read"));
    assert_eq!(app.sites[0].cfg.boards, saved.sites[0].boards);
    crate::http::serve_test_host("vb.invalid", None);
}

#[test]
fn hidden_words_hide_posts_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "# my config\nnotify = \"off\" # quiet\n").unwrap();
    let mut app = local_app();
    app.config_path = Some(path.clone());
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.view = View::Thread;
    app.set_thread(posts_saying(&[(1, "op"), (2, "free crypto here"), (3, "I like Crypto"), (4, "cryptography")]));
    // Settings › Filters › Hidden words: a, type, enter.
    app.open_settings();
    let at = settings::position("Hidden words").unwrap();
    app.settings_list.state.select(Some(at));
    app.activate_setting();
    app.on_key(KeyEvent::from(KeyCode::Char('a')));
    type_text(&mut app, "crypto");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.status.as_ref().unwrap().text.starts_with("Hiding posts with \"crypto\" (2 here)"), "{:?}", app.status);
    let hidden = |app: &App| app.tab.thread.as_ref().unwrap().marks.iter().map(|m| m.hidden.clone()).collect::<Vec<_>>();
    let label = Some("hidden word: crypto".to_string());
    assert_eq!(hidden(&app), [None, label.clone(), label.clone(), None]);
    // Saved, the file's comments kept; a config without it loads as before.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# my config") && text.contains("# quiet") && text.contains("hidden_words = [\"crypto\"]"), "{text}");
    assert!(toml::from_str::<Config>("[[site]]\nname = \"s\"\nkind = \"4chan\"\n").unwrap().hidden_words.is_empty());
    // x in the list takes it out.
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    assert!(app.hidden_words.is_empty() && hidden(&app).iter().all(Option::is_none));
    assert!(!std::fs::read_to_string(&path).unwrap().contains("hidden_words"));
    app.popup = None;
    app.tab.view = View::Thread;
    // From a post's X: w, the thread's search to start with; u right after takes it back.
    app.tab.thread.as_mut().unwrap().posts[1].name = "Satoshi".into();
    app.tab.thread.as_mut().unwrap().set_search("free".into());
    app.tab.thread.as_mut().unwrap().select(1);
    app.open_add_filter();
    assert!(app.filter_add().is_some_and(|a| !a.candidates.is_empty() && a.word.is_none()));
    app.on_key(KeyEvent::from(KeyCode::Char('w')));
    assert_eq!(app.filter_add().unwrap().word.as_deref(), Some("free"));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.filter_add().is_none());
    assert_eq!(app.hidden_words, ["free"]);
    assert_eq!(hidden(&app)[1], Some("hidden word: free".into()));
    app.on_key(KeyEvent::from(KeyCode::Char('u')));
    assert!(app.hidden_words.is_empty() && hidden(&app)[1].is_none());
    // Catalogs too.
    app.hidden_words = vec!["crypto".into()];
    app.apply_filters();
    app.tab.catalog = posts_saying(&[(10, "crypto thread"), (11, "a thread")]);
    app.tab.catalog_board = "x".into();
    app.remark_catalog();
    assert_eq!(app.tab.catalog_marks.iter().map(|m| m.hidden.clone()).collect::<Vec<_>>(), [label, None]);
}

#[test]
fn g_shows_the_very_end_so_new_posts_follow() {
    // A last post that doesn't fit below the scroll margin on a small screen: G still shows
    // the end of the thread (found walking a busy thread at 60x20).
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.view = View::Thread;
    let mut posts = posts_upto(8);
    posts[7].body = (0..9).map(|k| Line::from(format!("line {k}"))).collect();
    app.set_thread(posts.clone());
    draw_at(&mut app, 60, 20);
    app.on_key(KeyEvent::from(KeyCode::Char('G')));
    draw_at(&mut app, 60, 20);
    assert!(app.tab.thread.as_ref().unwrap().at_end());
    posts.push(Post { no: 9, body: vec![Line::from("nine")], ..Default::default() });
    app.set_thread(posts);
    draw_at(&mut app, 60, 20);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 9);
    // A last post taller than the screen: its last screenful.
    let mut app = thread_app_of(3);
    let mut tall = posts_upto(3);
    tall[2].body = (0..60).map(|k| Line::from(format!("tall {k}"))).collect();
    app.set_thread(tall);
    draw_at(&mut app, 60, 20);
    app.on_key(KeyEvent::from(KeyCode::Char('G')));
    draw_at(&mut app, 60, 20);
    let t = app.tab.thread.as_ref().unwrap();
    assert!(t.at_end() && t.tall().is_some_and(|(_, above, below)| above && !below));
}

#[test]
fn going_to_the_site_already_on_shows_its_boards() {
    // The app starts on the first site; `ck a` (or `:a`) goes there: its boards show.
    let mut app = local_app();
    assert_eq!(app.tab.site, 0);
    app.goto_str("a");
    assert_eq!(app.tab.view, View::Boards);
    assert_eq!(app.visible_boards().len(), 2);
}

/// The popup open, by kind: what tests used to read as fields.
impl App {
    pub fn menu(&self) -> Option<&Menu> {
        if let Some(Popup::Menu(m)) = &self.popup { Some(m) } else { None }
    }
    pub fn menu_mut(&mut self) -> Option<&mut Menu> {
        if let Some(Popup::Menu(m)) = &mut self.popup { Some(m) } else { None }
    }
    pub fn hints(&self) -> Option<&Hints> {
        if let Some(Popup::Hints(h)) = &self.popup { Some(h) } else { None }
    }
    pub fn confirm(&self) -> Option<&saving::Confirm> {
        if let Some(Popup::Confirm(c)) = &self.popup { Some(c) } else { None }
    }
    pub fn adding(&self) -> Option<&Adding> {
        if let Some(Popup::Adding(a)) = &self.popup { Some(a) } else { None }
    }
    pub fn adding_mut(&mut self) -> Option<&mut Adding> {
        if let Some(Popup::Adding(a)) = &mut self.popup { Some(a) } else { None }
    }
    pub fn filter_add(&self) -> Option<&AddFilter> {
        if let Some(Popup::AddFilter(a)) = &self.popup { Some(a) } else { None }
    }
    pub fn settings_popup(&self) -> Option<&SettingsPopup> {
        if let Some(Popup::Settings(p)) = &self.popup { Some(p) } else { None }
    }
    pub fn settings_popup_mut(&mut self) -> Option<&mut SettingsPopup> {
        if let Some(Popup::Settings(p)) = &mut self.popup { Some(p) } else { None }
    }
    pub fn image_search_panel(&self) -> Option<&ImageSearchPanel> {
        if let Some(Popup::ImageSearch(p)) = &self.popup { Some(p) } else { None }
    }
}

#[test]
fn a_quote_preview_survives_a_refresh_that_drops_posts() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.view = View::Thread;
    let quoting = |no, q: u64| Post { no, quotes: vec![q], body: vec![Line::from(format!("reply {no}"))], ..Default::default() };
    let plain = |no| Post { no, body: vec![Line::from(format!("post {no}"))], ..Default::default() };
    app.set_thread(vec![plain(1), plain(2), plain(3), plain(4), plain(5), quoting(6, 5)]);
    draw_at(&mut app, 100, 30);
    app.on_key(KeyEvent::from(KeyCode::Char('G')));
    app.act(Action::Preview);
    assert!(matches!(app.tab.popup, Some(TabPopup::Preview(_))));
    // Posts 2-4 deleted: the quoted post moves from the fifth place to the second.
    app.set_thread(vec![plain(1), plain(5), quoting(6, 5)]);
    let screen = draw_at(&mut app, 100, 30);
    assert!(screen.content.iter().map(|c| c.symbol()).collect::<String>().contains("post 5"));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 5);
}

#[test]
fn huge_refresh_intervals_in_the_config_dont_overflow_the_clock() {
    let huge = i64::MAX;
    let config = crate::config::DEFAULT_CONFIG
        .replace("refresh_thread_secs = 10", &format!("refresh_thread_secs = {huge}"))
        .replace("refresh_watched_secs = 60", &format!("refresh_watched_secs = {huge}"));
    let mut app = app_with(&config);
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.view = View::Thread;
    app.set_thread(nos(&[1, 2]));
    let now = Instant::now();
    assert!(app.next_wake(now) <= Duration::from_secs(1));
    assert_eq!(app.refresh_thread, Duration::from_secs(86400));
}
