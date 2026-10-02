//! The app's behavior, driven through keys, clicks and messages.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::*;
use crate::keys::ACTIONS;

/// An app over `config` (TOML), with an empty data directory, that never writes the real
/// config file.
pub fn app_with(config: &str) -> App {
    let cfg: Config = toml::from_str(config).unwrap();
    let mut app = App::new(cfg, KeyMap::default(), None, Store::default());
    app.config_path = None;
    app
}

/// An app over the default config; nothing here touches the network.
pub fn test_app() -> App {
    app_with(crate::config::DEFAULT_CONFIG)
}

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
    t.layout = Some(ThreadLayout { width: 40, blocks: vec![block.clone(), block.clone(), block], starts: vec![0, 4, 8, 12], thumbs: vec![], spots: vec![Rc::from([]); 3] });
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
    app.settings_list.state.select(Some(settings::items().iter().position(|&i| i == settings::Item::Keys).unwrap()));
    app.activate_setting();
    // Move to `watch` and rebind it to W.
    let rows = settings::key_rows();
    let watch = rows.iter().position(|r| *r == Ok(ACTIONS.iter().position(|e| e.0 == Action::Watch).unwrap())).unwrap();
    while app.settings_popup.as_ref().is_some_and(|p| !matches!(p, SettingsPopup::Keys { list, .. } if list.selected() == Some(watch))) {
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
    app.settings_popup = None;
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
    app.tab.viewer = None;
    // Catalog: subject and text; a thread link.
    app.tab.catalog = vec![post(7, vec![Line::raw("hello")])];
    app.tab.catalog_list.state.select(Some(0));
    app.tab.view = View::Catalog;
    app.act(Action::Copy);
    assert_eq!(app.copied.as_deref(), Some("Subj\nhello"));
    app.act(Action::CopyLink);
    assert_eq!(app.copied.as_deref(), Some("https://boards.4chan.org/g/thread/7"));
}

/// Two sites on hosts that refuse connections: nothing leaves the machine. (Their own
/// port, so the requests don't take rate-limit slots other tests' hosts need.)
fn local_app() -> App {
    app_with(
        "[[site]]\nname = \"a\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"x\", \"xy\"]\n\
         [[site]]\nname = \"b\"\nkind = \"vichan\"\nurl = \"http://localhost:3\"\nboards = [\"y\"]",
    )
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
    app.goto_str("https://example.com/g/");
    assert!(app.status.as_ref().unwrap().error);
    assert_eq!(app.tab.view, View::Boards);
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
    let kinds: Vec<String> = app.tab.links.as_ref().unwrap().items.iter().map(|i| match i {
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
    assert!(app.tab.links.is_none());
    assert_eq!(app.opened.as_deref(), Some("https://example.com/a"));
    // Enter on the quote opens its thread, and `u` will come back.
    app.act(Action::Links);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!((app.tab.board.as_ref().unwrap().uri.as_str(), app.tab.pending_thread, app.tab.pending_post), ("xy", 9, Some(10)));
    assert_eq!(app.tab.trail.len(), 1);
    // A post without links says so.
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }]));
    app.act(Action::Links);
    assert!(app.tab.links.is_none() && app.status.as_ref().unwrap().text == "Post has no links");
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
    assert_eq!((app.tab.viewer.as_ref().unwrap().files.len(), app.tab.viewer.as_ref().unwrap().index), (3, 2));
    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('Y'));
    assert_eq!(app.copied.as_deref(), Some("http://127.0.0.1:3/x/res/1.html#3"));
    press(&mut app, KeyCode::Esc);
    assert!(app.tab.viewer.is_none());
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
    let rows = &app.image_search_panel.as_ref().unwrap().rows;
    assert_eq!(rows.len(), 2 + 2 * 4);
    assert_eq!(rows[0], Err("a.png".into()));
    assert_eq!(rows[6], Ok(("https://i.example/bs.jpg".into(), 0)));
    app.on_key(KeyEvent::from(KeyCode::Down));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.opened.as_deref(), Some("https://lens.google.com/uploadbyurl?url=https%3A%2F%2Fi.example%2Fa.png"));
    assert!(app.image_search_panel.is_none());
    // In the viewer: the file shown.
    app.act(Action::View);
    app.on_key(KeyEvent::from(KeyCode::Char('R')));
    assert_eq!(app.image_search_panel.as_ref().unwrap().rows.len(), 4);
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
    app.settings_popup = Some(SettingsPopup::Folder { value: String::new() });
    app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 1, row: 0, modifiers: KeyModifiers::NONE }, Instant::now());
    assert_eq!((app.active, app.tab.view), (0, View::Settings));
    app.settings_popup = None;
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
    // They're on the home screen after Watched and History; 2 opens the second.
    app.tab.view = View::Sites;
    assert_eq!(app.visible_sites()[2..4], [SiteRow::Favorite(0), SiteRow::Favorite(1)]);
    app.on_key(KeyEvent::from(KeyCode::Char('1')));
    assert_eq!((app.tab.site, app.tab.view, app.tab.board.as_ref().unwrap().uri.as_str()), (1, View::Catalog, "y"));
    app.tab.view = View::Sites;
    app.on_key(KeyEvent::from(KeyCode::Char('2')));
    assert_eq!((app.tab.site, app.tab.board.as_ref().unwrap().uri.as_str()), (0, "xy"));
    // x on a favorite row takes it off; * again on the board does too.
    app.tab.view = View::Sites;
    app.site_list.state.select(Some(2));
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
    assert_eq!(rows[2..5], [SiteRow::Favorite(0), SiteRow::Recent(0), SiteRow::Recent(2)]);
    // Enter opens; x forgets it.
    app.site_list.state.select(Some(4));
    app.enter();
    assert_eq!((app.tab.view, app.tab.board.as_ref().unwrap().uri.as_str()), (View::Catalog, "x"));
    app.tab.view = View::Sites;
    app.site_list.state.select(Some(3));
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
    app.site_list.state.select(Some(2));
    app.act(Action::Remove);
    assert_eq!(sites(&app), [SiteRow::Site(1), SiteRow::HiddenSites]);
    let c: Config = toml::from_str(&std::fs::read_to_string(dir.path().join("config.toml")).unwrap()).unwrap();
    assert_eq!(c.hidden_sites, ["a"]);
    // The last row shows them; x on one brings it back.
    app.site_list.state.select(Some(3));
    app.enter();
    assert_eq!(sites(&app), [SiteRow::Site(0), SiteRow::Site(1), SiteRow::HiddenSites]);
    app.site_list.state.select(Some(2));
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
    assert_eq!(app.store.board_prefs["a/x"], crate::store::BoardPrefs { sort: Some(Sort::Replies), layout: Some(CatalogLayout::Compact) });
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
    let layout = || Some(ThreadLayout { width: 40, blocks: Vec::new(), starts: vec![0, 0], thumbs: Vec::new(), spots: Vec::new() });
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
    assert_eq!(app.tab.viewer.as_ref().map(|v| v.index), Some(1));
    app.tab.viewer = None;
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
    let m = app.menu.as_ref().unwrap();
    let has = |a: Action| m.items.iter().any(|i| matches!(i, MenuItem::Act(x, _) if *x == a));
    assert!(has(Action::View) && has(Action::Watch) && has(Action::Gallery) && !has(Action::Preview));
    // A row's own key runs it, and the menu closes.
    app.on_key(KeyEvent::from(KeyCode::Char('w')));
    assert!(app.menu.is_none() && app.status.as_ref().is_some_and(|s| s.text.starts_with("Watching")));
    // So does enter on a row.
    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    let at = app.menu.as_ref().unwrap().items.iter().position(|i| matches!(i, MenuItem::Act(Action::Watch, _))).unwrap();
    app.menu.as_mut().unwrap().list.select(Some(at));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.menu.is_none() && app.status.as_ref().is_some_and(|s| s.text.starts_with("Stopped watching")));
    // Esc just closes it.
    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.menu.is_none());
}
