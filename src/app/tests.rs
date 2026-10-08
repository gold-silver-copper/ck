//! The app's behavior, driven through keys, clicks and messages.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::*;
use crate::keys::ACTIONS;
use crate::store::Status;
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
    let list = Some(Hit::List { area: Rect::new(1, 2, 60, 10), offset: 0, item_height: 1 });
    app.begin_frame().body = list;
    let t0 = Instant::now();
    app.on_mouse(mouse(MouseEventKind::ScrollDown, 5, 5), t0);
    assert_eq!(app.site_list.state.selected(), Some(1));
    app.on_mouse(mouse(MouseEventKind::ScrollUp, 5, 5), t0);
    assert_eq!(app.site_list.state.selected(), Some(0));

    // Row 3 is the second item (History), in the frame after the wheel.
    app.begin_frame().body = list;
    let left = MouseEventKind::Down(MouseButton::Left);
    app.on_mouse(mouse(left, 5, 3), t0);
    assert_eq!(app.site_list.state.selected(), Some(1));
    assert_eq!(app.tab.view(), View::Sites);
    // A slow second click is just another click; a quick one opens.
    app.on_mouse(mouse(left, 5, 3), t0 + Duration::from_secs(1));
    assert_eq!(app.tab.view(), View::Sites);
    app.on_mouse(mouse(left, 5, 3), t0 + Duration::from_millis(1200));
    assert_eq!(app.tab.view(), View::History);

    // Clicks outside the list do nothing.
    app.tab.navigate(View::Sites);
    app.begin_frame().body = list;
    app.on_mouse(mouse(left, 5, 30), t0);
    assert_eq!(app.site_list.state.selected(), Some(1));
}

#[test]
fn clicks_on_a_settings_popup_never_reach_the_rows_behind() {
    let mut app = local_app();
    app.tab.navigate(View::Settings);
    app.settings_list.state.select(settings::position("Hidden words"));
    app.enter();
    assert!(matches!(app.popup, Some(Popup::Settings(SettingsPopup::HiddenWords { .. }))));
    draw_at(&mut app, 100, 40);
    // A double click on the Hidden replies row, beside the popup: nothing changes.
    let Some(Hit::Settings { area, offset }) = app.drawn.body else { panic!("no settings rows") };
    let pos = settings::position("Hidden replies").unwrap();
    let row = setting_rows().iter().position(|r| *r == Ok(pos)).unwrap() - offset;
    let left = MouseEventKind::Down(MouseButton::Left);
    let t0 = Instant::now();
    app.on_mouse(mouse(left, area.x, area.y + row as u16), t0);
    app.on_mouse(mouse(left, area.x, area.y + row as u16), t0 + Duration::from_millis(100));
    assert!(!app.hiding.recursive());
    assert_eq!(app.settings_list.state.selected(), settings::position("Hidden words"));
    assert!(matches!(app.popup, Some(Popup::Settings(SettingsPopup::HiddenWords { .. }))));
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
    app.tab.navigate(View::Thread);
    app.begin_frame().body = Some(Hit::Thread { area: Rect::new(0, 1, 40, 10), scroll: 0 });
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 1 + 9), Instant::now());
    assert_eq!(app.tab.thread.as_ref().unwrap().selected, 2);
    app.on_mouse(mouse(MouseEventKind::ScrollDown, 3, 3), Instant::now());
    assert_eq!(app.tab.thread.as_ref().unwrap().scroll, 2);
}

/// The footer is drawn: what it says has been seen (an error waits for that).
fn drawn(app: &mut App) {
    app.footer.tick(app.clock.instant(), true);
}

/// The footer's message has gone.
fn expired(app: &mut App) {
    app.footer = Footer::default();
}

// ----- the mouse reaches only what's on top, as it was drawn -----

/// A wheel notch while the key editor waits for a key is not a key: nothing is bound or
/// refused, and the editor still waits for one.
#[test]
fn the_wheel_never_answers_the_key_editor() {
    let mut app = test_app();
    app.tab.navigate(View::Settings);
    app.settings_list.state.select(Some(settings::position("Key bindings").unwrap()));
    app.activate_setting();
    // enter: waiting for the key to bind.
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(matches!(app.settings_popup(), Some(SettingsPopup::Keys { capture: Some(false), .. })));
    app.on_mouse(mouse(MouseEventKind::ScrollDown, 5, 5), Instant::now());
    assert!(app.status().is_none(), "the wheel was taken for a key: {:?}", app.status());
    assert!(matches!(app.settings_popup(), Some(SettingsPopup::Keys { capture: Some(false), .. })), "the editor stopped waiting");
}

/// The wheel over a save question doesn't scroll the thread behind it.
#[test]
fn the_wheel_over_a_save_question_leaves_the_thread_alone() {
    let mut app = thread_app();
    app.set_thread(nos(&(1..=60).collect::<Vec<_>>()));
    draw_at(&mut app, 80, 20);
    app.popup = Some(Popup::Confirm(saving::Confirm { what: saving::Saving::Page, title: "Save the thread as a page?", lines: vec![] }));
    let before = app.tab.thread.as_ref().unwrap().scroll;
    app.on_mouse(mouse(MouseEventKind::ScrollDown, 5, 5), Instant::now());
    assert_eq!(app.tab.thread.as_ref().unwrap().scroll, before, "the thread behind scrolled");
    assert!(app.confirm().is_some());
}

/// Over a list, the notch that leaves a save question open over a thread mustn't answer it
/// "no".
#[test]
fn the_wheel_never_answers_a_save_question() {
    let mut app = test_app();
    app.popup = Some(Popup::Confirm(saving::Confirm { what: saving::Saving::Page, title: "Save the thread as a page?", lines: vec![] }));
    app.on_mouse(mouse(MouseEventKind::ScrollDown, 5, 5), Instant::now());
    assert!(app.confirm().is_some(), "the wheel answered: {:?}", app.status());
    assert_eq!(app.site_list.state.selected(), Some(0));
}

/// Over the filter maker the wheel moves its choices, not the thread behind it.
#[test]
fn the_wheel_moves_the_filter_choices_over_a_thread() {
    let mut app = thread_app();
    let mut posts = nos(&(1..=60).collect::<Vec<_>>());
    posts[0].name = "Satoshi".into();
    posts[0].subject = Some("Bitcoin".into());
    app.set_thread(posts);
    draw_at(&mut app, 80, 20);
    app.open_add_filter();
    assert!(app.filter_add().is_some_and(|a| a.candidates.len() > 1));
    app.on_mouse(mouse(MouseEventKind::ScrollDown, 5, 5), Instant::now());
    assert_eq!(app.tab.thread.as_ref().unwrap().scroll, 0, "the thread behind scrolled");
    assert_eq!(app.filter_add().unwrap().list.selected(), Some(1));
}

/// The wheel over a right-click menu isn't a key: `u` still takes back the filter just added.
#[test]
fn the_wheel_over_a_menu_keeps_the_filter_undo() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = filter_app(dir.path());
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.act(Action::Filter);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_ne!(config_text(&app), FILTER_CONFIG);
    draw_at(&mut app, 80, 20);
    let right = MouseEventKind::Down(MouseButton::Right);
    app.on_mouse(mouse(right, 5, 3), Instant::now());
    assert!(matches!(app.popup, Some(Popup::Menu(_))));
    app.on_mouse(mouse(MouseEventKind::ScrollDown, 5, 3), Instant::now());
    draw_at(&mut app, 80, 20);
    app.on_mouse(mouse(right, 5, 3), Instant::now());
    assert!(app.popup.is_none());
    app.on_key(KeyEvent::from(KeyCode::Char('u')));
    assert_eq!(config_text(&app), FILTER_CONFIG, "the wheel dropped the undo");
}

/// A double click on a post while a go-to is being typed doesn't submit the go-to.
#[test]
fn a_double_click_never_submits_a_goto() {
    let mut app = thread_app();
    app.set_thread(nos(&(1..=60).collect::<Vec<_>>()));
    draw_at(&mut app, 80, 20);
    app.act(Action::Goto);
    for c in "zz/".chars() {
        app.on_key(KeyEvent::from(KeyCode::Char(c)));
    }
    let left = MouseEventKind::Down(MouseButton::Left);
    let t0 = Instant::now();
    app.on_mouse(mouse(left, 5, 4), t0);
    app.on_mouse(mouse(left, 5, 4), t0 + Duration::from_millis(100));
    assert_eq!((app.tab.view(), app.goto_text()), (View::Thread, Some("zz/")), "the go-to was submitted: {:?}", app.status());
}

/// A thread that's still loading draws no rows: the list drawn before it can't be clicked.
#[test]
fn a_loading_thread_keeps_no_clickable_rows_from_the_last_view() {
    let mut app = test_app();
    draw_at(&mut app, 80, 20);
    assert!(matches!(app.drawn.body, Some(Hit::List { .. })));
    app.tab.navigate(View::Thread);
    app.tab.fake_load(1, "Loading", Then::Thread { open: Opening::default() });
    draw_at(&mut app, 80, 20);
    assert!(app.drawn.body.is_none(), "stale {:?}", app.drawn.body);
}

/// Input handled before the next frame can change what's shown: a click then lands on
/// nothing, not on the rows the last frame drew for another view.
#[test]
fn a_click_after_leaving_a_view_waits_for_the_next_frame() {
    let mut app = local_app();
    app.switch_site(0);
    app.tab.navigate(View::Boards);
    draw_at(&mut app, 80, 20);
    let Some(Hit::List { area, .. }) = app.drawn.body else { panic!("no boards drawn") };
    app.on_key(KeyEvent::from(KeyCode::Char('h')));
    assert_eq!(app.tab.view(), View::Sites);
    let before = app.site_list.state.selected();
    // The second board's row; on the sites list it would be the second site.
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1, area.y + 1), Instant::now());
    assert_eq!(app.site_list.state.selected(), before, "the click landed on the boards drawn before");
}

/// A wheel notch handled before the next frame moves what's under the pointer: a click then
/// lands on nothing, and after the frame on the post drawn there.
#[test]
fn a_click_after_a_scroll_waits_for_the_next_frame() {
    let mut app = thread_app();
    app.set_thread(nos(&(1..=60).collect::<Vec<_>>()));
    draw_at(&mut app, 80, 20);
    let (col, row) = (5, 10);
    let selected = |app: &App| app.tab.thread.as_ref().unwrap().selected;
    for _ in 0..4 {
        app.on_mouse(mouse(MouseEventKind::ScrollDown, col, row), Instant::now());
    }
    assert!(app.tab.thread.as_ref().unwrap().scroll >= 12);
    let before = selected(&app);
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), col, row), Instant::now());
    assert_eq!(selected(&app), before, "the click landed on the posts as they were drawn before");
    draw_at(&mut app, 80, 20);
    let shown = app.thread_part_at(col, row).unwrap().0;
    assert_ne!(shown, before);
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), col, row), Instant::now());
    assert_eq!(selected(&app), shown);
}

/// `f` right after a wheel notch would label the posts as they were drawn, over a frame
/// showing others: it waits for the frame too.
#[test]
fn hints_after_a_scroll_wait_for_the_next_frame() {
    let mut app = thread_app();
    app.set_thread(nos(&(1..=60).collect::<Vec<_>>()));
    draw_at(&mut app, 80, 20);
    app.on_mouse(mouse(MouseEventKind::ScrollDown, 5, 5), Instant::now());
    app.on_key(KeyEvent::from(KeyCode::Char('f')));
    assert!(!matches!(app.popup, Some(Popup::Hints(_))), "labels on posts no longer drawn there");
    draw_at(&mut app, 80, 20);
    app.on_key(KeyEvent::from(KeyCode::Char('f')));
    assert!(matches!(app.popup, Some(Popup::Hints(_))));
}

/// The quote peek covers the posts under it: a click on it reaches none of them.
#[test]
fn clicks_never_reach_the_posts_under_the_quote_peek() {
    use crate::model::Target;
    let mut app = local_app();
    app.goto_str("a/x/1");
    let html = r##"<a href="#p1" class="quotelink">&gt;&gt;1</a>"##;
    let mut posts = nos(&(1..=60).collect::<Vec<_>>());
    posts[1] = Post { no: 2, ..crate::markup::parse_html(html, crate::markup::Flavor::Vichan).into() };
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts)));
    app.on_key(KeyEvent::from(KeyCode::Char('j')));
    draw_at(&mut app, 80, 20);
    let Some(Hit::Thread { area, .. }) = app.drawn.body else { panic!("no thread drawn") };
    // The post drawn on a row near the top and one near the bottom.
    let col = area.x + 5;
    let drawn = |row| (row, app.thread_part_at(col, row).map(|(e, _)| e));
    let (top, bottom) = (drawn(area.y + 1), drawn(area.bottom() - 2));
    app.on_key(KeyEvent::from(KeyCode::Tab));
    let focus = |app: &App| app.tab.thread.as_ref().unwrap().focus.clone();
    assert!(matches!(focus(&app), Some(Part::Link(Target::Quote(_)))));
    draw_at(&mut app, 80, 20);
    // The peek covers the top or the bottom rows of the thread, as drawn the frame before.
    let Some(Hit::Thread { area: after, .. }) = app.drawn.body else { panic!("no thread drawn") };
    let (row, under) = if after.y > area.y { top } else { bottom };
    assert!(under.is_some_and(|e| e != 1), "no post behind the peek");
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), col, row), Instant::now());
    assert_eq!(app.tab.thread.as_ref().unwrap().selected, 1, "the click reached a post under the peek");
    assert!(matches!(focus(&app), Some(Part::Link(Target::Quote(_)))));
}

/// A menu clicked before a frame has drawn it isn't closed by the click: the click waits for
/// the frame, then picks the row there.
#[test]
fn a_menu_clicked_before_it_is_drawn_still_picks_a_row() {
    let mut app = thread_app();
    app.set_thread(nos(&(1..=60).collect::<Vec<_>>()));
    draw_at(&mut app, 80, 20);
    let right = MouseEventKind::Down(MouseButton::Right);
    app.on_mouse(mouse(right, 5, 5), Instant::now());
    draw_at(&mut app, 80, 20);
    let Some(Hit::List { area, .. }) = app.drawn.popup else { panic!("no menu drawn") };
    assert_ne!(area, Rect::default());
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.popup.is_none());
    draw_at(&mut app, 80, 20);
    // Right click then left click in one input batch, as a fast double tap does.
    app.on_mouse(mouse(right, 5, 5), Instant::now());
    let items = app.menu().unwrap().items.len();
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1, area.y + 1), Instant::now());
    assert!(items > 1 && (app.popup.is_some() || app.status().is_some() || app.opened.is_some()), "the click closed the menu without picking a row");
    draw_at(&mut app, 80, 20);
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), area.x + 1, area.y + 1), Instant::now());
    assert!(app.menu().is_none() && (app.popup.is_some() || app.status().is_some() || app.opened.is_some() || app.tab.popup.is_some()), "the click picked no row");
}

/// Clicks on the same row in two tabs, with a tab chip clicked between, aren't a double click.
#[test]
fn a_click_in_another_tab_is_not_a_double_click() {
    let mut app = test_app();
    app.tabs.push(Tab::new(0, Instant::now()));
    let list = Some(Hit::List { area: Rect::new(1, 2, 60, 10), offset: 0, item_height: 1 });
    let frame = app.begin_frame();
    frame.tabs = vec![(Rect::new(0, 0, 5, 1), 1)];
    frame.body = list;
    let left = MouseEventKind::Down(MouseButton::Left);
    let t0 = Instant::now();
    app.on_mouse(mouse(left, 5, 3), t0);
    app.on_mouse(mouse(left, 1, 0), t0 + Duration::from_millis(100));
    assert_eq!(app.active, 1);
    app.begin_frame().body = list;
    app.on_mouse(mouse(left, 5, 3), t0 + Duration::from_millis(200));
    assert_eq!(app.tab.view(), View::Sites, "one click opened it");
}

/// Two clicks on one row with the list re-sorted between them are on two threads: not a
/// double click.
#[test]
fn a_list_re_sorted_between_two_clicks_is_not_a_double_click() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let ops = |nos: &[u64]| nos.iter().map(|&no| Post { no, ..Default::default() }).collect::<Vec<_>>();
    app.load_catalog(None);
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(ops(&[1, 2, 3, 4]))));
    app.tab.navigate(View::Catalog);
    draw_at(&mut app, 80, 30);
    let Some(hit) = app.drawn.body else { panic!("no catalog drawn") };
    let at = (0..30).find(|&row| hit.row_at(5, row) == Some(2)).unwrap();
    let t0 = Instant::now();
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, at), t0);
    app.handle(Msg::Done(Box::new(move |app: &mut App| app.catalog_arrived(Ok(ops(&[4, 3, 2, 1]))))));
    draw_at(&mut app, 80, 30);
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, at), t0 + Duration::from_millis(100));
    assert_eq!(app.tab.view(), View::Catalog, "one click on each thread opened one");
}

/// A preview tall enough to reach the tab row covers the chips there: a click on its title
/// closes it, and doesn't switch tabs under it.
#[test]
fn a_tall_preview_covers_the_tab_chips() {
    let mut app = local_app();
    app.tabs.push(Tab::new(0, Instant::now()));
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.navigate(View::Thread);
    let mut posts: Vec<Post> = (1..=20).map(|no| Post { no, body: vec![Line::from(format!("post {no}"))], ..Default::default() }).collect();
    posts.push(Post { no: 21, quotes: (1..=20).collect(), ..Default::default() });
    app.set_thread(posts);
    draw_at(&mut app, 80, 20);
    let Some(&(chip, 1)) = app.drawn.tabs.last() else { panic!("no second chip") };
    app.on_key(KeyEvent::from(KeyCode::Char('G')));
    app.act(Action::Preview);
    draw_at(&mut app, 80, 20);
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), chip.x, chip.y), Instant::now());
    assert_eq!(app.active, 0, "the click went through the preview to a chip");
    assert!(app.tab.popup.is_none());
}

/// A list that comes back from a refresh between a frame and a click, in one batch, may
/// be in another order: the click waits for the frame that shows it.
#[test]
fn a_click_after_a_list_answer_waits_for_the_next_frame() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let ops = |nos: &[u64]| nos.iter().map(|&no| Post { no, ..Default::default() }).collect::<Vec<_>>();
    app.load_catalog(None);
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(ops(&[1, 2, 3, 4]))));
    app.tab.navigate(View::Catalog);
    draw_at(&mut app, 80, 30);
    let Some(hit) = app.drawn.body else { panic!("no catalog drawn") };
    let at = (0..30).find(|&row| hit.row_at(5, row) == Some(2)).unwrap();
    let click = |app: &mut App| {
        let ev = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 5, row: at, modifiers: KeyModifiers::NONE };
        app.handle(Msg::Input(Event::Mouse(ev)));
    };
    let selected = |app: &App| app.tab.catalog_list.state.selected();
    app.load_catalog(None);
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(ops(&[4, 3, 2, 1]))));
    let before = selected(&app);
    click(&mut app);
    assert_eq!(selected(&app), before, "the click landed on the list as it was drawn before");
    draw_at(&mut app, 80, 30);
    click(&mut app);
    assert_eq!(selected(&app), Some(2));
    // So does what background work found (a watched refresh re-sorts Saved): the list
    // comes back reversed between this frame and the click.
    draw_at(&mut app, 80, 30);
    app.handle(Msg::Done(Box::new(move |app: &mut App| app.catalog_arrived(Ok(ops(&[1, 2, 3, 4]))))));
    app.tab.catalog_list.state.select(Some(0));
    click(&mut app);
    assert_eq!(selected(&app), Some(0), "the click landed on the list as it was drawn before");
}

#[test]
fn sleeps_until_the_next_thing_to_do() {
    let mut app = test_app();
    let now = Instant::now();
    // Idle: at most a second.
    assert_eq!(app.next_wake(now), Duration::from_secs(1));
    // A spinner animates.
    app.tab.fake_load(1, "Loading", Then::Show);
    assert_eq!(app.next_wake(now), Duration::from_millis(100));
    app.tab.navigate(View::Sites);
    // A status message wakes the loop when it's due to disappear.
    app.info("hi");
    app.footer.tick(now - Duration::from_millis(1700), true);
    assert_eq!(app.next_wake(now), Duration::from_millis(300));
    expired(&mut app);
    // A watched thread that was never refreshed is due now.
    let key = ThreadKey { site: "4chan".into(), board: "g".into(), no: 1 };
    app.store.watch(key, String::new(), 1, 1);
    assert_eq!(app.next_wake(now), Duration::ZERO);
    // But while the maximum number of refreshes is running, due ones don't spin the loop.
    for no in [2, 3] {
        app.refreshing.insert(ThreadKey { site: "4chan".into(), board: "g".into(), no });
    }
    assert_eq!(app.next_wake(now), Duration::from_millis(100));
}

/// `refreshed`, with the site's answer for `key`: `posts` as it sent them.
fn refresh(app: &mut App, key: ThreadKey, posts: Vec<Post>) {
    let no = key.no;
    app.refreshed(key, Thread::answer(no, posts));
}

/// A thread answer: `posts`, of the thread their first post starts.
fn arrived(posts: Vec<Post>) -> anyhow::Result<Thread> {
    Thread::answer(posts.first().map_or(0, |p| p.no), posts)
}

/// What load `id`'s job comes to once it has `found` something: `apply` it.
fn answer<T: Send + 'static>(id: u64, apply: fn(&mut App, T), found: T) -> Msg {
    Msg::answer(id, found, apply)
}

/// A thread load's answer, for the thread the tab last asked for (as `load_thread`'s own
/// closure is handed the key it asked for).
fn thread_arrived(app: &mut App, res: Result<Thread>) {
    let board = app.tab.board.as_ref().map(|b| b.uri.clone()).unwrap_or_default();
    let key = app.key(&board, app.tab.pending_thread.unwrap_or_default());
    app.thread_arrived(&key, res);
}

/// What request `id`'s job sends back while it goes on (a page so far): `apply` it.
fn partial<T: Send + 'static>(id: u64, apply: fn(&mut App, T), found: T) -> Msg {
    Msg::request(id, move |a| apply(a, found))
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
    app.tab.fake_load(7, "Loading boards", Then::Show);
    app.handle(partial(7, |a, b| a.set_boards(0, b, false), vec![board("a")]));
    assert_eq!(app.sites[0].boards.as_ref().unwrap().len(), 1);
    assert!(app.tab.loading().is_some());
    // A stale request's pages are ignored.
    app.handle(partial(6, |a, b| a.set_boards(0, b, false), vec![board("x"), board("y"), board("z")]));
    assert_eq!(app.sites[0].boards.as_ref().unwrap().len(), 1);
    let boards = Ok(vec![board("a"), board("b")]);
    app.handle(Msg::kept(7, move |a| a.boards_arrived(7, 0, boards)));
    assert_eq!(app.sites[0].boards.as_ref().unwrap().len(), 2);
    assert!(app.tab.loading().is_none());

    app.tab.fake_load(8, "Loading /a/", Then::Catalog { select: None });
    app.handle(partial(8, App::catalog_partial, vec![Post { no: 1, ..Default::default() }]));
    assert_eq!((app.tab.catalog.len(), app.tab.loading().is_some()), (1, true));
}

#[test]
fn partial_catalog_pages_keep_the_selected_thread() {
    let mut app = test_app();
    let upto = |n: u64| (1..=n).map(|no| Post { no, ..Default::default() }).collect::<Vec<_>>();
    app.tab.navigate(View::Catalog);
    app.tab.catalog = upto(30);
    app.tab.catalog_list.state.select(Some(24));
    // A refresh's first page has 10 threads, the next 20: thread 25 isn't there yet, and
    // the one the selection is moved to isn't kept instead.
    app.tab.fake_load(8, "Loading /a/", Then::Catalog { select: None });
    app.handle(partial(8, App::catalog_partial, upto(10)));
    app.handle(partial(8, App::catalog_partial, upto(20)));
    app.handle(answer(8, App::catalog_arrived, Ok(upto(30))));
    assert_eq!(app.tab.catalog_list.state.selected(), Some(24));
    // Once it's there, moving on from it is kept.
    app.tab.fake_load(9, "Loading /a/", Then::Catalog { select: None });
    app.handle(partial(9, App::catalog_partial, upto(28)));
    assert_eq!(app.tab.catalog_list.state.selected(), Some(24));
    app.tab.catalog_list.state.select(Some(2));
    app.handle(answer(9, App::catalog_arrived, Ok(upto(30))));
    assert_eq!(app.tab.catalog_list.state.selected(), Some(2));
}

#[test]
fn overboard_threads_open_on_their_board_and_back_returns() {
    // A local site that refuses connections: nothing leaves the machine.
    let mut app = app_with("[[site]]\nname = \"t\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:9\"\nboards = [\"ob\"]");
    app.tab.board = Some(Board { uri: "ob".into(), title: "Overboard".into(), nsfw: None });
    app.load_catalog(None);
    app.tab.catalog = vec![Post { no: 5, board: Some("tech".into()), ..Default::default() }];
    app.tab.catalog_list.state.select(Some(0));
    app.tab.navigate(View::Catalog);
    app.enter();
    assert_eq!((app.tab.view(), app.tab.board.as_ref().unwrap().uri.as_str()), (View::Thread, "tech"));
    assert_eq!(app.tab.pending_thread.unwrap(), 5);
    app.back();
    assert_eq!((app.tab.view(), app.tab.board.as_ref().unwrap().uri.as_str()), (View::Catalog, "ob"));
    // The overboard's catalog is still there; nothing was reloaded.
    assert_eq!(app.tab.catalog.len(), 1);
}

#[test]
fn key_editor_rebinds_saves_and_refuses_clashes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = test_app();
    app.config_path = Some(dir.path().join("config.toml"));
    let press = |app: &mut App, code| app.on_key(KeyEvent::from(code));
    app.tab.navigate(View::Settings);
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
    assert!(app.footer.get().is_some_and(|s| s.error && s.text.contains("'v'")), "{:?}", app.footer.get());
    // x resets to the default, which removes the entry.
    press(&mut app, KeyCode::Char('x'));
    assert!(app.keys.is_default(Action::Watch));
    let c: Config = toml::from_str(&std::fs::read_to_string(dir.path().join("config.toml")).unwrap()).unwrap();
    assert!(!c.keys.contains_key("watch"));
    // The new keys work at once.
    app.popup = None;
    app.tab.navigate(View::Catalog);
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
    app.tab.navigate(View::Thread);
    app.act(Action::Copy);
    assert_eq!(app.copied.as_deref(), Some(">>1\n>green\nsecret text"));
    assert_eq!(app.footer.get().unwrap().text, "Copied 22 characters");
    app.act(Action::CopyLink);
    assert_eq!(app.copied.as_deref(), Some("https://boards.4chan.org/g/thread/1#p2"));
    // The viewer copies the file's URL, or the post's link.
    app.tab.thread.as_mut().unwrap().posts[1].files = vec![Attachment::at("https://i.4cdn.org/g/1.png")];
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
    app.tab.navigate(View::Catalog);
    app.act(Action::Copy);
    assert_eq!(app.copied.as_deref(), Some("Subj\nhello"));
    app.act(Action::CopyLink);
    assert_eq!(app.copied.as_deref(), Some("https://boards.4chan.org/g/thread/7"));
}

#[test]
fn goto_opens_places_and_u_comes_back() {
    let mut app = local_app();
    app.goto_str("b/y/5#6");
    assert_eq!((app.tab.site, app.tab.view(), app.tab.pending_thread.unwrap(), app.tab.opening().select), (1, View::Thread, 5, Some(6)));
    assert_eq!(app.tab.board.as_ref().unwrap().uri, "y");
    // Esc from there returns to where : was typed.
    assert_eq!(app.tab.return_to, Some(View::Sites));
    // From a thread, `u` comes back across sites.
    app.tab.thread = Some(ThreadView::new("y".into(), 5, vec![Post { no: 5, ..Default::default() }, Post { no: 6, ..Default::default() }]));
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.goto_str("http://127.0.0.1:3/x/res/3.html#4");
    assert_eq!((app.tab.site, app.tab.pending_thread.unwrap(), app.tab.board.as_ref().unwrap().uri.as_str()), (0, 3, "x"));
    app.tab.thread = None;
    app.act(Action::JumpBack);
    assert!(app.tab.thread.is_none());
    // (JumpBack needs a loaded thread; simulate the arrival of thread 3.)
    app.tab.thread = Some(ThreadView::new("x".into(), 3, vec![Post { no: 3, ..Default::default() }]));
    app.act(Action::JumpBack);
    assert_eq!((app.tab.site, app.tab.pending_thread.unwrap(), app.tab.opening().select, app.tab.board.as_ref().unwrap().uri.as_str()), (1, 5, Some(6), "y"));
    // A board opens its catalog; a site its boards.
    app.goto_str("a/xy");
    assert_eq!((app.tab.site, app.tab.view(), app.tab.board.as_ref().unwrap().uri.as_str()), (0, View::Catalog, "xy"));
    app.goto_str("b");
    assert_eq!((app.tab.site, app.tab.view()), (1, View::Boards));
    // Errors are said, not acted on.
    app.goto_str("a/x/abc");
    assert!(app.footer.get().unwrap().error);
    assert_eq!(app.tab.view(), View::Boards);
    // A link to a site ck doesn't have: it asks the site what it runs, to add it. Also
    // without a scheme or a path (not a board of the current site called that).
    for link in ["https://example.com/g/", "example.com"] {
        app.popup = None;
        app.goto_str(link);
        assert!(matches!(app.adding(), Some(Adding::Looking { host, .. }) if host == "example.com"), "{link}");
        assert_eq!(app.tab.view(), View::Boards);
    }
    app.popup = None;
}

#[test]
fn a_post_on_another_site_moves_the_tab_once_found() {
    let host = "lookup.invalid";
    crate::http::serve_test_host(
        host,
        Some(Arc::new(|url: &str, _: Option<&str>| {
            let (status, body) = if url.contains("num=77") { (200, r#"{"thread_num":"3"}"#) } else { (404, "") };
            http::Raw { status, last_modified: None, body: body.into() }
        })),
    );
    let mut app = app_with(&format!(
        "[[site]]\nname = \"a\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"x\"]\n\
         [[site]]\nname = \"f\"\nkind = \"foolfuuka\"\nurl = \"http://{host}\"\nboards = [\"b\"]"
    ));
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }, Post { no: 2, ..Default::default() }]));
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.tab.navigate(View::Thread);
    // Not found: the thread shown stays on its own site, and nothing is left for `u`.
    app.goto_str(&format!("http://{host}/b/post/99/"));
    assert_eq!(app.tab.site, 0);
    settle_until(&mut app, |a| a.tab.loading().is_none());
    assert!(app.footer.get().unwrap().error);
    assert_eq!((app.tab.site, app.tab.thread.as_ref().unwrap().no, app.tab.trail.len()), (0, 1, 0));
    // Found: the tab moves to its thread there, and `u` comes back to where it was asked.
    app.goto_str(&format!("http://{host}/b/post/77/"));
    assert_eq!(app.tab.site, 0);
    settle_until(&mut app, |a| a.tab.pending_thread == Some(3));
    assert_eq!((app.tab.site, app.tab.board.as_ref().unwrap().uri.as_str(), app.tab.opening().select), (1, "b", Some(77)));
    let trail: Vec<_> = app.tab.trail.iter().map(|(site, b, no, post)| (*site, b.uri.as_str(), *no, *post)).collect();
    assert_eq!(trail, [(0, "x", 1, 2)]);
    crate::http::serve_test_host(host, None);
}

#[test]
fn goto_input_completes_and_takes_pastes() {
    let mut app = local_app();
    app.act(Action::Goto);
    app.paste("a/");
    app.on_key(KeyEvent::from(KeyCode::Tab));
    // x and xy: completes the common part and lists both.
    assert_eq!(app.goto_text(), Some("a/x"));
    assert!(app.footer.get().unwrap().text.contains("xy"));
    app.on_key(KeyEvent::from(KeyCode::Char('y')));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!((app.goto_text(), app.tab.view(), app.tab.board.as_ref().unwrap().uri.as_str()), (None, View::Catalog, "xy"));
    // Site names complete with a slash.
    app.act(Action::Goto);
    app.on_key(KeyEvent::from(KeyCode::Char('b')));
    app.on_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(app.goto_text(), Some("b/"));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    // A paste with nothing being typed starts the input.
    app.paste("http://localhost:3/y/res/1.html\n");
    assert_eq!(app.goto_text(), Some("http://localhost:3/y/res/1.html"));
}

#[test]
fn links_panel_lists_and_opens() {
    let mut app = local_app();
    app.switch_site(0);
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let html = r#"<a href="/x/res/1.html#1" class="quotelink">&gt;&gt;1</a> <a href="/xy/res/9.html#10">&gt;&gt;&gt;/xy/10</a> see https://example.com/a"#;
    let parsed = crate::markup::parse_html(html, crate::markup::Flavor::Vichan);
    let reply = Post { no: 2, files: vec![Attachment { filename: "a.png".into(), ..Attachment::at("http://127.0.0.1:3/x/src/a.png") }], ..parsed.into() };
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }, reply]));
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.tab.navigate(View::Thread);
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
    assert_eq!((app.tab.board.as_ref().unwrap().uri.as_str(), app.tab.pending_thread.unwrap(), app.tab.opening().select), ("xy", 9, Some(10)));
    assert_eq!(app.tab.trail.len(), 1);
    // A post without links says so.
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }]));
    app.act(Action::Links);
    assert!(!matches!(app.tab.popup, Some(crate::app::TabPopup::Links(_))) && app.footer.get().unwrap().text == "Post has no links");
}

#[test]
fn filters_and_hiding() {
    let mut app = local_app();
    let cfg = "[[filter]]\npattern = \"(?i)spam\"\nlabel = \"spam\"\n[[filter]]\npattern = \"rust\"\naction = \"highlight\"";
    app.rehide(|a| a.hiding.set_filters(crate::filter::tests::filters(cfg).unwrap()));
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.catalog_board = Some("x".into());
    let op = |no, subject: &str| Post { no, subject: Some(subject.into()), ..Default::default() };
    app.tab.catalog = vec![op(1, "SPAM here"), op(2, "rust thread"), op(3, "other")];
    app.remark();
    app.tab.navigate(View::Catalog);
    assert_eq!(app.visible_catalog(), [1, 2]);
    assert_eq!(app.tab.catalog_marks.highlight(1), Some("rust"));
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
    assert!(app.footer.get().unwrap().text.contains("filter \"spam\""));
    app.tab.catalog_list.state.select(Some(2));
    app.act(Action::Hide);
    assert!(!app.store.hidden_on("a", "x").contains(&3));
    app.act(Action::ShowHidden);
    // In a thread, hidden posts collapse (never the OP).
    let mut reply = op(11, "");
    reply.body = vec![Line::raw("buy spam")];
    app.set_thread(vec![op(10, "spam OP"), reply, op(12, "")]);
    app.tab.navigate(View::Thread);
    let t = app.tab.thread.as_ref().unwrap();
    assert!(!t.is_collapsed(0) && t.is_collapsed(1) && !t.is_collapsed(2));
    app.tab.thread.as_mut().unwrap().selected = 2;
    app.act(Action::Hide);
    assert!(app.tab.thread.as_ref().unwrap().is_collapsed(2));
    app.act(Action::ShowHidden);
    assert!(!app.tab.thread.as_ref().unwrap().is_collapsed(1));
}

#[test]
fn hidden_posts_are_not_new_in_watched_threads() {
    let mut app = local_app();
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    let post = |no, quotes: Vec<u64>, text: &str| Post { no, quotes, body: vec![Line::raw(text.to_string())], ..Default::default() };
    app.rehide(|a| a.hiding.set_filters(crate::filter::tests::filters("[[filter]]\npattern = \"spam\"\n\n[[filter]]\npattern = \"rust\"\naction = \"highlight\"\nnotify = true\n").unwrap()));
    app.rehide(|a| a.hiding.set_recursive(true));
    app.store.watch(key.clone(), "One".into(), 2, 5);
    app.store.toggle_mine(&key, 5).unchecked();
    let start = vec![post(1, vec![], ""), post(5, vec![], "")];
    refresh(&mut app, key.clone(), start.clone());
    // A hidden reply to yours, a reply to that (hidden with it), and one you can see.
    let mut posts = start;
    posts.extend([post(6, vec![5], "buy spam"), post(7, vec![6, 5], "agreed"), post(8, vec![5], "hello")]);
    refresh(&mut app, key.clone(), posts.clone());
    app.flush_notes(Instant::now());
    assert_eq!(app.notified, ["New reply to your post in /x/ One"]);
    assert_eq!(app.store.watched(&key).unwrap().status, Status::Live { unread: 1, replies: 1 });
    // A `notify` filter still tells about what it catches, hidden or not.
    app.notified.clear();
    posts.push(post(9, vec![], "rust spam"));
    refresh(&mut app, key.clone(), posts);
    app.flush_notes(Instant::now());
    assert_eq!(app.notified, ["A new post caught by \"rust\" in /x/ One"]);
    assert_eq!(app.store.watched(&key).unwrap().status.counts().0, 1);
}

#[test]
fn notifies_about_new_posts_and_replies_to_yours() {
    let mut app = local_app();
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    let post = |no, quotes: Vec<u64>| Post { no, quotes, ..Default::default() };
    app.store.watch(key(1), "One".into(), 2, 5);
    app.store.watch(key(2), "Two".into(), 1, 20);
    app.store.toggle_mine(&key(1), 5).unchecked();
    // The first refresh of the session tells nothing.
    refresh(&mut app, key(1), vec![post(1, vec![]), post(5, vec![]), post(6, vec![5])]);
    app.flush_notes(Instant::now());
    assert!(app.notified.is_empty());
    assert_eq!(app.store.watched(&key(1)).unwrap().status.counts().1, 1);
    // Then: a reply to your post, and another post.
    refresh(&mut app, key(1), vec![post(1, vec![]), post(5, vec![]), post(6, vec![5]), post(7, vec![5]), post(8, vec![1])]);
    app.flush_notes(Instant::now());
    assert_eq!(app.notified, ["New reply to your post in /x/ One", "1 new post in /x/ One"]);
    assert_eq!(app.store.watched(&key(1)).unwrap().status.counts().1, 2);
    // Several threads at once make one notification.
    app.notified.clear();
    refresh(&mut app, key(2), vec![post(2, vec![]), post(20, vec![])]);
    refresh(&mut app, key(1), vec![post(1, vec![]), post(5, vec![]), post(6, vec![5]), post(7, vec![5]), post(8, vec![1]), post(9, vec![])]);
    refresh(&mut app, key(2), vec![post(2, vec![]), post(20, vec![]), post(21, vec![])]);
    // ...once nothing is still refreshing.
    app.refreshing.insert(key(3));
    app.flush_notes(Instant::now());
    assert!(app.notified.is_empty());
    app.refreshing.clear();
    app.flush_notes(Instant::now());
    assert_eq!(app.notified, ["2 watched threads have new posts"]);
}

#[test]
fn notify_filters_tell_about_what_they_catch_once() {
    let mut app = local_app();
    let cfg = "[[filter]]\npattern = \"^Ab3d$\"\nfield = \"id\"\naction = \"highlight\"\nnotify = true\nlabel = \"that guy\"\n\
               [[filter]]\npattern = \"(?i)/lmg/\"\nfield = \"subject\"\nop = true\naction = \"highlight\"\nnotify = true\nlabel = \"lmg\"";
    app.rehide(|a| a.hiding.set_filters(crate::filter::tests::filters(cfg).unwrap()));
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    let post = |no, id: &str| Post { no, id: Some(id.into()), ..Default::default() };
    app.store.watch(key(1), "One".into(), 1, 1);
    // The first refresh of the session tells nothing, though it catches one.
    refresh(&mut app, key(1), vec![post(1, "x"), post(2, "Ab3d")]);
    app.flush_notes(Instant::now());
    assert!(app.notified.is_empty());
    // A new post it catches: told, once.
    refresh(&mut app, key(1), vec![post(1, "x"), post(2, "Ab3d"), post(3, "Ab3d"), post(4, "zz")]);
    app.flush_notes(Instant::now());
    assert_eq!(app.notified, ["A new post caught by \"that guy\" in /x/ One", "2 new posts in /x/ One"]);
    app.notified.clear();
    refresh(&mut app, key(1), vec![post(1, "x"), post(2, "Ab3d"), post(3, "Ab3d"), post(4, "zz")]);
    app.flush_notes(Instant::now());
    assert!(app.notified.is_empty());
    // A followed general's board: new threads it catches, after the first look.
    let op = |no, subject: &str| Post { no, subject: Some(subject.into()), ..Default::default() };
    app.general_catalog(&key(1), Ok(vec![op(10, "/lmg/ old"), op(11, "other")]));
    app.flush_notes(Instant::now());
    assert!(app.notified.is_empty());
    app.general_catalog(&key(1), Ok(vec![op(10, "/lmg/ old"), op(12, "/LMG/ - Local Models General"), op(13, "other")]));
    app.general_catalog(&key(1), Ok(vec![op(12, "/LMG/ - Local Models General")]));
    app.flush_notes(Instant::now());
    assert_eq!(app.notified, ["A new post caught by \"lmg\" in /x/ /LMG/ - Local Models General"]);
}

#[test]
fn top_filters_put_highlighted_threads_first() {
    let mut app = local_app();
    let cfg = "[[filter]]\npattern = \"rust\"\naction = \"highlight\"\ntop = true\n[[filter]]\npattern = \"go\"\naction = \"highlight\"";
    app.rehide(|a| a.hiding.set_filters(crate::filter::tests::filters(cfg).unwrap()));
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.catalog_board = Some("x".into());
    let op = |no, subject: &str, replies| Post { no, subject: Some(subject.into()), replies: Some(replies), time: no as i64, ..Default::default() };
    app.tab.catalog = vec![op(1, "go", 5), op(2, "rust 1", 1), op(3, "c", 9), op(4, "rust 2", 3)];
    app.remark();
    app.tab.navigate(View::Catalog);
    // Bump order, with the top ones first (in that order).
    assert_eq!(app.visible_catalog(), [1, 3, 0, 2]);
    app.tab.catalog_sort = crate::app::Sort::Replies;
    assert_eq!(app.visible_catalog(), [3, 1, 2, 0]);
    app.tab.catalog_sort = crate::app::Sort::Newest;
    assert_eq!(app.visible_catalog(), [3, 1, 2, 0]);
}

#[test]
fn watched_threads_first_after_top_ones() {
    let mut app = local_app();
    app.rehide(|a| a.hiding.set_filters(crate::filter::tests::filters("[[filter]]\npattern = \"rust\"\naction = \"highlight\"\ntop = true").unwrap()));
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.catalog_board = Some("x".into());
    let op = |no, subject: &str, replies| Post { no, subject: Some(subject.into()), replies: Some(replies), time: no as i64, ..Default::default() };
    app.tab.catalog = vec![op(1, "go", 5), op(2, "rust 1", 1), op(3, "c", 9), op(4, "rust 2", 3), op(5, "d", 7)];
    app.remark();
    app.tab.navigate(View::Catalog);
    let key = |site: &str, board: &str, no| ThreadKey { site: site.into(), board: board.into(), no };
    // Watched: 3 and 4 here; 1 on another board, 5 on another site.
    for k in [key("a", "x", 3), key("a", "x", 4), key("a", "xy", 1), key("b", "x", 5)] {
        app.store.watch(k, String::new(), 0, 0);
    }
    // Off: the sort, with the top ones first.
    assert!(!app.watched_first);
    assert_eq!(app.visible_catalog(), [1, 3, 0, 2, 4]);
    // On (Settings, saved): top ones, then the watched ones, then the rest, each in the
    // sort's order, watched ones first among the top ones too.
    let dir = tempfile::tempdir().unwrap();
    app.config_path = Some(dir.path().join("config.toml"));
    app.tab.navigate(View::Settings);
    app.settings_list.state.select(settings::position("Watched first"));
    app.enter();
    assert!(app.watched_first);
    assert!(std::fs::read_to_string(dir.path().join("config.toml")).unwrap().contains("watched_first = true"));
    app.tab.navigate(View::Catalog);
    assert_eq!(app.visible_catalog(), [3, 1, 2, 0, 4]);
    app.tab.catalog_sort = crate::app::Sort::Replies;
    assert_eq!(app.visible_catalog(), [3, 1, 2, 4, 0]);
    assert!(app.catalog_watching(&app.tab.catalog[2]) && !app.catalog_watching(&app.tab.catalog[0]));
    // A list filter still filters.
    app.tab.catalog_list.filter = "rust".into();
    assert_eq!(app.visible_catalog(), [3, 1]);
}

#[test]
fn marking_posts_as_yours_watches_the_thread() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }, Post { no: 2, ..Default::default() }]));
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.tab.navigate(View::Thread);
    app.act(Action::Mine);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    assert_eq!(app.store.watched(&key).unwrap().mine(), [2]);
    assert!(app.tab.thread.as_ref().unwrap().marks.is_mine(2));
    app.act(Action::Mine);
    assert!(app.store.watched(&key).unwrap().mine().is_empty());
}

#[test]
fn catalogs_mark_new_threads_and_replies() {
    let mut app = local_app();
    app.clock = Clock { fixed: Some(1000), ..Default::default() };
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let op = |no, replies| Post { no, replies: Some(replies), ..Default::default() };
    app.load_catalog(None);
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(vec![op(1, 3), op(2, 0)])));
    assert!(app.tab.catalog_new.is_empty());
    // Thread 1 is opened with 3 replies.
    app.set_thread(vec![Post { no: 1, ..Default::default() }, Post { no: 5, ..Default::default() }, Post { no: 6, ..Default::default() }, Post { no: 7, ..Default::default() }]);
    app.load_catalog(None);
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(vec![op(9, 0), op(1, 8), op(2, 1)])));
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
    app.tab.navigate(View::Thread);
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
    app.tab.navigate(View::Catalog);
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
    assert_eq!(app.tab.view(), View::Boards);
    // Clicks hit the right card.
    app.tab.navigate(View::Catalog);
    app.begin_frame().body = Some(Hit::Grid { area: Rect::new(2, 2, 66, 24), offset: 0, cols: 3, cell: (22, 12) });
    app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2 + 22 + 5, 2 + 12 + 3), Instant::now());
    assert_eq!(at(&app), 4);
    // c cycles this board's layout.
    app.act(Action::Compact);
    assert_eq!(app.layout(), CatalogLayout::Cards);
}

#[test]
fn the_gallery_leaves_out_hidden_posts() {
    let mut app = local_app();
    app.images = crate::images::Images::offline();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let file = |name: &str| Attachment { filename: name.into(), ..Attachment::at(format!("http://127.0.0.1:3/x/src/{name}")) };
    let post = |no, files: Vec<Attachment>| Post { no, files, ..Default::default() };
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![post(1, vec![file("a.png")]), post(2, vec![file("hidden.png")]), post(3, vec![file("b.jpg")])]));
    app.tab.navigate(View::Thread);
    let site = app.current_site().cfg.name.clone();
    app.rehide(|a| a.store.toggle_hidden(&site, "x", 2));
    let names = |app: &App| app.tab.gallery.as_ref().unwrap().files.iter().map(|(_, f)| f.filename.clone()).collect::<Vec<_>>();
    app.act(Action::Gallery);
    assert_eq!(names(&app), ["a.png", "b.jpg"]);
    // Z shows it again, and its file.
    app.tab.gallery = None;
    app.act(Action::ShowHidden);
    app.act(Action::Gallery);
    assert_eq!(names(&app), ["a.png", "hidden.png", "b.jpg"]);
}

#[test]
fn searching_a_thread_passes_hidden_posts_over() {
    let mut app = local_app();
    let post = |no, text: &str| Post { no, body: vec![Line::raw(text.to_string())], ..Default::default() };
    app.hidden_words = vec!["crypto".into()];
    app.rehide(|a| a.hiding.set_filters(crate::filter::Filters::new(&[]).unwrap().with_words(&a.hidden_words).unwrap()));
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![post(1, "a thread"), post(2, "buy crypto"), post(3, "crypto is bad, says a post you can read")]));
    app.tab.navigate(View::Thread);
    let site = app.current_site().cfg.name.clone();
    app.rehide(|a| a.store.toggle_hidden(&site, "x", 3));
    let matches = |app: &App| app.tab.thread.as_ref().unwrap().matches.clone();
    app.tab.thread.as_mut().unwrap().set_search("crypto".into());
    assert!(matches(&app).is_empty());
    // Z shows them, and they're found; hidden again, they aren't.
    app.act(Action::ShowHidden);
    assert_eq!(matches(&app), [1, 2]);
    app.act(Action::ShowHidden);
    assert!(matches(&app).is_empty());
}

#[test]
fn hiding_marks_every_tab_again() {
    let mut app = local_app();
    let post = |no, text: &str| Post { no, body: vec![Line::raw(text.to_string())], ..Default::default() };
    let thread = || ThreadView::new("x".into(), 1, vec![post(1, "a thread"), post(2, "buy crypto"), post(3, "a reply")]);
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.thread = Some(thread());
    app.tab.navigate(View::Thread);
    // Tab 1: the same thread, and the board's catalog.
    app.tabs.push(Tab::new(0, Instant::now()));
    app.switch_tab(1);
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.thread = Some(thread());
    app.tab.catalog = vec![post(1, "a thread"), post(7, "crypto general")];
    app.tab.catalog_board = Some("x".into());
    app.tab.navigate(View::Catalog);
    app.remark();
    app.switch_tab(0);
    let hidden = |app: &mut App| {
        app.switch_tab(1);
        let t = app.tab.thread.as_ref().unwrap();
        let marks = (t.marks.all_hidden().iter().map(Option::is_some).collect::<Vec<_>>(), t.marks.show_hidden());
        let catalog = app.tab.catalog_marks.all_hidden().iter().map(Option::is_some).collect::<Vec<_>>();
        app.switch_tab(0);
        (marks, catalog)
    };
    // A hidden word added in tab 0 hides in tab 1 too.
    app.hidden_words = vec!["crypto".into()];
    app.apply_filters();
    assert_eq!(hidden(&mut app), ((vec![false, true, false], false), vec![false, true]));
    // So does H, and Z shows them there too.
    app.tab.thread.as_mut().unwrap().selected = 2;
    app.act(Action::Hide);
    assert_eq!(hidden(&mut app), ((vec![false, true, true], false), vec![false, true]));
    app.act(Action::ShowHidden);
    assert_eq!(hidden(&mut app).0, (vec![false, true, true], true));
}

#[test]
fn gallery_of_the_threads_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.download_dir = Some(dir.path().display().to_string());
    app.images = crate::images::Images::offline();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let file = |name: &str| Attachment { filename: name.into(), ..Attachment::at(format!("http://127.0.0.1:3/x/src/{name}")) };
    let post = |no, files: Vec<Attachment>| Post { no, files, ..Default::default() };
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![post(1, vec![file("a.png")]), post(2, vec![]), post(3, vec![file("b.jpg"), file("c.gif")])]));
    app.tab.navigate(View::Thread);
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
    // The wheel moves through the grid a row at a time, not the thread behind it.
    let scroll = app.tab.thread.as_ref().unwrap().scroll;
    app.on_mouse(mouse(MouseEventKind::ScrollUp, 5, 5), Instant::now());
    assert_eq!(app.tab.gallery.as_ref().unwrap().state.selected(), Some(0));
    app.on_mouse(mouse(MouseEventKind::ScrollDown, 5, 5), Instant::now());
    assert_eq!(app.tab.gallery.as_ref().unwrap().state.selected(), Some(2));
    app.on_mouse(mouse(MouseEventKind::ScrollUp, 5, 5), Instant::now());
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(app.tab.thread.as_ref().unwrap().scroll, scroll);
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
    app.tab.navigate(View::Catalog);
    app.act(Action::ArchiveSearch);
    for c in "borrow".chars() {
        app.on_key(KeyEvent::from(KeyCode::Char(c)));
    }
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!((app.tab.view(), app.tab.site), (View::Search, 1));
    let v = crate::backend::fixture("foolfuuka_search.json");
    app.handle(answer(app.tab.req().unwrap(), |a, (page, r)| a.search_results(page, r), (1, crate::backend::foolfuuka::parse_search(&v))));
    assert_eq!(app.tab.search.as_ref().unwrap().hits.len(), 4);
    // Going down to the end asks for the next page.
    let req = app.tab.req().unwrap();
    for _ in 0..4 {
        app.on_key(KeyEvent::from(KeyCode::Down));
    }
    assert_eq!(app.tab.req().unwrap(), req + 1);
    app.handle(answer(app.tab.req().unwrap(), |a, (page, r)| a.search_results(page, r), (2, Err(anyhow::anyhow!("You're searching too fast.")))));
    assert!(app.footer.get().is_some_and(|s| s.error && s.text.contains("too fast")));
    // Enter: the thread, on the archive, with the post selected.
    app.tab.search_list.state.select(Some(1));
    app.enter();
    assert_eq!((app.tab.view(), app.tab.pending_thread.unwrap(), app.tab.opening().select), (View::Thread, 109912686, Some(109914413)));
    app.back();
    assert_eq!(app.tab.view(), View::Search);
    app.back();
    assert_eq!((app.tab.view(), app.tab.site, app.tab.search.is_none()), (View::Catalog, 0, true));
    // Sites without an archive say so.
    drawn(&mut app);
    app.sites[0].cfg.archive = None;
    app.act(Action::ArchiveSearch);
    assert!(app.typing.is_none() && app.footer.get().unwrap().text.contains("no archive"));
}

#[test]
fn search_results_leave_out_hidden_posts() {
    let mut app = app_with(
        "[[site]]\nname = \"chan\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"g\"]\narchive = \"arch\"\n\
         [[site]]\nname = \"arch\"\nkind = \"foolfuuka\"\nurl = \"http://localhost:3\"\nboards = [\"g\"]",
    );
    app.tab.board = Some(Board { uri: "g".into(), title: String::new(), nsfw: None });
    app.tab.navigate(View::Catalog);
    app.act(Action::ArchiveSearch);
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let page = crate::backend::foolfuuka::parse_search(&crate::backend::fixture("foolfuuka_search.json")).unwrap();
    let nos: Vec<u64> = page.hits.iter().map(|(_, p)| p.no).collect();
    // One hidden by hand, one by a hidden word (the longest word in it, and in no other).
    app.rehide(|a| a.store.toggle_hidden("arch", "g", nos[0]));
    let text = |k: usize| page.hits[k].1.plain_text().to_lowercase();
    let word = text(2)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| (0..4).all(|k| k == 2 || !text(k).contains(w)))
        .max_by_key(|w| w.len())
        .unwrap()
        .to_string();
    app.hidden_words = vec![word];
    app.rehide(|a| a.hiding.set_filters(crate::filter::Filters::new(&[]).unwrap().with_words(&a.hidden_words).unwrap()));
    app.handle(answer(app.tab.req().unwrap(), |a, (page, r)| a.search_results(page, r), (1, Ok(page))));
    let shown = |app: &App| app.visible_hits().iter().map(|&k| app.tab.search.as_ref().unwrap().hits[k].1.no).collect::<Vec<_>>();
    assert_eq!(shown(&app), [nos[1], nos[3]]);
    // Enter opens the one selected among those shown.
    app.tab.search_list.state.select(Some(1));
    app.enter();
    assert_eq!(app.tab.opening().select, Some(nos[3]));
    app.back();
    // Z shows them all; unhiding by hand marks the results again.
    app.act(Action::ShowHidden);
    assert_eq!(shown(&app), nos);
    app.act(Action::ShowHidden);
    app.rehide(|a| a.store.toggle_hidden("arch", "g", nos[0]));
    assert_eq!(shown(&app), [nos[0], nos[1], nos[3]]);
}

#[test]
fn a_file_with_only_its_thumbnail_is_never_taken_for_the_file() {
    let mut app = local_app();
    app.images = crate::images::Images::offline();
    let thumb = "https://i.example/1s.jpg";
    let file = Attachment { filename: "clip.webm".into(), url: None, kind: FileKind::Video, thumb: Some(thumb.into()), ..Default::default() };
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, files: vec![file.clone()], ..Default::default() }]));
    app.tab.navigate(View::Thread);
    app.act(Action::View);
    // Shown as the thumbnail it is; nothing to save; o, y and i act on the thumbnail and say so.
    assert_eq!(app.viewer_source(&file), Some((thumb.into(), crate::images::Kind::Thumb)));
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    assert_eq!(app.status().unwrap().text, "Only the thumbnail is available; there's no file to save");
    app.on_key(KeyEvent::from(KeyCode::Char('y')));
    assert_eq!((app.copied.as_deref(), app.status().unwrap().text.as_str()), (Some(thumb), "Copied thumbnail URL: https://i.example/1s.jpg"));
    app.on_key(KeyEvent::from(KeyCode::Char('i')));
    assert_eq!(app.opened.as_deref(), Some(thumb));
    assert!(app.status().unwrap().text.starts_with("Only the thumbnail is available"));
    // The same file focused in the thread: `o` and `d` say so too.
    app.on_key(KeyEvent::from(KeyCode::Esc));
    app.act(Action::NextPart);
    app.opened = None;
    app.act(Action::Browser);
    assert_eq!((app.opened.as_deref(), app.status().unwrap().text.as_str()), (Some(thumb), "Only the thumbnail is available; opened https://i.example/1s.jpg"));
    app.act(Action::Download);
    assert_eq!(app.status().unwrap().text, "Only the thumbnail is available; there's no file to save");
    // The menu offers the thumbnail, and no saving.
    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    let items = &app.menu().unwrap().items;
    let label = |a: Action| items.iter().find_map(|i| if let MenuItem::Act(x, l) = i { (*x == a).then(|| l.to_string()) } else { None });
    assert_eq!(label(Action::Copy).as_deref(), Some("copy the thumbnail's URL"));
    assert!([Action::Download, Action::DownloadPost, Action::DownloadThread].iter().all(|&a| label(a).is_none()), "{items:?}");
    app.popup = None;
    // A spoilered file the archive didn't keep has neither: everything says so, and `o`
    // and `y` don't fall back to the post.
    let gone = Attachment { thumb: None, spoiler: true, ..file };
    app.tab.thread.as_mut().unwrap().posts[0].files = vec![gone.clone()];
    let neither = "Neither the file nor its thumbnail is available";
    app.opened = None;
    for a in [Action::Browser, Action::Copy, Action::Download] {
        app.footer = Footer::default();
        app.act(a);
        assert_eq!(app.status().map(|s| s.text.as_str()), Some(neither), "{a:?}");
    }
    app.open_file(&gone);
    assert_eq!((app.opened.as_deref(), app.status().map(|s| s.text.as_str())), (None, Some(neither)));
}

#[test]
fn reverse_image_search() {
    let mut app = local_app();
    app.images = crate::images::Images::offline();
    let file = |name: &str, thumb| Attachment { filename: name.into(), thumb, ..Attachment::at(format!("https://i.example/{name}")) };
    let files = vec![file("a.png", None), file("b.webm", Some("https://i.example/bs.jpg".into())), file("c.pdf", None)];
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, files, ..Default::default() }]));
    app.tab.navigate(View::Thread);
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
fn a_session_not_restored_is_left_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("session.json");
    let mut app = saving_app(dir.path(), 1000);
    app.goto_str("a/x");
    app.save_session(None);
    let before = std::fs::read(&file).unwrap();
    // A run with restore_session off saves everything else, never the session.
    let mut next = saving_app(dir.path(), 1000);
    next.restore_session = false;
    next.goto_str("b/y/5");
    next.save_session(None);
    next.store.save().unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), before);
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
    let saved = app.store.session.clone();
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
    assert_eq!((next.tab.site, next.tab.view(), next.tab.pending_thread.unwrap(), next.tab.opening().select, next.tab.catalog_sort), (1, View::Thread, 5, Some(6), Sort::Newest));
    // If the thread is gone, its catalog instead.
    next.handle(answer(next.tab.req().unwrap(), thread_arrived, Err(anyhow::Error::new(http::HttpError::NotFound("x".into())))));
    assert_eq!(next.tab.view(), View::Catalog);
    // A catalog with its selected thread.
    next.handle(answer(next.tab.req().unwrap(), App::catalog_arrived, Ok(vec![])));
    next.tab.catalog = (1..4).map(|no| Post { no, ..Default::default() }).collect();
    next.tab.catalog_list.state.select(Some(2));
    // (The board has no sort of its own, so its catalog is in bump order: index 2 is thread 3.)
    let place = next.place();
    assert_eq!((place.view.as_str(), place.selected), ("catalog", Some(3)));
    let mut third = local_app();
    third.go_to_place(&place);
    third.handle(answer(third.tab.req().unwrap(), App::catalog_arrived, Ok((1..4).map(|no| Post { no, time: no as i64, ..Default::default() }).collect())));
    assert_eq!(third.selected_index().map(|i| third.tab.catalog[i].no), Some(3));
}

#[test]
fn tabs_keep_their_own_place_and_responses() {
    let mut app = local_app();
    // Tab 0 loads a catalog on site a.
    app.goto_str("a/x");
    let first_req = app.tab.req().unwrap();
    // Tab 1 opens a thread on site b while that's still loading.
    app.tabs.push(Tab::new(0, Instant::now()));
    app.switch_tab(1);
    assert_eq!((app.tab.view(), app.tab.thread.is_none()), (View::Sites, true));
    app.goto_str("b/y/5");
    assert_eq!((app.tab.site, app.tab.view(), app.tab.pending_thread.unwrap()), (1, View::Thread, 5));
    // Tab 0's catalog arrives: it goes to tab 0, not here.
    app.handle(answer(first_req, App::catalog_arrived, Ok(vec![Post { no: 1, ..Default::default() }])));
    assert!(app.tab.catalog.is_empty());
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(vec![Post { no: 5, ..Default::default() }])));
    assert_eq!(app.tab.thread.as_ref().unwrap().no, 5);
    app.switch_tab(0);
    assert_eq!((app.tab.site, app.tab.view(), app.tab.catalog.len(), app.tab.loading().is_none()), (0, View::Catalog, 1, true));
    assert!(app.tab.thread.is_none());
    // A thread with no posts at all is an error, and what was shown stays.
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(vec![])));
    app.goto_str("a/x/1");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(vec![Post { no: 1, ..Default::default() }])));
    app.act(Action::Reload);
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, Thread::answer(1, vec![])));
    assert!(app.tab.thread.as_ref().is_some_and(|t| t.posts.len() == 1) && app.footer.get().is_some_and(|s| s.error));
    // A post's thread found while the settings are open opens behind them.
    app.goto_str("a/x/1#77");
    app.act(Action::Settings);
    app.handle(answer(app.tab.req().unwrap(), |a, (b, post, r)| a.thread_found(0, b, post, None, r), (Board { uri: "x".into(), title: String::new(), nsfw: None }, 77, Ok(Some(3)))));
    assert_eq!((app.tab.view(), app.tab.place_view(), app.tab.pending_thread.unwrap()), (View::Settings, View::Thread, 3));
    // Tab chips don't switch tabs under a settings popup (it isn't the tab's).
    app.tabs.push(Tab::new(0, Instant::now()));
    app.popup = Some(Popup::Settings(SettingsPopup::Folder { value: String::new() }));
    app.begin_frame().tabs = vec![(Rect::new(0, 0, 5, 1), 1)];
    app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 1, row: 0, modifiers: KeyModifiers::NONE }, Instant::now());
    assert_eq!((app.active, app.tab.view()), (0, View::Settings));
    app.popup = None;
    app.tabs.pop();
    app.tab.navigate(View::Catalog);
    // A response for a tab that's gone is dropped.
    let stale = app.tabs[1].req().unwrap();
    app.tabs.truncate(1);
    app.handle(answer(stale, thread_arrived, arrived(vec![Post { no: 9, ..Default::default() }])));
    assert!(app.tab.thread.is_none());
}

#[test]
fn new_tabs_switching_closing_and_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.store = Store::load(Some(dir.path().to_path_buf())).0;
    app.goto_str("a/x");
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok((1..=3).map(|no| Post { no, ..Default::default() }).collect())));
    app.tab.catalog_list.state.select(Some(1));
    let key = |c| KeyEvent::from(KeyCode::Char(c));
    // T: thread 2 in a new tab after this one.
    app.on_key(key('T'));
    assert_eq!((app.tabs.len(), app.active, app.tab.view(), app.tab.pending_thread.unwrap()), (2, 1, View::Thread, 2));
    assert_eq!(app.tab_label(0), "/x/");
    // ] / [ switch; each tab keeps its place.
    app.on_key(key(']'));
    assert_eq!((app.active, app.tab.view(), app.tab.catalog.len()), (0, View::Catalog, 3));
    app.on_key(key('['));
    assert_eq!((app.active, app.tab.view()), (1, View::Thread));
    // The session has both.
    app.save_session(None);
    let s = app.store.session.clone();
    assert_eq!((s.tabs.len(), s.active, s.tabs[1].thread), (2, 1, Some(2)));
    // ctrl-w closes; the last tab stays.
    app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    assert_eq!((app.tabs.len(), app.active, app.tab.view()), (1, 0, View::Catalog));
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
    assert_eq!((next.tabs.len(), next.active, next.tab.view(), next.tab.pending_thread.unwrap()), (2, 1, View::Thread, 2));
    next.switch_tab(0);
    assert_eq!((next.tab.view(), next.tab.board.as_ref().unwrap().uri.as_str()), (View::Catalog, "x"));
}

#[test]
fn favorite_boards_on_the_home_screen() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.config_path = Some(dir.path().join("config.toml"));
    // * in Boards on the selected board, and in a catalog on its board.
    app.switch_site(1);
    app.tab.navigate(View::Boards);
    app.tab.board_list.state.select(Some(0));
    app.act(Action::Favorite);
    app.goto_str("a/xy");
    app.act(Action::Favorite);
    assert_eq!(app.favorites.iter().map(BoardRef::key).collect::<Vec<_>>(), ["b/y", "a/xy"]);
    let c: Config = toml::from_str(&std::fs::read_to_string(dir.path().join("config.toml")).unwrap()).unwrap();
    assert_eq!(c.favorites, ["b/y", "a/xy"]);
    // They're on the home screen after Watched, History and Saved; 2 opens the second.
    app.tab.navigate(View::Sites);
    assert_eq!(app.visible_sites()[3..5], [SiteRow::Favorite(0), SiteRow::Favorite(1)]);
    app.on_key(KeyEvent::from(KeyCode::Char('1')));
    assert_eq!((app.tab.site, app.tab.view(), app.tab.board.as_ref().unwrap().uri.as_str()), (1, View::Catalog, "y"));
    app.tab.navigate(View::Sites);
    app.on_key(KeyEvent::from(KeyCode::Char('2')));
    assert_eq!((app.tab.site, app.tab.board.as_ref().unwrap().uri.as_str()), (0, "xy"));
    // x on a favorite row takes it off; * again on the board does too.
    app.tab.navigate(View::Sites);
    app.site_list.state.select(Some(3));
    app.act(Action::Remove);
    assert_eq!(app.favorites.len(), 1);
    app.goto_str("a/xy");
    app.act(Action::Favorite);
    assert!(app.favorites.is_empty());
    app.tab.navigate(View::Sites);
    app.on_key(KeyEvent::from(KeyCode::Char('3')));
    assert!(app.footer.get().unwrap().text.contains("No favorites yet"));
}

#[test]
fn recent_boards_on_the_home_screen() {
    let mut app = local_app();
    for board in ["a/x", "b/y", "a/xy"] {
        app.goto_str(board);
        app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(vec![])));
    }
    assert_eq!(app.store.recent_boards, ["a/xy", "b/y", "a/x"]);
    // Favorites aren't repeated as recent.
    app.favorites.push(BoardRef::parse("b/y").unwrap());
    app.tab.navigate(View::Sites);
    let rows = app.visible_sites();
    assert_eq!(rows[3..6], [SiteRow::Favorite(0), SiteRow::Recent(0), SiteRow::Recent(2)]);
    // Enter opens; x forgets it.
    app.site_list.state.select(Some(5));
    app.enter();
    assert_eq!((app.tab.view(), app.tab.board.as_ref().unwrap().uri.as_str()), (View::Catalog, "x"));
    app.tab.navigate(View::Sites);
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
    app.tab.navigate(View::Thread);
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
    app.tab.navigate(View::Sites);
    refresh(&mut app, key(10), vec![full, Post { no: 11, ..Default::default() }]);
    assert!(app.store.watched(&key(10)).unwrap().at_limit);
    app.check_generals(now);
    app.check_generals(now);
    assert_eq!(app.generals_searching.len(), 1);
    // No new thread yet: tried again only after a while.
    app.general_catalog(&key(10), Ok(vec![op(10, "/lmg/ - Local Models General #5"), op(12, "/ldg/ - Local Diffusion")]));
    app.check_generals(now + Duration::from_secs(60));
    assert!(app.generals_searching.is_empty());
    // The next one appears: it's watched and followed; the old one (still going) is kept.
    app.check_generals(now + Duration::from_secs(601));
    app.general_catalog(&key(10), Ok(vec![op(9, "/lmg/ old"), op(13, "/lmg/ - Local Models General #6"), op(14, "/ldg/")]));
    assert_eq!(app.store.watched(&key(13)).unwrap().general.as_deref(), Some("/lmg/"));
    assert_eq!(app.store.watched(&key(10)).unwrap().general, None);
    assert!(app.notified.last().unwrap().starts_with("New /lmg/ thread on /x/"));
    // When the followed thread dies, it's replaced in Watched.
    app.store.watched_mut(&key(13)).unwrap().status = Status::Dead;
    app.check_generals(now + Duration::from_secs(1200));
    app.general_catalog(&key(13), Ok(vec![op(20, "/lmg/ - Local Models General #7")]));
    assert!(app.store.watched(&key(13)).is_none());
    assert!(app.store.watched(&key(20)).is_some());
    // F again stops following.
    app.tab.navigate(View::Watched);
    let i = app.store.all_watched().iter().position(|w| w.key == key(20)).unwrap();
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
    app.tab.navigate(View::Thread);
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
    app.footer.tick(t0, true);
    app.footer.tick(t0 + Duration::from_millis(1500), true);
    assert!(app.footer.get().is_some());
    app.footer.tick(t0 + Duration::from_secs(2), true);
    assert!(app.footer.get().is_none());

    // Errors stay longer, and a new message restarts the timer.
    app.error("Rate limited");
    app.footer.tick(t0, true);
    app.footer.tick(t0 + Duration::from_secs(4), true);
    assert!(app.footer.get().is_some());
    app.error("Thread was deleted or archived");
    app.footer.tick(t0 + Duration::from_secs(4), true);
    app.footer.tick(t0 + Duration::from_secs(8), true);
    assert!(app.footer.get().is_some());
    app.footer.tick(t0 + Duration::from_secs(9), true);
    assert!(app.footer.get().is_none());
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
    let file = |name: &str| Attachment { filename: name.into(), ..Attachment::at(format!("http://127.0.0.1:3/x/src/{name}")) };
    let reply = Post { no: 2, files: vec![file("a.png"), file("b.webm")], ..crate::markup::parse_html(html, crate::markup::Flavor::Vichan).into() };
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(vec![Post { no: 1, ..Default::default() }, reply, Post { no: 3, ..Default::default() }])));
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
    assert_eq!((selected(&app), focus(&app), app.tab.view()), (1, None, View::Thread));
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
    assert_eq!(app.footer.get().map(|s| s.text.as_str()), Some("No more images or links below"));
}

#[test]
fn the_menu_runs_what_it_lists() {
    let mut app = local_app();
    app.goto_str("a/x/1");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(vec![Post { no: 1, files: vec![Attachment { filename: "a.png".into(), ..Attachment::at("http://127.0.0.1:3/a.png") }], ..Default::default() }])));
    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    let m = app.menu().unwrap();
    let has = |a: Action| m.items.iter().any(|i| matches!(i, MenuItem::Act(x, _) if *x == a));
    assert!(has(Action::View) && has(Action::Watch) && has(Action::Gallery) && !has(Action::Preview));
    // A row's own key runs it, and the menu closes.
    app.on_key(KeyEvent::from(KeyCode::Char('w')));
    assert!(app.menu().is_none() && app.footer.get().is_some_and(|s| s.text.starts_with("Watching")));
    // So does enter on a row.
    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    let at = app.menu().unwrap().items.iter().position(|i| matches!(i, MenuItem::Act(Action::Watch, _))).unwrap();
    app.menu_mut().unwrap().list.select(Some(at));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.menu().is_none() && app.footer.get().is_some_and(|s| s.text.starts_with("Stopped watching")));
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
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(nos(&[1, 2]))));
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
    app.store.watch(key(7), "seven".into(), 1, 7);
    refresh(&mut app, key(7), nos(&[7, 8]));
    app.flush_writes();
    assert!(file(7).exists());
    // The index is written with the rest of the data.
    app.save_now();
    assert_eq!(Store::load(Some(dir.path().to_path_buf())).0.saved.len(), 2);
}

#[test]
fn a_search_of_saved_threads_is_stopped_only_by_its_own_tab() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let n = 50;
    for no in 1..=n {
        let key = ThreadKey { site: "a".into(), board: "x".into(), no };
        app.store.keep_thread(&key, "t", "u", &whole(&nos(&[no])), 1000);
    }
    app.flush_writes();
    // Tab 0 searches them; meanwhile tab 1 starts a search of its own and leaves it.
    app.search_saved("post");
    app.tabs.push(Tab::new(0, Instant::now()));
    app.switch_tab(1);
    app.search_saved("post");
    app.search_saved("post");
    app.close_search();
    app.switch_tab(0);
    // Tab 0's search reads every copy.
    let finished = |a: &App| a.tab.search.as_ref().and_then(|s| s.saved.as_ref()).is_some_and(|s| s.finished);
    settle_until(&mut app, finished);
    assert_eq!(app.tab.search.as_ref().unwrap().hits.len(), n as usize);
}

#[test]
fn a_dead_thread_offers_its_saved_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 10_000);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    // A watched thread opens from its saved copy at once, and when it's gone, that's it.
    app.store.watch(key.clone(), "one".into(), 2, 2);
    app.store.keep_thread(&key, "one", "u", &whole(&nos(&[1, 2])), 10_000 - 7200);
    app.goto_str("a/x/1");
    assert_eq!(app.tab.cached(), Some(tabs::Offline { saved: 10_000 - 7200, dead: false }));
    assert!(app.tab.loading().is_some() && app.tab.thread.as_ref().unwrap().posts.len() == 2);
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, Err(gone())));
    assert!(app.store.watched(&key).unwrap().status.is_dead() && app.store.saved(&key).unwrap().dead);
    assert_eq!((app.tab.cached(), app.tab.saved().map(|o| o.dead)), (None, Some(true)));
    // An exported copy of a thread that isn't watched: offered when the thread is gone.
    app.rehide(|a| a.store.unwatch(&key));
    app.goto_str("a/x");
    app.goto_str("a/x/1");
    assert!(app.tab.thread.is_none());
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, Err(gone())));
    let text = &app.footer.get().unwrap().text;
    assert!(text.contains("A saved copy from 2h ago: enter opens it"), "{text}");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.tab.saved(), Some(tabs::Offline { saved: 10_000 - 7200, dead: true }));
    assert_eq!(app.tab.thread.as_ref().unwrap().posts.len(), 2);
    // Read offline: r says so, and nothing is fetched, however long it stays open.
    drawn(&mut app);
    app.act(Action::Reload);
    assert!(app.footer.get().unwrap().text.contains("saved copy from 2h ago; the thread is gone"));
    for _ in 0..3 {
        app.clock = Clock { fixed: Some(app.clock.now() + 600), instant: Some(app.clock.instant() + Duration::from_secs(600)) };
        app.poll();
        assert!(app.tab.loading().is_none() && app.refreshing.is_empty() && app.tab.req().is_none());
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
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(nos(&[1, 2]))));
    app.act(Action::Watch);
    app.refreshed(key, Err(gone()));
    assert_eq!(app.tab.saved(), Some(tabs::Offline { saved: 1000, dead: true }));
    assert!(app.footer.get().unwrap().text.contains("this is its saved copy"));
    assert_eq!(app.tab.thread.as_ref().unwrap().posts.len(), 2);
    // Without a copy, as before.
    let mut app = saving_app(dir.path(), 1000);
    app.goto_str("a/x/5");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, Err(gone())));
    assert!(app.tab.saved_offer.is_none() && app.tab.saved().is_none());
    assert_eq!(app.footer.get().unwrap().text, "Thread was deleted or archived");
}

#[test]
fn a_saved_copy_of_a_live_thread_goes_live_with_r() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.store.keep_thread(&key, "one", "u", &whole(&nos(&[1, 2])), 900);
    app.tab.navigate(View::Saved);
    app.saved_list.state.select(Some(0));
    app.enter();
    assert_eq!(app.tab.saved(), Some(tabs::Offline { saved: 900, dead: false }));
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.act(Action::Reload);
    assert!(app.tab.saved().is_none() && app.tab.loading().is_some());
    // The copy stays up, and the live thread arrives in its place, keeping the selection.
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(nos(&[1, 2, 3]))));
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!((t.posts.len(), t.current().unwrap().no), (3, 2));
    // Back goes to the Saved view.
    app.back();
    assert_eq!(app.tab.view(), View::Saved);
}

#[test]
fn the_saved_view_lists_and_removes_after_asking() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    for (no, at) in [(1, 100), (2, 300), (3, 200)] {
        app.store.keep_thread(&key(no), &format!("thread {no}"), "u", &whole(&nos(&[no])), at);
    }
    app.tab.navigate(View::Sites);
    app.site_list.state.select(Some(2));
    assert_eq!(app.selected_site_row(), Some(SiteRow::Saved));
    app.enter();
    assert_eq!(app.tab.view(), View::Saved);
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
    assert!(app.footer.get().unwrap().text.contains("again to remove the saved copy of thread 3"));
    app.act(Action::Remove);
    app.flush_writes();
    assert!(app.store.saved(&key(3)).is_none() && !dir.path().join("threads/a/x/3.json").exists());
    // Unwatching keeps a copy.
    app.store.watch(key(1), String::new(), 1, 1);
    app.rehide(|a| a.store.unwatch(&key(1)));
    assert!(app.store.saved(&key(1)).is_some());
    // From : too.
    app.tab.navigate(View::Sites);
    app.goto_str("saved");
    assert_eq!(app.tab.view(), View::Saved);
}

#[test]
fn export_saves_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(&dir.path().join("data"), 1000);
    app.download_dir = Some(dir.path().join("dl").display().to_string());
    app.goto_str("a/x/1");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(nos(&[1, 2]))));
    // It asks first; enter saves.
    app.act(Action::Export);
    assert!(!dir.path().join("dl/thread.json").exists());
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(dir.path().join("dl/thread.json").exists());
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    assert_eq!(app.store.saved(&key).unwrap().posts, 2);
    assert!(app.footer.get().unwrap().text.contains("(and in Saved)"));
}

#[test]
fn a_saved_copy_is_remembered_in_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = ThreadKey { site: "b".into(), board: "y".into(), no: 5 };
    app.store.keep_thread(&key, "five", "u", &whole(&nos(&[5, 6])), 900);
    app.open_saved(&key, Opening::default());
    app.tab.thread.as_mut().unwrap().selected = 1;
    let place = app.place();
    assert_eq!((place.view.as_str(), place.thread, place.selected), ("saved", Some(5), Some(6)));
    let mut next = saving_app(dir.path(), 1000);
    next.go_to_place(&place);
    assert!(next.tab.saved().is_some() && next.tab.loading().is_none());
    assert_eq!(next.tab.thread.as_ref().unwrap().current().unwrap().no, 6);
    // The Saved view itself.
    next.tab.navigate(View::Saved);
    assert_eq!(next.place().view, "saved");
}

#[test]
fn watching_a_saved_copy_keeps_it_under_its_own_number() {
    // (Found by fuzzing.) The site answered thread 5 with thread 9's posts; its copy, kept as
    // 5's, shows thread 9. Watching that keeps a copy as 9's too, so 9 dying loses nothing.
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    app.store.keep_thread(&key(5), "five", "u", &whole(&nos(&[9, 10])), 900);
    app.open_saved(&key(5), Opening::default());
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
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(vec![named(1, "Anonymous"), named(2, "Named !Trip"), named(3, "Anonymous"), named(4, "Named !Trip")])));
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
    assert!(t.marks.why_hidden(1).is_some() && t.marks.why_hidden(3).is_some() && t.marks.why_hidden(2).is_none());
    assert!(app.footer.get().unwrap().text.contains("Hiding Named !Trip (2 here) · u undoes"));
    // u takes it back, from the file too.
    app.on_key(KeyEvent::from(KeyCode::Char('u')));
    assert_eq!(config_text(&app), FILTER_CONFIG);
    assert!(app.tab.thread.as_ref().unwrap().marks.why_hidden(1).is_none() && app.filter_cfgs.len() == 1);
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
    assert!(app.goto_text().is_none());
    app.on_key(KeyEvent::from(KeyCode::Enter));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let f = app.filter_cfgs.last().unwrap();
    assert_eq!((f.action, f.sites.len(), f.boards.len(), f.label.as_deref()), (crate::filter::FilterAction::Highlight, 0, 0, Some("him")));
    assert_eq!(app.tab.thread.as_ref().unwrap().marks.highlight(3), Some("him"));
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
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(vec![op(1, "Daily (thread)"), op(2, "Other"), op(3, "Daily (thread)")])));
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
    app.tab.navigate(View::Settings);
    app.settings_list.state.select(settings::position("Filters"));
    app.enter();
    let Some(SettingsPopup::Filters { counts, .. }) = app.settings_popup() else { panic!("no list") };
    assert_eq!(counts, &[(0, 0)]);
    // a: a new one, typing its pattern; a bad regex is refused, and isn't saved.
    app.on_key(KeyEvent::from(KeyCode::Char('a')));
    app.paste("(Named");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.footer.get().unwrap().error && app.filter_cfgs.len() == 1);
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
    assert!(app.tab.thread.as_ref().unwrap().marks.why_hidden(1).is_some());
    // The last field can't go.
    drawn(&mut app);
    app.on_key(KeyEvent::from(KeyCode::Up));
    app.on_key(KeyEvent::from(KeyCode::Down));
    if let Some(SettingsPopup::FilterEdit { row: r, .. }) = app.settings_popup_mut() {
        *r = 5;
    }
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.footer.get().unwrap().text, "A filter needs at least one field");
    // Posts: OPs only (the named posts are replies), replies only, all again.
    if let Some(SettingsPopup::FilterEdit { row: r, .. }) = app.settings_popup_mut() {
        *r = crate::app::EDIT_ROWS.iter().position(|r| *r == crate::app::EditRow::Posts).unwrap();
    }
    let hidden = |app: &App| app.tab.thread.as_ref().unwrap().marks.why_hidden(1).is_some();
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(config_text(&app).contains("op = true") && !hidden(&app));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(config_text(&app).contains("reply = true") && !config_text(&app).contains("op = ") && hidden(&app));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(!config_text(&app).contains("reply = ") && hidden(&app));
    // Back to the list: it counts what it catches; space turns it off (kept in the file).
    app.on_key(KeyEvent::from(KeyCode::Esc));
    let Some(SettingsPopup::Filters { counts, list }) = app.settings_popup() else { panic!("not the list") };
    assert_eq!((counts[1], list.selected()), ((2, 0), Some(1)));
    app.on_key(KeyEvent::from(KeyCode::Char(' ')));
    assert!(config_text(&app).contains("enabled = false"));
    assert!(app.tab.thread.as_ref().unwrap().marks.why_hidden(1).is_none());
    let reloaded: Config = toml::from_str(&config_text(&app)).unwrap();
    assert!(!reloaded.filters[1].enabled && reloaded.filters[0].enabled);
    // x removes it; the first filter and its comments are as they were.
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    assert_eq!(config_text(&app), FILTER_CONFIG);
    // A filter changed in the file meanwhile isn't overwritten.
    std::fs::write(app.config_path.as_ref().unwrap(), FILTER_CONFIG.replace("(?i)spam", "eggs")).unwrap();
    app.on_key(KeyEvent::from(KeyCode::Char(' ')));
    assert!(app.footer.get().unwrap().text.contains("changed since ck read it"));
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
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts.iter().map(post).collect())));
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
    assert!(app.footer.get().unwrap().text.contains("isn't part of a conversation"));
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
fn i_shows_a_posters_posts_until_esc() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.store = Store::load(Some(dir.path().to_path_buf())).0;
    app.goto_str("a/x/1");
    let post = |no: u64, id: Option<&str>| Post { no, id: id.map(String::from), quotes: if no > 1 { vec![no - 1] } else { vec![] }, ..Default::default() };
    let posts = || vec![post(1, Some("aa")), post(2, Some("bb")), post(3, Some("aa")), post(4, None), post(5, Some("bb"))];
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts())));
    let shown = |app: &App| app.tab.thread.as_ref().unwrap().entries.iter().map(|e| app.tab.thread.as_ref().unwrap().posts[e.post].no).collect::<Vec<_>>();
    let t = app.tab.thread.as_mut().unwrap();
    assert_eq!(t.id_count("aa"), 2);
    t.scroll = 5;
    t.select(1);
    app.on_key(KeyEvent::from(KeyCode::Char('I')));
    assert_eq!(shown(&app), [2, 5]);
    assert!(app.footer.get().unwrap().text.contains("2 posts by ID:bb"), "{:?}", app.footer.get());
    // A refresh keeps it, with the poster's new posts.
    let mut more = posts();
    more.push(post(6, Some("bb")));
    more.push(post(7, Some("aa")));
    app.set_thread(more);
    assert_eq!(shown(&app), [2, 5, 6]);
    // It isn't kept in the session (a conversation is).
    app.save_session(None);
    assert_eq!(app.store.session.tabs[0].conversation, None);
    // esc: the whole thread, scrolled where it was.
    app.on_key(KeyEvent::from(KeyCode::Esc));
    let t = app.tab.thread.as_ref().unwrap();
    assert!(t.conversation.is_none() && t.entries.len() == 7 && t.scroll == 5);
    // A post without an ID has no poster to show.
    app.tab.thread.as_mut().unwrap().select(3);
    app.act(Action::Poster);
    assert!(app.tab.thread.as_ref().unwrap().conversation.is_none());
    assert!(app.footer.get().unwrap().text.contains("has no poster ID"));
    // The ID is the post's first part: tab focuses it, enter shows the poster's posts, and
    // again (or I) goes back.
    app.tab.thread.as_mut().unwrap().select(0);
    app.act(Action::NextPart);
    assert_eq!(app.focused(), Some(&Part::Poster));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(shown(&app), [1, 3, 7]);
    app.act(Action::Poster);
    assert_eq!(shown(&app).len(), 7);
    // From a conversation: back is still the whole thread.
    let t = app.tab.thread.as_mut().unwrap();
    t.scroll = 3;
    t.select(2);
    app.act(Action::Conversation);
    run_menu_row(&mut app, "only this poster's posts (ID:aa, 3)");
    assert_eq!(shown(&app), [1, 3, 7]);
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!((app.tab.thread.as_ref().unwrap().scroll, app.tab.thread.as_ref().unwrap().conversation.is_none()), (3, true));
}

#[test]
fn a_conversation_is_remembered_in_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    app.store = Store::load(Some(dir.path().to_path_buf())).0;
    app.goto_str("a/x/1");
    let posts = || vec![Post { no: 1, ..Default::default() }, Post { no: 2, quotes: vec![1], ..Default::default() }, Post { no: 3, quotes: vec![2], ..Default::default() }, Post { no: 4, quotes: vec![1], ..Default::default() }];
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts())));
    app.tab.thread.as_mut().unwrap().select(1);
    app.act(Action::Conversation);
    app.tab.thread.as_mut().unwrap().select(2);
    app.save_session(None);
    let place = app.store.session.clone().tabs[0].clone();
    assert_eq!((place.conversation, place.selected), (Some(2), Some(3)));
    // An old session without it still loads.
    let old: crate::store::Place = serde_json::from_str(r#"{"view":"thread","site":"a","board":"x","thread":1}"#).unwrap();
    assert_eq!(old.conversation, None);
    let mut next = local_app();
    next.go_to_place(&place);
    next.handle(answer(next.tab.req().unwrap(), thread_arrived, arrived(posts())));
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
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(vec![Post { no: 1, ..Default::default() }])));
    app.goto_str("b/g/5");
    app.back();
    assert_eq!((app.tab.view(), app.tab.site, app.tab.catalog_site), (View::Catalog, 1, 1));
    assert!(app.tab.catalog.is_empty() && app.tab.loading().is_some());
}

#[test]
fn the_viewer_goes_through_the_whole_thread_and_zooms() {
    use crate::images::Crop;
    let mut app = local_app();
    app.images = crate::images::Images::offline();
    app.goto_str("a/x/1");
    let file = |name: &str| Attachment { filename: name.into(), ..Attachment::at(format!("http://127.0.0.1:9/{name}")) };
    let post = |no, files: Vec<Attachment>| Post { no, files, ..Default::default() };
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(vec![post(1, vec![file("a.png")]), post(2, vec![]), post(3, vec![file("b.png"), file("c.png")]), post(4, vec![file("d.png")])])));
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
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(vec![post(1, vec![file("a.png"), file("e.png")])])));
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
    settle_until(&mut app, |a| a.tab.loading().is_none());
    let n = app.tab.thread.as_ref().unwrap().posts.len();
    assert!(n > 3 && app.tab.cached().is_none());
    assert_eq!(site.asked(), [None]);
    // A restart: shown at once from the copy (no request for that), marked cached.
    http::forget_host(host);
    let mut app = page_app(host, dir.path());
    site.hold(true);
    app.goto_str("c/g/30364");
    settle_until(&mut app, |a| a.tab.thread.is_some());
    assert_eq!(app.tab.cached(), Some(tabs::Offline { saved: 5000, dead: false }));
    assert!(app.tab.loading().is_some());
    // Nothing is new against the last visit; reading on while it loads.
    let t = app.tab.thread.as_mut().unwrap();
    assert!((0..n).all(|i| !t.is_new(i)));
    t.select(3);
    // The refresh asks If-Modified-Since; unchanged, it's a 304, and the place stays.
    *http::lock(&site.mode) = 304;
    site.hold(false);
    settle_until(&mut app, |a| a.tab.loading().is_none());
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!((t.selected, t.posts.len(), app.tab.cached()), (3, n, None));
    assert_eq!(site.asked(), [None, Some("day 1".into())]);
    // A failing refresh leaves the copy shown, still marked.
    for (mode, dead) in [(500, false), (404, true)] {
        http::forget_host(host);
        let mut app = page_app(host, dir.path());
        *http::lock(&site.mode) = mode;
        app.goto_str("c/g/30364");
        settle_until(&mut app, |a| a.tab.loading().is_none());
        assert_eq!(app.tab.cached().map(|c| c.dead), Some(dead), "{mode}");
        assert!(app.footer.get().unwrap().error);
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
    settle_until(&mut app, |a| a.tab.loading().is_none());
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
    settle_until(&mut app, |a| a.tab.loading().is_none());
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
    let file = |name: &str| Attachment { filename: name.into(), size: Some(1 << 20), ..Attachment::at(format!("http://127.0.0.1:3/x/src/{name}")) };
    let posts = vec![
        Post { no: 1, files: vec![file("a.png")], ..Default::default() },
        Post { no: 2, files: vec![file("b.png"), file("c.png")], ..Default::default() },
        Post { no: 3, ..Default::default() },
    ];
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts)));
    let press = |app: &mut App, c: char| app.on_key(KeyEvent::from(KeyCode::Char(c)));
    let total = |app: &App| app.downloads.total;
    // d on a post with nothing focused saves nothing, and says how.
    press(&mut app, 'd');
    assert_eq!(total(&app), 0);
    assert!(app.footer.get().unwrap().text.starts_with("tab to a file, then d saves it (the . menu saves"), "{:?}", app.footer.get());
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
    assert!(c.lines[1].starts_with("to ") && c.lines[1].ends_with(&super::settings::tilde(&dir.path().display().to_string())));
    // Anything but enter cancels.
    press(&mut app, 'j');
    assert!(app.confirm().is_none() && total(&app) == 1);
    assert_eq!(app.footer.get().unwrap().text, "Not saved");
    // So does a click.
    run_menu_row(&mut app, "save all the thread's files…");
    app.begin_frame();
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
    let file = Attachment { filename: "a.png".into(), ..Attachment::at("http://127.0.0.1:3/x/src/a.png") };
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(vec![Post { no: 1, files: vec![file], ..Default::default() }])));
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
    assert!(app.adding().is_some() && app.footer.get().unwrap().text == "A site is already called A");
    app.on_key(KeyEvent::from(KeyCode::Backspace));
    type_text(&mut app, "my/chan");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.adding().is_some() && app.footer.get().unwrap().text.contains("can't have /"));
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
    assert_eq!((app.tab.site, app.tab.view(), app.tab.pending_thread.unwrap(), app.tab.opening().select), (new, View::Thread, 5, Some(7)));
    assert!(app.visible_sites().contains(&SiteRow::Site(new)));
    // From now on its links just open.
    app.goto_str("https://newchan.invalid/tech/");
    assert!(app.adding().is_none());
    assert_eq!((app.tab.site, app.tab.view()), (new, View::Catalog));
    // A site that doesn't answer like any engine: said, nothing added. (After the catalog
    // load above has answered, so its error doesn't take the status.)
    settle_until(&mut app, |a| a.tab.loading().is_none());
    serve("blank.invalid", vec![]);
    app.goto_str("blank.invalid/b/");
    settle_until(&mut app, |a| a.adding().is_none());
    assert!(app.footer.get().unwrap().text.starts_with("blank.invalid doesn't answer like"), "{:?}", app.footer.get());
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
    app.tab.navigate(View::Sites);
    // From the home screen's menu (it has no key).
    run_menu_row(&mut app, "add a site…");
    app.paste("vi2.invalid/tech/");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    settle_until(&mut app, |a| matches!(a.adding(), Some(Adding::Site { .. })));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let i = app.sites.len() - 1;
    assert_eq!(app.footer.get().unwrap().text, "Added vi2 (vichan): it's on the home screen");
    assert_eq!(app.tab.view(), View::Sites);
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
    assert!(app.footer.get().unwrap().text.contains("has no /zz/"));
    // One it has: nothing to do.
    drawn(&mut app);
    app.popup = Some(Popup::Adding(Adding::Typing("vi2.invalid/b/".into())));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.adding().is_none() && app.footer.get().unwrap().text == "vi2 is already one of your sites");
    // esc while asking: the answer is dropped.
    app.popup = Some(Popup::Adding(Adding::Typing("vi2.invalid/b2/".into())));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    std::thread::sleep(Duration::from_millis(50));
    app.poll();
    assert!(app.adding().is_none());
    // Settings › Your sites lists it; x twice takes it out of the config and off the home screen.
    app.tab.navigate(View::Settings);
    let mine = settings::position("Your sites").unwrap();
    app.settings_list.state.select(Some(mine));
    app.activate_setting();
    assert!(matches!(app.settings_popup(), Some(SettingsPopup::Sites(m)) if m.sites.len() == 1 && m.sites[0].name == "vi2"));
    // Behind an error not yet seen the question isn't asked, so x twice removes nothing.
    app.error("Couldn't save to the data directory: disk full");
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    assert!(matches!(app.settings_popup(), Some(SettingsPopup::Sites(m)) if m.sites.len() == 1));
    drawn(&mut app);
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    assert_eq!(app.footer.get().unwrap().text, "x again removes vi2 from your config");
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    assert!(matches!(app.settings_popup(), Some(SettingsPopup::Sites(m)) if m.sites.is_empty()));
    assert!(!app.visible_sites().contains(&SiteRow::Site(i)));
    assert!(toml::from_str::<Config>(&std::fs::read_to_string(&path).unwrap()).unwrap().sites.is_empty());
    crate::http::serve_test_host("vi2.invalid", None);
}

fn thread_app_of(n: u64) -> App {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.navigate(View::Thread);
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
    assert!(t.is_new(t.selected) && t.new_below().0 < 5);
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
    assert_eq!(t.new_below().0, 5);
    app.act(Action::Unread);
    draw_at(&mut app, 100, 30);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 41);
}

#[test]
fn hidden_posts_are_not_new_in_the_open_thread() {
    let mut app = thread_app_of(40);
    app.set_title = true;
    for _ in 0..12 {
        app.on_key(KeyEvent::from(KeyCode::Char('j')));
    }
    draw_at(&mut app, 100, 30);
    // 41-45 arrive, 41 and 43 hidden: three are new, and U skips the hidden line.
    app.rehide(|a| a.store.toggle_hidden("a", "x", 41));
    app.rehide(|a| a.store.toggle_hidden("a", "x", 43));
    app.set_thread(posts_upto(45));
    draw_at(&mut app, 100, 30);
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!(((0..t.posts.len()).filter(|&i| t.is_new(i)).count(), t.new_below().0), (3, 3));
    let unread = t.unread_line().and_then(|e| t.entries.get(e)).and_then(|e| t.posts.get(e.post)).map(|p| p.no);
    assert_eq!(unread, Some(42));
    assert!(app.terminal_title().unwrap().starts_with("ck: (3) "));
    app.act(Action::Unread);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 42);
    // Shown with Z, they're new again.
    app.act(Action::ShowHidden);
    draw_at(&mut app, 100, 30);
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!((0..t.posts.len()).filter(|&i| t.is_new(i)).count(), 5);
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
    assert_eq!((t.current().unwrap().no, t.new_below().0), (40, 0));
    assert!(t.scroll >= scroll);
    // New posts that are hidden are passed over; all hidden: nothing moves.
    app.rehide(|a| a.store.toggle_hidden("a", "x", 41));
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
    app.tab.navigate(View::Thread);
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
    app.tab.navigate(View::Thread);
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
    app.tab.navigate(View::Thread);
    app.set_thread((1..=60).map(|no| with_file(no, None)).collect());
    draw_at(&mut app, 100, 30);
    // Nothing on screen, nothing prefetched below it, nothing in the gallery.
    assert_eq!(app.images.queued_urls(), Vec::<String>::new());
    app.act(Action::Gallery);
    draw_at(&mut app, 100, 30);
    assert!(app.images.queued_urls().is_empty());
    // The viewer says why instead of opening.
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.tab.viewer().is_none() && app.footer.get().unwrap().text.starts_with("Images are off on /x/"));
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
    app.tab.navigate(View::Catalog);
    app.tab.catalog_site = 0;
    app.tab.catalog_board = Some("all".into());
    app.tab.catalog = (1..=6).map(|no| with_file(no, Some(if no % 2 == 0 { "x" } else { "xy" }))).collect();
    app.tab.catalog_marks = crate::app::Marks::from_marks(vec![Default::default(); 6]);
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
    app.store.keep_thread(&key("x", 1), "one", "u", &whole(&posts_saying(&[(1, "about rust"), (2, "nothing here"), (3, "Rust again")])), 900);
    app.store.keep_thread(&key("xy", 7), "seven", "u", &whole(&posts_saying(&[(7, "no match"), (8, "a crab: RUST")])), 950);
    app.store.keep_thread(&key("x", 9), "nine", "u", &whole(&posts_saying(&[(9, "quiet")])), 980);
    app.flush_writes();
    app.tab.navigate(View::Saved);
    // From the Saved view's menu: `:` with "saved " typed.
    run_menu_row(&mut app, "search inside the saved threads…");
    assert_eq!(app.goto_text(), Some("saved "));
    type_text(&mut app, "rust");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.tab.view(), View::Search);
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
    assert!(app.tab.saved().is_some());
    assert_eq!((t.no, t.current().unwrap().no, t.search.as_str(), t.matches.len()), (1, 3, "rust", 2));
    // esc: back to the results, then where the search started.
    app.on_key(KeyEvent::from(KeyCode::Esc));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.tab.view(), View::Search);
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.tab.view(), View::Saved);
    // A copy that can't be read is skipped and said; the rest still count.
    std::fs::write(crate::saved::path(dir.path(), &key("x", 9)), b"not json").unwrap();
    app.goto_str("saved quiet");
    settle_until(&mut app, |a| a.tab.search.as_ref().unwrap().saved.as_ref().unwrap().finished);
    let s = app.tab.search.as_ref().unwrap();
    assert_eq!((s.hits.len(), s.saved.as_ref().unwrap().skipped), (0, 1));
    assert_eq!(app.footer.get().unwrap().text, "No saved post matches \"quiet\"");
    // Another search stops the one running: only its answers count.
    app.goto_str("saved rust");
    app.goto_str("saved crab");
    settle_until(&mut app, |a| a.tab.search.as_ref().unwrap().saved.as_ref().unwrap().finished);
    let s = app.tab.search.as_ref().unwrap();
    assert_eq!((s.query.as_str(), s.hits.len()), ("crab", 1));
}

#[test]
fn searching_saved_threads_leaves_out_hidden_posts() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = |board: &str, no| ThreadKey { site: "a".into(), board: board.into(), no };
    app.store.keep_thread(&key("x", 1), "one", "u", &whole(&posts_saying(&[(1, "about rust"), (2, "nothing here"), (3, "Rust again")])), 900);
    app.store.keep_thread(&key("xy", 7), "seven", "u", &whole(&posts_saying(&[(7, "no match"), (8, "a crab: RUST")])), 950);
    app.flush_writes();
    // A hidden word, and a post hidden by hand on its own board.
    app.hidden_words = vec!["crab".into()];
    app.rehide(|a| a.hiding.set_filters(crate::filter::Filters::new(&[]).unwrap().with_words(&a.hidden_words).unwrap()));
    app.rehide(|a| a.store.toggle_hidden("a", "x", 3));
    app.goto_str("saved rust");
    settle_until(&mut app, |a| a.tab.search.as_ref().unwrap().saved.as_ref().unwrap().finished);
    let shown = |app: &App| app.visible_hits().iter().map(|&k| app.tab.search.as_ref().unwrap().hits[k].1.no).collect::<Vec<_>>();
    assert_eq!(shown(&app), [1]);
    // Everything found hidden: said, with how to see it.
    app.goto_str("saved crab");
    settle_until(&mut app, |a| a.tab.search.as_ref().unwrap().saved.as_ref().unwrap().finished);
    assert!(shown(&app).is_empty());
    assert_eq!(app.footer.get().unwrap().text, "1 result, all hidden (Z shows them)");
}

#[test]
fn searching_saved_threads_with_none_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    app.goto_str("saved anything");
    settle_until(&mut app, |a| a.tab.search.as_ref().is_some_and(|s| s.saved.as_ref().unwrap().finished));
    assert!(app.tab.search.as_ref().unwrap().hits.is_empty());
    assert!(app.footer.get().unwrap().text.starts_with("Nothing is saved yet"));
    // `saved` alone is still the Saved view.
    app.goto_str("saved");
    assert_eq!(app.tab.view(), View::Saved);
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
    app.tab.navigate(View::Settings);
    let mine = settings::position("Your sites").unwrap();
    app.settings_list.state.select(Some(mine));
    app.activate_setting();
    app.on_key(KeyEvent::from(KeyCode::Char('r')));
    settle_until(&mut app, |a| matches!(a.adding(), Some(Adding::Boards { .. })));
    let Some(Adding::Boards { update, drop: false, .. }) = app.adding() else { panic!("{:?}", app.footer.get()) };
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
    assert_eq!(app.footer.get().unwrap().text, "vb's board list is up to date");
    // Pages without a bar: said, nothing changes.
    serve_text("vb.invalid", vec![("/", "<html></html>".into())]);
    crate::http::forget_host("vb.invalid");
    app.refresh_board_list(0);
    settle_until(&mut app, |a| a.adding().is_none());
    assert!(app.footer.get().unwrap().text.contains("have no board list to read"));
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
    app.tab.navigate(View::Thread);
    app.set_thread(posts_saying(&[(1, "op"), (2, "free crypto here"), (3, "I like Crypto"), (4, "cryptography")]));
    // Settings › Filters › Hidden words: a, type, enter.
    app.tab.navigate(View::Settings);
    let at = settings::position("Hidden words").unwrap();
    app.settings_list.state.select(Some(at));
    app.activate_setting();
    app.on_key(KeyEvent::from(KeyCode::Char('a')));
    type_text(&mut app, "crypto");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.footer.get().unwrap().text.starts_with("Hiding posts with \"crypto\" (2 here)"), "{:?}", app.footer.get());
    let hidden = |app: &App| app.tab.thread.as_ref().unwrap().marks.all_hidden();
    let label = Some(Hidden::ByFilter("hidden word: crypto".into()));
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
    app.tab.navigate(View::Thread);
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
    assert_eq!(hidden(&app)[1], Some(Hidden::ByFilter("hidden word: free".into())));
    app.on_key(KeyEvent::from(KeyCode::Char('u')));
    assert!(app.hidden_words.is_empty() && hidden(&app)[1].is_none());
    // Catalogs too.
    app.hidden_words = vec!["crypto".into()];
    app.apply_filters();
    app.tab.catalog = posts_saying(&[(10, "crypto thread"), (11, "a thread")]);
    app.tab.catalog_board = Some("x".into());
    app.remark();
    assert_eq!(app.tab.catalog_marks.all_hidden(), [label, None]);
}

#[test]
fn g_shows_the_very_end_so_new_posts_follow() {
    // A last post that doesn't fit below the scroll margin on a small screen: G still shows
    // the end of the thread (found walking a busy thread at 60x20).
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.navigate(View::Thread);
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
    assert_eq!(app.tab.view(), View::Boards);
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
    app.tab.navigate(View::Thread);
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
    app.tab.navigate(View::Thread);
    app.set_thread(nos(&[1, 2]));
    let now = Instant::now();
    assert!(app.next_wake(now) <= Duration::from_secs(1));
    assert_eq!(app.refresh_thread, Duration::from_secs(86400));
}

#[test]
fn a_request_that_panics_ends_like_one_that_failed() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let then = Then::Thread { open: Opening::default() };
    app.spawn("Loading the thread".into(), then, |_, _, _| -> Result<Thread> { std::panic::panic_any("deliberate: index out of bounds") }, thread_arrived);
    settle_until(&mut app, |a| a.tab.loading().is_none());
    let status = app.footer.get().unwrap();
    assert!(status.error && status.text.contains("ck hit a bug: deliberate: index out of bounds"), "{}", status.text);
}

#[cfg(unix)]
#[test]
fn a_termination_signal_quits_like_q() {
    let mut app = local_app();
    app.quit_on_signals().unwrap();
    signal_hook::low_level::raise(signal_hook::consts::SIGTERM).unwrap();
    settle_until(&mut app, |a| a.quit);
}

#[test]
fn new_posts_wait_for_notifying_by_the_app_clock() {
    let mut app = local_app();
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    let post = |no| Post { no, ..Default::default() };
    // The app's clock is an hour on from the real one.
    let later = Instant::now() + Duration::from_secs(3600);
    app.clock = Clock { instant: Some(later), ..Default::default() };
    app.store.watch(key.clone(), "One".into(), 1, 1);
    refresh(&mut app, key.clone(), vec![post(1)]);
    refresh(&mut app, key, vec![post(1), post(2)]);
    assert_eq!(app.notes_since, Some(later));
}

#[test]
fn removing_a_saved_copy_asks_again_once_the_question_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 3 };
    app.store.keep_thread(&key, "thread 3", "u", &whole(&nos(&[3])), 100);
    app.tab.navigate(View::Saved);
    app.saved_list.state.select(Some(0));
    app.act(Action::Remove);
    // The question goes (it expired, or something else was said): x asks again.
    expired(&mut app);
    app.act(Action::Remove);
    assert!(app.store.saved(&key).is_some());
    app.act(Action::Remove);
    assert!(app.store.saved(&key).is_none());
}

#[test]
fn where_ck_can_start_is_checked_first() {
    let cfg: Config = toml::from_str(crate::config::DEFAULT_CONFIG).unwrap();
    for fine in ["4chan/g", "4chan/g/123#456", "https://boards.4chan.org/g/thread/1", "saved", "saved some words", "history", "somechan.example/b/", "lainchan/λ"] {
        assert_eq!(start_error(&cfg.sites, fine), None, "{fine}");
    }
    let e = start_error(&cfg.sites, "nosuchsite/g").unwrap();
    assert!(e.contains("No site is called `nosuchsite`"), "{e}");
    assert!(start_error(&cfg.sites, "123").unwrap().contains("Open a board first"));
}

#[test]
fn a_failed_load_stays_on_screen_in_plain_words() {
    // A host of its own (one that refuses connections), so no other test's requests make
    // this one wait its turn.
    let mut app = app_with("[[site]]\nname = \"a\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:5\"\nboards = [\"x\"]");
    app.goto_str("a/x");
    settle_until(&mut app, |a| a.tab.loading().is_none());
    let failed = app.tab.failed.clone().unwrap();
    assert_eq!(failed, "Couldn't reach 127.0.0.1:5 (connection refused). r tries again");
    // Long after the footer's message has gone, it's still where the threads would be.
    expired(&mut app);
    let screen = draw_at(&mut app, 100, 30);
    let text: String = screen.content.iter().map(|c| c.symbol()).collect();
    assert!(text.contains("Couldn't reach 127.0.0.1:5 (connection refused)") && !text.contains("No threads"), "{text}");
    // Trying again clears it.
    app.act(Action::Reload);
    assert!(app.tab.failed.is_none());
}

#[test]
fn trying_again_clears_the_failure_from_the_footer_too() {
    let mut app = app_with("[[site]]\nname = \"a\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:5\"\nboards = [\"x\"]");
    app.goto_str("a/x");
    settle_until(&mut app, |a| a.tab.loading().is_none());
    assert!(app.footer.get().is_some_and(|s| s.error), "{:?}", app.footer.get());
    // Seen, then r while it's still up: the old failure doesn't outlive the retry.
    app.act(Action::Reload);
    assert_eq!(app.footer.get(), None);
}

/// The local app on /x/, in the thread view.
fn thread_app() -> App {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.navigate(View::Thread);
    app
}

#[test]
fn deleted_posts_stay_marked_deleted() {
    let mut app = thread_app();
    let quoting = |no, q: u64| Post { no, quotes: vec![q], body: vec![Line::from(format!("reply {no}"))], ..Default::default() };
    let mut posts = nos(&[1, 2, 3, 4, 5]);
    posts.push(quoting(6, 3));
    app.set_thread(posts.clone());
    // No.3 deleted: kept in its place, with its reply.
    let without = |gone: &[u64]| posts.iter().filter(|p| !gone.contains(&p.no)).cloned().collect::<Vec<_>>();
    app.set_thread(without(&[3]));
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!(t.posts.iter().map(|p| p.no).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6]);
    assert!(t.is_deleted(2) && !t.is_deleted(1));
    assert_eq!(t.backlinks[2], [6]);
    assert_eq!(t.live_posts().len(), 5);
    let screen: String = draw_at(&mut app, 100, 30).content.iter().map(|c| c.symbol()).collect();
    assert!(screen.contains(" deleted ") && screen.contains("5 posts") && screen.contains("1 deleted"), "{screen}");
    // Its quote and its conversation work.
    app.tab.thread.as_mut().unwrap().select(5);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 3);
    app.act(Action::Conversation);
    assert!(app.tab.thread.as_ref().unwrap().conversation.is_some());
    app.act(Action::Conversation);
    // Still deleted on the next refresh, with another one gone, and a new post: in order.
    let mut next = without(&[3, 5]);
    next.push(Post { no: 7, ..Default::default() });
    app.set_thread(next);
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!(t.posts.iter().map(|p| p.no).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(t.deleted, HashSet::from([3, 5]));
    // Back again (a moderator undid it): not deleted any more.
    app.set_thread(without(&[5]));
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!(t.deleted, HashSet::from([5, 7]));
}

#[test]
fn a_refresh_much_smaller_than_the_thread_is_shown_as_it_came() {
    let mut app = thread_app();
    app.set_thread(posts_upto(10));
    // Half is still deletion; fewer is more likely a broken answer.
    app.set_thread(posts_upto(5));
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!((t.posts.len(), t.deleted.len(), t.known), (10, 5, 5));
    app.set_thread(posts_upto(2));
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!((t.posts.len(), t.deleted.len(), t.known), (2, 0, 5));
    // Another thread isn't a refresh of this one.
    app.set_thread(nos(&[20, 21]));
    app.set_thread(nos(&[30]));
    assert!(app.tab.thread.as_ref().unwrap().deleted.is_empty());
}

#[test]
fn deleted_posts_are_never_new_or_counted() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.navigate(View::Thread);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.set_thread(nos(&[1, 2]));
    app.act(Action::Watch);
    // Two new posts, then the newest deleted: the other is still new, it isn't.
    app.set_thread(nos(&[1, 2, 3, 4]));
    app.set_thread(nos(&[1, 2, 3]));
    let t = app.tab.thread.as_ref().unwrap();
    assert!(t.is_new(2) && t.is_deleted(3) && !t.is_new(3));
    assert_eq!((0..t.posts.len()).filter(|&i| t.is_new(i)).count(), 1);
    // The saved copy and the counts are the thread as the site has it.
    app.flush_writes();
    assert_eq!(app.store.load_saved(&key).unwrap().posts.len(), 3);
    assert_eq!(app.store.saved(&key).unwrap().posts, 3);
    // A refresh with nothing new isn't a visit, though the deleted post was the newest.
    let seen = app.store.history[0].last_seen;
    app.set_thread(nos(&[1, 2, 3]));
    assert_eq!(app.store.history[0].last_seen, seen);
}

#[test]
fn replies_to_hidden_posts_hide_with_them() {
    use crate::filter::Hidden;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let cfg = "[[filter]]\npattern = \"recurse\"\nlabel = \"deep\"\nrecursive = true\n";
    std::fs::write(&path, cfg).unwrap();
    let mut app = app_with(&format!("{cfg}[[site]]\nname = \"a\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"x\"]\n"));
    app.config_path = Some(path);
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.navigate(View::Thread);
    let quoting = |no, q: &[u64], text: &str| Post { no, quotes: q.to_vec(), body: vec![Line::from(text.to_string())], ..Default::default() };
    // 3 replies to 2, 4 to 3, 5 to the OP; 7 replies to 6, which a recursive filter hides.
    app.set_thread(vec![
        quoting(1, &[], "op"),
        quoting(2, &[1], "two"),
        quoting(3, &[2], "three"),
        quoting(4, &[3, 1], "four"),
        quoting(5, &[1], "five"),
        quoting(6, &[], "recurse"),
        quoting(7, &[6], "seven"),
    ]);
    let hidden = |app: &App| app.tab.thread.as_ref().unwrap().marks.all_hidden();
    let r = |no| Some(Hidden::Reply(no));
    let deep = Some(Hidden::ByFilter("deep".into()));
    assert_eq!(hidden(&app), [None, None, None, None, None, deep.clone(), r(6)]);
    // Hidden by hand, without the setting: alone.
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.act(Action::Hide);
    assert_eq!(hidden(&app), [None, Some(Hidden::ByHand), None, None, None, deep.clone(), r(6)]);
    // With it (Settings, saved in the config): its replies, and theirs, collapse too.
    app.tab.navigate(View::Settings);
    app.settings_list.state.select(settings::position("Hidden replies"));
    app.enter();
    assert!(app.hiding.recursive() && config_text(&app).contains("recursive_hiding = true"));
    app.tab.navigate(View::Thread);
    assert_eq!(hidden(&app), [None, Some(Hidden::ByHand), r(2), r(3), None, deep.clone(), r(6)]);
    let screen: String = draw_at(&mut app, 100, 40).content.iter().map(|c| c.symbol()).collect();
    assert!(screen.contains("No.3  hidden") && !screen.contains("reply to hidden"), "{screen}");
    // H on a reply says where it comes from; unhiding the post it replies to shows it.
    app.tab.thread.as_mut().unwrap().selected = 3;
    app.act(Action::Hide);
    assert!(app.footer.get().unwrap().text.contains("reply to No.3"), "{:?}", app.footer.get());
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.act(Action::Hide);
    assert_eq!(hidden(&app), [None, None, None, None, None, deep, r(6)]);
    // A refresh hides new replies too.
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.act(Action::Hide);
    let mut more = app.tab.thread.as_ref().unwrap().posts.clone();
    more.push(quoting(8, &[4], "eight"));
    app.set_thread(more);
    assert_eq!(hidden(&app).last().unwrap(), &r(4));
    // The catalog's threads aren't replies to anything.
    app.tab.catalog = vec![quoting(6, &[], "recurse"), quoting(10, &[6], "x")];
    app.remark();
    assert!(app.tab.catalog_marks.why_hidden(1).is_none());
}

#[test]
fn the_terminal_title_says_where_and_whats_new() {
    let mut app = local_app();
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    let title = |app: &App| app.terminal_title().unwrap_or_default();
    // Away from a thread: the watched threads' unread posts, and whether some reply to yours.
    assert_eq!(title(&app), "ck: Sites");
    app.store.watch(key(1), "One".into(), 2, 5);
    app.store.watch(key(9), "Gone".into(), 2, 5);
    app.store.watched_mut(&key(1)).unwrap().status = Status::Live { unread: 3, replies: 0 };
    assert_eq!(title(&app), "ck: (3) Sites");
    app.store.watched_mut(&key(1)).unwrap().status = Status::Live { unread: 3, replies: 1 };
    app.store.watched_mut(&key(9)).unwrap().status = Status::Dead;
    assert_eq!(title(&app), "ck: (3) (You) Sites");
    // In a thread: its new posts below the screen. The subject is the site's text: nothing
    // in it reaches the terminal as an escape.
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.navigate(View::Thread);
    let mut posts = posts_saying(&[(1, "op"), (2, "mine"), (3, "new"), (4, "new, to you")]);
    posts[0].subject = Some("Evil\x1b]2;owned\x07 sub\u{9b}2Jject\n".into());
    posts[3].quotes = vec![2];
    app.set_thread(posts);
    app.tab.thread.as_mut().unwrap().new_after = 2;
    app.rehide(|a| a.store.toggle_mine(&key(1), 2));
    draw_at(&mut app, 80, 8);
    assert_eq!(title(&app), "ck: (2) (You) /x/ Evil ]2;owned sub 2Jject");
    app.on_key(KeyEvent::from(KeyCode::Char('G')));
    draw_at(&mut app, 80, 8);
    assert_eq!(title(&app), "ck: /x/ Evil ]2;owned sub 2Jject");
    // Off (Settings): left alone.
    app.tab.navigate(View::Settings);
    app.settings_list.state.select(settings::position("Terminal title"));
    app.enter();
    assert!(!app.set_title && app.terminal_title().is_none());
    app.show_title();
}

#[test]
fn quiet_threads_are_refreshed_less_often() {
    let mut app = local_app();
    let t0 = Instant::now();
    let at = |app: &mut App, secs: u64| app.clock = Clock { instant: Some(t0 + Duration::from_secs(secs)), ..Default::default() };
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    let secs = |d: Duration| d.as_secs_f64();
    at(&mut app, 0);
    // A watched thread: each refresh that brings nothing waits half as long again, up to
    // ten times the setting (60s) and 10 minutes; a new post starts over.
    app.store.watch(key(1), "One".into(), 2, 2);
    let mut every = Vec::new();
    for _ in 0..9 {
        refresh(&mut app, key(1), nos(&[1, 2]));
        every.push(secs(app.watched_every(&key(1))));
    }
    assert_eq!(every, [60.0, 90.0, 135.0, 202.5, 303.75, 455.625, 600.0, 600.0, 600.0]);
    refresh(&mut app, key(1), nos(&[1, 2, 3]));
    assert_eq!(app.watched_every(&key(1)), Duration::from_secs(60));
    // The next one waits for it, on the app's clock.
    refresh(&mut app, key(1), nos(&[1, 2, 3]));
    refresh(&mut app, key(1), nos(&[1, 2, 3]));
    app.watched_checked.insert(key(1), t0);
    at(&mut app, 134);
    app.background();
    assert!(app.refreshing.is_empty());
    at(&mut app, 135);
    assert_eq!(app.next_wake(t0 + Duration::from_secs(135)), Duration::ZERO);
    app.background();
    assert!(app.refreshing.contains(&key(1)));
    app.refreshing.clear();
    // Off: the interval set, whatever.
    app.refresh_backoff = false;
    assert_eq!(app.watched_every(&key(1)), Duration::from_secs(60));
    app.refresh_backoff = true;
    app.rehide(|a| a.store.unwatch(&key(1)));

    // The open thread the same, from 10s up to 100s; opening it (or r) starts over.
    at(&mut app, 1000);
    app.goto_str("a/x/5");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(nos(&[5, 6]))));
    let mut every = Vec::new();
    for _ in 0..8 {
        refresh(&mut app, key(5), nos(&[5, 6]));
        every.push(secs(app.thread_every()));
    }
    assert_eq!(every, [15.0, 22.5, 33.75, 50.625, 75.9375, 100.0, 100.0, 100.0]);
    // A deleted post isn't news.
    refresh(&mut app, key(5), nos(&[5]));
    assert_eq!(app.thread_every(), Duration::from_secs(100));
    refresh(&mut app, key(5), nos(&[5, 7]));
    assert_eq!(app.thread_every(), Duration::from_secs(10));
    refresh(&mut app, key(5), nos(&[5, 7]));
    app.tab.thread_checked = t0 + Duration::from_secs(1000);
    at(&mut app, 1014);
    app.background();
    assert!(app.refreshing.is_empty());
    at(&mut app, 1015);
    app.background();
    assert!(app.refreshing.contains(&key(5)));
    app.refreshing.clear();
    app.act(Action::Reload);
    assert_eq!(app.thread_every(), Duration::from_secs(10));
    // Never under the refetch floor, whatever the settings.
    app.refresh_thread = Duration::from_secs(10);
    assert!(app.thread_every() >= crate::http::MIN_REFETCH);
}

#[test]
fn watched_threads_know_their_page_once_a_round_per_board() {
    // A vichan site: threads.json says where each thread is.
    let asked = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let log = asked.clone();
    crate::http::serve_test_host(
        "pages.invalid",
        Some(Arc::new(move |url: &str, _| {
            http::lock(&log).push(url.to_string());
            let fixture = if url.ends_with("/threads.json") { "vichan_pages.json" } else { "vichan_thread.json" };
            let body = std::fs::read_to_string(format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"))).unwrap();
            http::Raw { status: 200, last_modified: None, body }
        })),
    );
    let mut app = app_with("[[site]]\nname = \"p\"\nkind = \"vichan\"\nurl = \"https://pages.invalid\"\nboards = [\"tech\"]");
    let t0 = Instant::now();
    app.clock = Clock { instant: Some(t0), ..Default::default() };
    let key = |no| ThreadKey { site: "p".into(), board: "tech".into(), no };
    app.store.watch(key(30364), "First".into(), 2, 2);
    app.store.watch(key(39212), "Last".into(), 2, 2);
    let pages = |asked: &Arc<std::sync::Mutex<Vec<String>>>| http::lock(asked).iter().filter(|u| u.ends_with("/threads.json")).count();
    // Both threads refresh (one at a time), and the board's pages are asked for once.
    for _ in 0..2 {
        app.background();
        settle_until(&mut app, |a| a.refreshing.is_empty() && a.pages_asking.is_empty());
    }
    assert_eq!(http::lock(&asked).len(), 3, "{:?}", http::lock(&asked));
    assert_eq!(pages(&asked), 1);
    assert_eq!((app.thread_page(&key(30364)), app.thread_page(&key(39212))), (Some((1, 13)), Some((13, 13))));
    // Not a thread there: nothing to say.
    assert_eq!(app.thread_page(&key(2)), None);
    // The next round asks again.
    app.clock = Clock { instant: Some(t0 + Duration::from_secs(61)), ..Default::default() };
    app.background();
    settle_until(&mut app, |a| a.refreshing.is_empty() && a.pages_asking.is_empty());
    assert_eq!(pages(&asked), 2);
    // A thread that isn't watched (open, say) doesn't ask.
    app.rehide(|a| a.store.unwatch(&key(30364)));
    app.rehide(|a| a.store.unwatch(&key(39212)));
    app.clock = Clock { instant: Some(t0 + Duration::from_secs(200)), ..Default::default() };
    app.refresh_in_background(key(30364));
    settle_until(&mut app, |a| a.refreshing.is_empty() && a.pages_asking.is_empty());
    assert_eq!(pages(&asked), 2);
    // Nor is what was known shown for long after.
    app.background();
    assert!(app.thread_page(&key(30364)).is_some());
    app.clock = Clock { instant: Some(t0 + Duration::from_secs(61 + 1201)), ..Default::default() };
    app.background();
    assert_eq!(app.thread_page(&key(30364)), None);
    crate::http::serve_test_host("pages.invalid", None);

    // An engine that can't tell says so without asking.
    let archive = crate::backend::build(&toml::from_str("name = \"f\"\nkind = \"foolfuuka\"\nurl = \"https://127.0.0.1:3\"").unwrap());
    assert_eq!(archive.thread_pages("a").unwrap(), None);
}

#[test]
fn m_shows_posts_with_files_then_hides_images() {
    let mut app = local_app();
    app.goto_str("a/x/1");
    // 1 (the OP, no file), 2 with a file quoting 3, 3 without, 4 with.
    let mut posts = posts_saying(&[(1, "op"), (3, "three")]);
    posts.insert(1, Post { quotes: vec![3], ..with_file(2, None) });
    posts.push(with_file(4, None));
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts.clone())));
    let shown = |app: &App| {
        let t = app.tab.thread.as_ref().unwrap();
        t.entries.iter().map(|e| t.posts[e.post].no).collect::<Vec<_>>()
    };
    let media = |app: &App| app.tab.thread.as_ref().unwrap().media;
    // In the menu, then by key: the OP and the posts with files.
    let t = app.tab.thread.as_mut().unwrap();
    t.select(2);
    t.set_search("t".into());
    run_menu_row(&mut app, "only the posts with files");
    assert_eq!((media(&app), shown(&app)), (Media::Files, vec![1, 2, 4]));
    assert_eq!(app.footer.get().unwrap().text, "3 posts with files; M again shows all, images hidden");
    // The selected post had none: the next one that has is selected. Search finds what's
    // shown (not "three").
    let t = app.tab.thread.as_ref().unwrap();
    assert_eq!((t.current().unwrap().no, t.matches.clone()), (4, vec![1, 3]));
    app.tab.thread.as_mut().unwrap().set_search(String::new());
    // A refresh keeps it, with what it brings.
    posts.push(with_file(5, None));
    posts.push(posts_saying(&[(6, "six")]).remove(0));
    let key = app.key("x", 1);
    refresh(&mut app, key, posts.clone());
    assert_eq!(shown(&app), [1, 2, 4, 5]);
    // A quote to a post without files: everything again, there.
    let t = app.tab.thread.as_mut().unwrap();
    t.select(1);
    assert!(t.jump_to(3));
    assert_eq!((media(&app), shown(&app).len(), app.tab.thread.as_ref().unwrap().current().unwrap().no), (Media::All, 6, 3));
    // M twice: images hidden, on every post; the viewer isn't opened for them.
    app.act(Action::Media);
    app.act(Action::Media);
    assert_eq!((media(&app), shown(&app).len()), (Media::NoImages, 6));
    assert!(!app.thread_images_on() && app.images_on(app.tab.site, "x"));
    app.images = crate::images::Images::offline();
    app.tab.thread.as_mut().unwrap().select(1);
    app.act(Action::View);
    assert!(app.tab.viewer().is_none());
    assert!(app.footer.get().unwrap().text.starts_with("Images are hidden in this thread (M shows them)"));
    app.act(Action::Media);
    assert!(media(&app) == Media::All && app.thread_images_on());
    // In a conversation, its posts whatever they have; leaving it, those with files.
    app.tab.thread.as_mut().unwrap().select(2);
    app.act(Action::Conversation);
    app.act(Action::Media);
    assert_eq!(shown(&app), [2, 3]);
    assert!(app.footer.get().unwrap().text.starts_with("Only posts with files, once you leave the conversation"));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(shown(&app), [1, 2, 4, 5]);
    // Another thread starts with everything.
    app.goto_str("a/x/7");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(nos(&[7, 8]))));
    assert_eq!((media(&app), shown(&app)), (Media::All, vec![7, 8]));
}

/// Following a general end to end: the thread and the catalog come from a site (through the
/// real fetching and parsing), not handed to the app.
#[test]
fn following_a_general_through_the_site() {
    let host = "generals.invalid";
    let thread = r#"{"posts":[{"no":100,"resto":0,"time":1000,"sub":"/lmg/ - Local Models General #5","com":"OP","bumplimit":1,"replies":1},{"no":101,"resto":100,"time":1001,"com":"reply"}]}"#;
    let catalog = r#"[{"page":0,"threads":[{"no":100,"resto":0,"time":1000,"sub":"/lmg/ - Local Models General #5","replies":1},{"no":150,"resto":0,"time":1500,"sub":"/ldg/ - Local Diffusion General","replies":3},{"no":200,"resto":0,"time":2000,"sub":"/lmg/ - Local Models General #6","replies":0}]}]"#;
    let asked: Asked = Default::default();
    let log = asked.clone();
    crate::http::serve_test_host(
        host,
        Some(Arc::new(move |url: &str, _| {
            http::lock(&log).push((url.to_string(), None));
            let body = if url.ends_with("/g/res/100.json") {
                thread
            } else if url.ends_with("/g/catalog.json") {
                catalog
            } else {
                return http::Raw { status: 404, last_modified: None, body: String::new() };
            };
            http::Raw { status: 200, last_modified: None, body: body.into() }
        })),
    );
    let mut app = app_with(&format!("[[site]]\nname = \"c\"\nkind = \"vichan\"\nurl = \"http://{host}\"\nboards = [\"g\"]\n"));
    let key = |no| ThreadKey { site: "c".into(), board: "g".into(), no };
    // Open the thread, and F: followed (and watched).
    app.goto_str("c/g/100");
    settle_until(&mut app, |a| a.tab.loading().is_none());
    assert_eq!(app.tab.thread.as_ref().map(|t| t.no), Some(100));
    app.act(Action::Follow);
    assert_eq!(app.store.watched(&key(100)).and_then(|w| w.general.as_deref()), Some("/lmg/"));
    // A background refresh reads the site's bump limit flag.
    app.tab.navigate(View::Watched);
    app.refresh_in_background(key(100));
    settle_until(&mut app, |a| a.refreshing.is_empty());
    assert!(app.store.watched(&key(100)).unwrap().at_limit);
    // That starts a search of the board's catalog, which finds #6: watched, followed, told.
    app.check_generals(app.clock.instant());
    assert_eq!(app.generals_searching.len(), 1);
    settle_until(&mut app, |a| a.generals_searching.is_empty());
    assert_eq!(app.store.watched(&key(200)).and_then(|w| w.general.as_deref()), Some("/lmg/"));
    assert_eq!(app.store.watched(&key(100)).map(|w| w.general.clone()), Some(None), "the full one is kept, no longer followed");
    assert!(app.store.watched(&key(150)).is_none());
    assert!(app.notified.iter().any(|n| n.starts_with("New /lmg/ thread on /g/")), "{:?}", app.notified);
    let urls: Vec<String> = http::lock(&asked).iter().map(|(u, _)| u.clone()).collect();
    assert!(urls.iter().any(|u| u.ends_with("/g/catalog.json")), "{urls:?}");
    crate::http::serve_test_host(host, None);
}

// Hiding hides everywhere: each of these reads posts or threads past the hiding marks.

#[test]
fn the_menu_offers_no_gallery_when_only_hidden_posts_have_files() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let file = Attachment { filename: "hidden.png".into(), ..Attachment::at("http://127.0.0.1:3/x/src/hidden.png") };
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }, Post { no: 2, files: vec![file], ..Default::default() }]));
    app.tab.navigate(View::Thread);
    app.rehide(|a| a.store.toggle_hidden("a", "x", 2));
    // The gallery has nothing to show, and says why; so the menu doesn't offer it.
    app.act(Action::Gallery);
    assert_eq!(app.footer.get().map(|s| s.text.as_str()), Some("Only hidden posts have files (Z shows them)"));
    expired(&mut app);
    app.act(Action::DownloadThread);
    assert_eq!(app.footer.get().map(|s| s.text.as_str()), Some("Only hidden posts have files (Z shows them)"));
    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    let m = app.menu().unwrap();
    let has = |a: Action| m.items.iter().any(|i| matches!(i, MenuItem::Act(x, _) if *x == a));
    assert!(!has(Action::Gallery) && !has(Action::DownloadThread), "{:?}", m.items);
}

#[test]
fn only_hidden_thumbnails_are_not_files_to_save() {
    // A hidden post's file the archive kept only the thumbnail of: showing hidden posts
    // brings it to the gallery, but there's still nothing to save.
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let file = Attachment { filename: "clip.webm".into(), url: None, kind: FileKind::Video, thumb: Some("http://127.0.0.1:3/x/thumb/1s.jpg".into()), ..Default::default() };
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }, Post { no: 2, files: vec![file], ..Default::default() }]));
    app.tab.navigate(View::Thread);
    app.rehide(|a| a.store.toggle_hidden("a", "x", 2));
    app.act(Action::Gallery);
    assert_eq!(app.status().map(|s| s.text.as_str()), Some("Only hidden posts have files (Z shows them)"));
    app.footer = Footer::default();
    app.act(Action::DownloadThread);
    assert_eq!(app.status().map(|s| s.text.as_str()), Some("Thread has no files to save"));
}

#[test]
fn hiding_recounts_new_posts_in_watched_threads() {
    let mut app = local_app();
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.store.watch(key.clone(), "One".into(), 1, 1);
    let start = posts_saying(&[(1, "a thread")]);
    refresh(&mut app, key.clone(), start);
    refresh(&mut app, key.clone(), posts_saying(&[(1, "a thread"), (2, "buy crypto"), (3, "hello")]));
    assert_eq!(app.store.watched(&key).unwrap().status.counts().0, 2);
    // A hidden word hides No.2: Watched's "new" leaves it out at once, not at the next refresh.
    app.hidden_words = vec!["crypto".into()];
    app.apply_filters();
    assert_eq!(app.store.watched(&key).unwrap().status.counts().0, 1);
    // And counts it again when the word goes.
    app.hidden_words.clear();
    app.apply_filters();
    assert_eq!(app.store.watched(&key).unwrap().status.counts().0, 2);
}

/// Hiding's recount and a 404 are one count: once the thread is gone, nothing that
/// changes what's hidden brings its last new posts back, in it or in the totals.
#[test]
fn hiding_recounts_nothing_in_a_dead_watched_thread() {
    let mut app = local_app();
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.store.watch(key.clone(), "One".into(), 1, 1);
    refresh(&mut app, key.clone(), posts_saying(&[(1, "a thread")]));
    refresh(&mut app, key.clone(), posts_saying(&[(1, "a thread"), (2, "buy crypto"), (3, "hello")]));
    assert_eq!(app.store.watched_new(), (2, 0));
    app.refreshed(key.clone(), Err(gone()));
    assert_eq!(app.store.watched(&key).unwrap().status, Status::Dead);
    for words in [vec!["crypto".to_string()], Vec::new()] {
        app.hidden_words = words;
        app.apply_filters();
        assert_eq!((app.store.watched(&key).unwrap().status, app.store.watched_new()), (Status::Dead, (0, 0)));
    }
}

#[test]
fn following_a_general_passes_hidden_threads_over() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    let op = |no, subject: &str| Post { no, subject: Some(subject.into()), replies: Some(10), ..Default::default() };
    app.set_thread(vec![op(10, "/lmg/ - Local Models General #5"), Post { no: 11, ..Default::default() }]);
    app.tab.navigate(View::Thread);
    app.act(Action::Follow);
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    // The next one is caught by a hidden word: it isn't watched, followed or announced.
    app.hidden_words = vec!["shill".into()];
    app.apply_filters();
    app.notified.clear();
    app.general_catalog(&key(10), Ok(vec![op(10, "/lmg/ - Local Models General #5"), op(13, "/lmg/ - shill edition")]));
    assert!(app.store.watched(&key(13)).is_none());
    assert!(app.notified.is_empty(), "{:?}", app.notified);
}

#[test]
fn searching_saved_threads_leaves_out_replies_to_hidden_posts() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    app.rehide(|a| a.hiding.set_recursive(true));
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    let mut posts = posts_saying(&[(1, "a thread"), (2, "buy crypto"), (3, "crypto, rust says"), (4, "rust, by that guy")]);
    posts[2].quotes = vec![2];
    posts[3].id = Some("Ab3d".into());
    app.store.keep_thread(&key, "one", "u", &whole(&posts), 900);
    let that_guy = crate::filter::FilterConfig::new("^Ab3d$".into(), &[crate::filter::Field::Id]);
    app.rehide(|a| a.hiding.set_filters(crate::filter::Filters::new(&[that_guy]).unwrap()));
    app.flush_writes();
    app.rehide(|a| a.store.toggle_hidden("a", "x", 2));
    // In the thread, No.3 is hidden as a reply to No.2.
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.set_thread(posts);
    assert!(matches!(app.tab.thread.as_ref().unwrap().marks.why_hidden(2), Some(&Hidden::Reply(2))));
    // So the search of the saved threads leaves it out too, and No.4, which a filter on its
    // poster's ID hides.
    app.goto_str("saved rust");
    settle_until(&mut app, |a| a.tab.search.as_ref().unwrap().saved.as_ref().unwrap().finished);
    let s = app.tab.search.as_ref().unwrap();
    assert_eq!(s.hits.len(), 2);
    let shown: Vec<u64> = app.visible_hits().iter().map(|&k| s.hits[k].1.no).collect();
    assert!(shown.is_empty(), "{shown:?} shown though hidden in their thread");
}

#[test]
fn marking_a_post_as_yours_reaches_every_tab() {
    let mut app = local_app();
    let thread = || ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }, Post { no: 2, ..Default::default() }]);
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.thread = Some(thread());
    app.tab.navigate(View::Thread);
    app.tabs.push(Tab::new(0, Instant::now()));
    app.switch_tab(1);
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.thread = Some(thread());
    app.tab.navigate(View::Thread);
    app.switch_tab(0);
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.act(Action::Mine);
    app.switch_tab(1);
    assert!(app.tab.thread.as_ref().unwrap().marks.is_mine(2), "the other tab still doesn't mark No.2 (You)");
}

#[test]
fn unwatching_a_thread_forgets_which_posts_are_yours() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.thread = Some(ThreadView::new("x".into(), 1, vec![Post { no: 1, ..Default::default() }, Post { no: 2, ..Default::default() }]));
    app.tab.thread.as_mut().unwrap().selected = 1;
    app.tab.navigate(View::Thread);
    app.act(Action::Mine);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    assert!(app.store.watched(&key).is_some());
    // w stops watching it, and with that its list of yours goes: (You) goes too.
    app.act(Action::Watch);
    assert!(app.store.watched(&key).is_none());
    assert!(!app.tab.thread.as_ref().unwrap().marks.is_mine(2), "No.2 still shown as yours");
    // So does x in Watched, from another tab.
    app.act(Action::Mine);
    assert!(app.tab.thread.as_ref().unwrap().marks.is_mine(2));
    app.new_tab();
    app.tab.navigate(View::Watched);
    app.act(Action::Remove);
    assert!(app.store.watched(&key).is_none());
    app.switch_tab(0);
    assert!(!app.tab.thread.as_ref().unwrap().marks.is_mine(2), "No.2 still shown as yours after x in Watched");
}

#[test]
fn a_tab_searching_the_archive_keeps_its_catalog_and_thread_hidden() {
    let mut app = app_with(
        "[[site]]\nname = \"chan\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"g\"]\narchive = \"arch\"\n\
         [[site]]\nname = \"arch\"\nkind = \"foolfuuka\"\nurl = \"http://localhost:3\"\nboards = [\"g\"]",
    );
    app.tab.board = Some(Board { uri: "g".into(), title: String::new(), nsfw: None });
    app.tab.catalog = nos(&[1, 2]);
    app.set_thread(nos(&[1, 2]));
    app.rehide(|a| a.store.toggle_hidden("chan", "g", 2));
    let hidden = |app: &App| (app.tab.catalog_marks.why_hidden(1).is_some(), app.tab.thread.as_ref().unwrap().marks.why_hidden(1).is_some());
    assert_eq!(hidden(&app), (true, true));
    // The tab searches the archive (another site), and meanwhile what's hidden is decided
    // again (here: `Z` twice in another tab): No.2 is still hidden on chan's /g/.
    app.tab.navigate(View::Catalog);
    app.act(Action::ArchiveSearch);
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.tab.site, 1);
    app.new_tab();
    app.act(Action::ShowHidden);
    app.act(Action::ShowHidden);
    app.switch_tab(0);
    app.close_search();
    assert_eq!(hidden(&app), (true, true), "No.2 shown once the search closed");
}

/// A watched thread that 404s has nothing new any more: its unread posts and replies to
/// you go with it, whichever load found it gone (a background refresh, opening it, or
/// restoring last session's place).
#[test]
fn a_watched_thread_that_404s_keeps_no_counts() {
    let mut app = local_app();
    let key = |no| ThreadKey { site: "a".into(), board: "x".into(), no };
    app.store.watch(key(1), "One".into(), 2, 5);
    app.store.watched_mut(&key(1)).unwrap().status = Status::Live { unread: 3, replies: 2 };
    app.refreshed(key(1), Err(gone()));
    assert_eq!(app.store.watched(&key(1)).unwrap().status, Status::Dead, "refresh 404");
    // Opened and found gone.
    app.store.watch(key(2), "Two".into(), 2, 5);
    app.store.watched_mut(&key(2)).unwrap().status = Status::Live { unread: 4, replies: 1 };
    app.goto_str("a/x/2");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, Err(gone())));
    assert_eq!(app.store.watched(&key(2)).unwrap().status, Status::Dead, "open 404");
    // Last session's thread, found gone on restoring it.
    app.store.watch(key(3), "Three".into(), 2, 5);
    app.store.watched_mut(&key(3)).unwrap().status = Status::Live { unread: 5, replies: 1 };
    app.goto_str("a/x/3");
    app.tab.fake_load(99, "Loading thread 3", Then::Thread { open: Opening { restoring: true, ..Opening::default() } });
    app.handle(answer(99, thread_arrived, Err(gone())));
    assert_eq!((app.store.watched(&key(3)).unwrap().status, app.tab.view()), (Status::Dead, View::Catalog), "restore 404");
}

#[test]
fn gallery_files_keep_their_posts_when_the_live_thread_replaces_a_cached_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = thread_app();
    app.download_dir = Some(dir.path().display().to_string());
    app.images = crate::images::Images::offline();
    let file = |name: &str| Attachment { filename: name.into(), ..Attachment::at(format!("http://127.0.0.1:3/x/src/{name}")) };
    let post = |no, files: Vec<Attachment>| Post { no, files, ..Default::default() };
    // The cached copy has No.2; the live thread, arriving with the gallery open, doesn't.
    app.set_cached_thread(arrived(vec![post(1, vec![]), post(2, vec![]), post(3, vec![file("3.png")]), post(4, vec![file("4.png")])]).unwrap(), 0);
    app.act(Action::Gallery);
    app.set_thread(vec![post(1, vec![]), post(3, vec![file("3.png")]), post(4, vec![file("4.png")]), post(5, vec![])]);
    // The first file is still 3.png, from No.3: its link, its save and esc go there.
    assert_eq!(app.gallery_link(0).as_deref(), Some("http://127.0.0.1:3/x/res/1.html#3"));
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    assert_eq!(app.downloads.total, 1);
    app.close_gallery();
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 3);
}

#[test]
fn a_thread_hint_picks_its_post_after_a_refresh_moves_it() {
    let mut app = thread_app();
    app.set_cached_thread(arrived(nos(&[1, 2, 3, 4, 5])).unwrap(), 0);
    draw_at(&mut app, 100, 30);
    app.on_key(KeyEvent::from(KeyCode::Char('f')));
    let label = app.hints().unwrap().targets.iter().find(|x| matches!(x.to, HintTo::Thread(ref p, None) if p == &[4])).unwrap().label.clone();
    // The label went up on No.4; the live thread, without No.2, arrives before it's typed.
    app.set_thread(nos(&[1, 3, 4, 5]));
    type_text(&mut app, &label);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 4);
}

#[test]
fn a_catalog_hint_opens_its_thread_after_a_refresh_reorders_them() {
    let mut app = local_app();
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.catalog_board = Some("x".into());
    app.tab.navigate(View::Catalog);
    app.show_catalog(nos(&[1, 2, 3]), None);
    draw_at(&mut app, 100, 30);
    app.on_key(KeyEvent::from(KeyCode::Char('f')));
    let label = app.hints().unwrap().targets.iter().find(|x| matches!(x.to, HintTo::Row(RowKey::Thread(2)))).unwrap().label.clone();
    // The label went up on No.2; a refresh bumps No.3 to the top before it's typed.
    app.show_catalog(nos(&[3, 1, 2]), None);
    type_text(&mut app, &label);
    assert_eq!((app.tab.view(), app.tab.pending_thread), (View::Thread, Some(2)));
}

#[test]
fn u_skips_a_post_a_refresh_took_away() {
    let mut app = thread_app();
    app.set_cached_thread(arrived(nos(&[1, 2, 3, 4, 5])).unwrap(), 0);
    let t = app.tab.thread.as_mut().unwrap();
    t.select(1);
    assert!(t.jump_to(3) && t.jump_to(5));
    // The live thread, without No.3, arrives: u goes back past it, to No.2.
    app.set_thread(nos(&[1, 2, 4, 5]));
    app.act(Action::JumpBack);
    assert_eq!(app.tab.thread.as_ref().unwrap().current().unwrap().no, 2);
    // With only a dropped post to go back to, there's nowhere: the menu doesn't offer u.
    let mut app = thread_app();
    app.set_cached_thread(arrived(nos(&[1, 2, 3])).unwrap(), 0);
    let t = app.tab.thread.as_mut().unwrap();
    t.select(1);
    assert!(t.jump_to(3) && app.can_jump_back());
    app.set_thread(nos(&[1, 3]));
    assert!(!app.can_jump_back());
}

#[test]
fn a_recent_board_label_opens_its_board_after_another_comes_first() {
    let mut app = local_app();
    app.store.recent_boards = vec!["a/x".into(), "b/y".into()];
    app.tab.navigate(View::Sites);
    draw_at(&mut app, 100, 30);
    app.on_key(KeyEvent::from(KeyCode::Char('f')));
    let label = app.hints().unwrap().targets.iter().find(|x| matches!(x.to, HintTo::Row(RowKey::Recent(ref b)) if b == "a/x")).unwrap().label.clone();
    // The label went up on a/x; a catalog asked for before arrives, and its board goes first.
    app.store.board_opened("a", "xy");
    type_text(&mut app, &label);
    assert_eq!((app.tab.view(), app.tab.board.as_ref().unwrap().uri.as_str()), (View::Catalog, "x"));
}

#[test]
fn search_hits_with_the_same_numbers_in_two_saved_copies_are_told_apart() {
    let mut app = local_app();
    let key = |board: &str| ThreadKey { site: "a".into(), board: board.into(), no: 100 };
    let op = Post { no: 100, ..Default::default() };
    let mut s = Search::for_tests("", "q", crate::backend::SearchPage { hits: vec![(100, op.clone()), (100, op)], total: None });
    s.saved = Some(SavedSearch::for_tests(vec![key("x"), key("xy")], 2, 2, true));
    app.tab.search = Some(s);
    let rows = app.row_keys(View::Search);
    assert_eq!(rows.iter().map(|k| app.row_of(View::Search, k)).collect::<Vec<_>>(), [Some(0), Some(1)]);
}

fn posts(nos: &[u64]) -> Vec<Post> {
    nos.iter().map(|&no| Post { no, ..Default::default() }).collect()
}

#[test]
fn closing_the_settings_keeps_the_load_they_were_opened_over() {
    let mut app = local_app();
    app.goto_str("a/x/1");
    let req = app.tab.req().unwrap();
    app.tab.navigate(View::Settings);
    app.back();
    assert_eq!(app.tab.view(), View::Thread);
    // The thread asked for before the settings opened still arrives.
    app.handle(answer(req, thread_arrived, arrived(posts(&[1, 2]))));
    assert_eq!(app.tab.thread.as_ref().map(|t| t.no), Some(1));
}

#[test]
fn closing_the_settings_keeps_the_way_back_with_u() {
    let mut app = local_app();
    app.goto_str("a/x/1");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts(&[1, 2]))));
    app.goto_str("a/x/3");
    assert_eq!(app.tab.trail.len(), 1);
    app.tab.navigate(View::Settings);
    app.back();
    assert_eq!((app.tab.view(), app.tab.trail.len()), (View::Thread, 1));
}

#[test]
fn a_thread_found_after_leaving_for_watched_leaves_the_tab_there() {
    let mut app = local_app();
    app.goto_str("a/x/1#77");
    let req = app.tab.req().unwrap();
    app.goto_str("watched");
    assert_eq!(app.tab.view(), View::Watched);
    // Leaving drops the request: no spinner over Watched, and its answer moves nothing.
    assert!(app.tab.loading().is_none(), "still loading: {:?}", app.tab.loading());
    let board = Board { uri: "x".into(), title: String::new(), nsfw: None };
    app.handle(answer(req, |a, (b, post, r)| a.thread_found(0, b, post, None, r), (board, 77, Ok(Some(3)))));
    assert_eq!(app.tab.view(), View::Watched);
}

#[test]
fn a_thread_that_dies_after_moving_to_another_site_marks_nothing_there() {
    let mut app = local_app();
    let other = ThreadKey { site: "b".into(), board: "x".into(), no: 1 };
    app.store.watch(other.clone(), String::new(), 1, 1);
    app.goto_str("a/x/1");
    let req = app.tab.req().unwrap();
    // `:b` goes to the other site's boards while a/x/1 is still loading.
    app.goto_str("b");
    assert_eq!((app.tab.site, app.tab.view()), (1, View::Boards));
    app.handle(answer(req, thread_arrived, Err(gone())));
    assert!(!app.store.watched(&other).unwrap().status.is_dead(), "b/x/1 was marked dead for a/x/1's 404");
}

#[test]
fn leaving_a_restored_thread_before_it_loads_forgets_the_restore() {
    let mut app = local_app();
    let place = crate::store::Place { view: "thread".into(), site: "a".into(), board: Some("x".into()), thread: Some(5), ..Default::default() };
    app.go_to_place(&place);
    // Esc before it loads: its catalog.
    app.back();
    assert_eq!(app.tab.view(), View::Catalog);
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(posts(&[7]))));
    app.tab.catalog_list.state.select(Some(0));
    // Another thread, opened by hand, is gone: that's this thread's 404, not last session's.
    app.enter();
    assert_eq!(app.tab.view(), View::Thread);
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, Err(gone())));
    let status = app.status().map(|s| s.text.clone()).unwrap_or_default();
    assert!(!status.contains("last time"), "{status}");
    assert_eq!(app.tab.view(), View::Thread);
}

#[test]
fn a_restored_thread_gone_while_the_settings_are_open_leaves_them_open() {
    let mut app = local_app();
    let place = crate::store::Place { view: "thread".into(), site: "a".into(), board: Some("x".into()), thread: Some(5), ..Default::default() };
    app.go_to_place(&place);
    let req = app.tab.req().unwrap();
    app.tab.navigate(View::Settings);
    app.handle(answer(req, thread_arrived, Err(gone())));
    assert_eq!(app.tab.view(), View::Settings);
}

#[test]
fn a_post_to_select_from_a_dropped_load_isnt_used_by_the_next_thread() {
    let mut app = local_app();
    app.goto_str("a/x/5#12");
    assert_eq!(app.tab.opening().select, Some(12));
    // Esc before it loads, then a watched thread that also has a No.12.
    app.back();
    app.tab.navigate(View::Watched);
    app.open_key(ThreadKey { site: "a".into(), board: "x".into(), no: 9 });
    // While it loads the session doesn't remember No.12 as its selected post...
    assert_eq!(app.place().selected, None);
    // ...and it opens at the top, not at No.12.
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts(&[9, 10, 12]))));
    assert_eq!(app.tab.thread.as_ref().unwrap().selected, 0);
}

#[test]
fn a_thread_found_behind_the_settings_loads_once_they_close() {
    let mut app = local_app();
    app.goto_str("a/x/1#77");
    app.tab.navigate(View::Settings);
    let board = Board { uri: "x".into(), title: String::new(), nsfw: None };
    app.handle(answer(app.tab.req().unwrap(), |a, (b, post, r)| a.thread_found(0, b, post, None, r), (board, 77, Ok(Some(3)))));
    let req = app.tab.req().unwrap();
    app.back();
    assert_eq!(app.tab.view(), View::Thread);
    app.handle(answer(req, thread_arrived, arrived(posts(&[3, 77]))));
    assert_eq!(app.tab.thread.as_ref().map(|t| t.no), Some(3));
}

#[test]
fn a_gallery_open_on_a_restored_thread_that_dies_isnt_over_the_next_thread() {
    let mut app = local_app();
    let place = crate::store::Place { view: "thread".into(), site: "a".into(), board: Some("x".into()), thread: Some(5), ..Default::default() };
    app.go_to_place(&place);
    let req = app.tab.req().unwrap();
    // Its last copy shows while it loads; `V` over it.
    let mut ps = posts(&[5, 6]);
    ps[1].files = vec![crate::model::Attachment { filename: "old.png".into(), ..crate::model::Attachment::at("http://x/old.png") }];
    app.set_cached_thread(Thread::answer(5, ps).unwrap(), 1);
    app.open_gallery();
    assert!(app.tab.gallery.is_some());
    // Last session's thread is gone: its catalog.
    app.handle(answer(req, thread_arrived, Err(gone())));
    assert_eq!(app.tab.view(), View::Catalog);
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(posts(&[7]))));
    app.tab.catalog_list.state.select(Some(0));
    app.enter();
    assert_eq!(app.tab.view(), View::Thread);
    assert!(app.tab.gallery.is_none(), "thread 7 opened inside thread 5's gallery: {:?}", app.tab.gallery.as_ref().map(|g| g.files.len()));
}

#[test]
fn a_catalog_missing_after_moving_to_another_site_blames_nothing_there() {
    let mut app = local_app();
    app.goto_str("a/x/");
    assert_eq!(app.tab.view(), View::Catalog);
    let req = app.tab.req().unwrap();
    app.goto_str("b");
    assert_eq!((app.tab.site, app.tab.view()), (1, View::Boards));
    app.handle(answer(req, App::catalog_arrived, Err(gone())));
    let status = app.status().map(|s| s.text.clone()).unwrap_or_default();
    assert!(app.tab.failed.is_none(), "b's Boards shows a's catalog failure: {:?} / {status}", app.tab.failed);
}

#[test]
fn searching_saved_threads_drops_the_load_it_left() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    app.goto_str("a/x/1");
    app.goto_str("saved foo");
    assert_eq!(app.tab.view(), View::Search);
    assert!(app.tab.loading().is_none(), "still loading over the search: {:?}", app.tab.loading());
}


#[test]
fn a_thread_that_fails_to_open_keeps_what_it_was_to_open_on() {
    let mut app = local_app();
    let place = crate::store::Place { view: "thread".into(), site: "a".into(), board: Some("x".into()), thread: Some(5), selected: Some(12), conversation: Some(10), ..Default::default() };
    app.go_to_place(&place);
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, Err(anyhow::anyhow!("connection refused"))));
    // Quitting now saves the place as it was asked for...
    let p = app.place();
    assert_eq!((p.thread, p.selected, p.conversation), (Some(5), Some(12), Some(10)));
    // ...and r tries it again on the same post, though not as last session's any more.
    app.act(Action::Reload);
    assert_eq!(app.tab.opening(), Opening { select: Some(12), conversation: Some(10), restoring: false });
}

#[test]
fn a_post_lookup_that_took_over_a_thread_still_loading_leaves_r_to_load_it() {
    let mut app = local_app();
    app.goto_str("a/x/5");
    // A post lookup (as `:` with a post's link starts) takes over the tab's load, and fails.
    app.tab.fake_load(50, "Looking up post 7", Then::Show);
    let board = Board { uri: "x".into(), title: String::new(), nsfw: None };
    app.handle(answer(50, |a, (b, r)| a.thread_found(0, b, 7, None, r), (board, Ok(None))));
    assert_eq!((app.tab.view(), app.tab.thread.is_none(), app.tab.loading()), (View::Thread, true, None));
    app.act(Action::Reload);
    assert_eq!(app.tab.loading(), Some("Loading thread 5"));
}

#[test]
fn going_to_a_post_from_the_settings_closes_them() {
    let mut app = app_with(
        "[[site]]\nname = \"a\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"x\"]\n\
         [[site]]\nname = \"arch\"\nkind = \"foolfuuka\"\nurl = \"http://localhost:3\"\nboards = [\"g\"]",
    );
    app.tab.navigate(View::Settings);
    app.goto_str("http://localhost:3/g/post/99/");
    assert_eq!((app.tab.view(), app.tab.loading()), (View::Sites, Some("Looking up post 99")));
}

#[test]
fn replies_to_the_open_thread_while_the_settings_are_open_count_as_new() {
    let mut app = local_app();
    app.goto_str("a/x/1");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts(&[1, 2]))));
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.store.watch(key.clone(), String::new(), 2, 2);
    app.tab.navigate(View::Settings);
    // Refreshed behind the settings, as a watched thread: No.3 isn't seen yet.
    refresh(&mut app, key.clone(), posts(&[1, 2, 3]));
    assert_eq!(app.store.watched(&key).unwrap().status, Status::Live { unread: 1, replies: 0 });
    assert_eq!(app.tab.thread.as_ref().unwrap().posts.len(), 2);
}

#[test]
fn a_thread_from_an_overboard_goes_back_to_it_after_a_search() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.navigate(View::Catalog);
    app.load_catalog(None);
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(vec![Post { no: 5, board: Some("xy".into()), ..Default::default() }])));
    app.enter();
    assert_eq!(app.tab.board.as_ref().unwrap().uri, "xy");
    // To the saved threads' search and back: the thread still came from the overboard.
    app.goto_str("saved foo");
    app.back();
    assert_eq!(app.tab.view(), View::Thread);
    app.back();
    assert_eq!((app.tab.view(), app.tab.board.as_ref().unwrap().uri.as_str(), app.tab.catalog.len()), (View::Catalog, "x", 1));
}

#[test]
fn a_catalog_that_fails_to_load_keeps_the_thread_it_was_to_select() {
    let mut app = local_app();
    let place = crate::store::Place { view: "catalog".into(), site: "a".into(), board: Some("x".into()), selected: Some(123), ..Default::default() };
    app.go_to_place(&place);
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Err(anyhow::anyhow!("connection refused"))));
    // Quitting now saves it, and r selects it once the catalog comes.
    assert_eq!(app.place().selected, Some(123));
    app.act(Action::Reload);
    app.handle(answer(app.tab.req().unwrap(), App::catalog_arrived, Ok(posts(&[1, 123]))));
    assert_eq!(app.place().selected, Some(123));
}

/// A data directory and a config file ck can't write: both sit under a plain file.
fn unwritable(app: &mut App, dir: &std::path::Path) {
    let blocked = dir.join("blocked");
    std::fs::write(&blocked, "a file, not a folder").unwrap();
    app.store = Store::load(Some(blocked.join("data"))).0;
    app.config_path = Some(blocked.join("config.toml"));
    assert!(app.edit_config(|_| Ok(())).is_err(), "the config write has to fail for these tests to mean anything");
}

/// The footer's message: what it says, and whether it's an error.
fn footer(app: &App) -> (String, bool) {
    app.footer.get().map(|s| (s.text.clone(), s.error)).unwrap_or_default()
}

#[test]
fn a_hide_that_cant_be_saved_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    unwritable(&mut app, dir.path());
    app.tab.board = Some(Board { uri: "x".into(), title: String::new(), nsfw: None });
    app.tab.catalog_board = Some("x".into());
    app.tab.catalog = nos(&[1, 2]);
    app.remark();
    app.tab.navigate(View::Catalog);
    app.tab.catalog_list.state.select(Some(0));
    app.act(Action::Hide);
    let (text, error) = footer(&app);
    assert!(app.store.save().is_err(), "the hide wasn't written");
    assert!(error && text.contains("Couldn't save"), "the failed write isn't reported: {text:?} (error: {error})");
}

#[test]
fn a_sort_or_layout_that_cant_be_saved_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    unwritable(&mut app, dir.path());
    app.goto_str("a/x");
    app.act(Action::Sort);
    let (text, error) = footer(&app);
    assert!(app.store.save().is_err(), "the sort wasn't written");
    assert!(error && text.contains("Couldn't save"), "sort: the failed write isn't reported: {text:?} (error: {error})");
    app.act(Action::Compact);
    let (text, error) = footer(&app);
    assert!(error && text.contains("Couldn't save"), "layout: the failed write isn't reported: {text:?} (error: {error})");
}

#[test]
fn board_images_that_cant_be_saved_say_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    unwritable(&mut app, dir.path());
    app.goto_str("a/x");
    app.toggle_board_images();
    let (text, error) = footer(&app);
    assert!(app.store.save().is_err(), "the image setting wasn't written");
    assert!(error && text.contains("Couldn't save"), "toggle: the failed write isn't reported: {text:?} (error: {error})");
    app.reset_board_images("a/x");
    let (text, error) = footer(&app);
    assert!(error && text.contains("Couldn't save"), "reset: the failed write isn't reported: {text:?} (error: {error})");
}

#[test]
fn a_default_layout_kept_nowhere_doesnt_claim_it_was_kept() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    unwritable(&mut app, dir.path());
    app.cycle_default_layout();
    let (text, error) = footer(&app);
    assert!(app.store.save().is_err(), "the layout wasn't kept in the data directory either");
    assert!(error, "neither the config nor the data directory could be written, but it's an info: {text:?}");
    assert!(!text.contains("kept in the data directory"), "the data directory couldn't be written either: {text:?}");
}

#[test]
fn a_hidden_word_that_cant_be_saved_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    unwritable(&mut app, dir.path());
    app.add_hidden_word("spam");
    let (text, error) = footer(&app);
    assert!(error, "the config write failed, but it's an info (2s, not error-styled): {text:?}");
    // Removing one says the same failure the same way.
    app.remove_hidden_word("spam", true);
    let (removed, _) = footer(&app);
    assert!(removed.contains("couldn't save it") && text.contains("couldn't save it"), "two wordings for one failure: {text:?} / {removed:?}");
}

#[test]
fn taking_back_what_couldnt_be_written_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    unwritable(&mut app, dir.path());
    app.add_hidden_word("spam");
    drawn(&mut app);
    app.undo_filter();
    assert_eq!(footer(&app), ("Posts with \"spam\" aren't hidden now".into(), false), "it was never written, so nothing failed to be");
}

#[test]
fn an_error_behind_the_spinner_waits_to_be_seen() {
    let mut app = test_app();
    let t0 = Instant::now();
    app.clock = Clock { instant: Some(t0), ..Default::default() };
    app.tab.fake_load(1, "Loading", Then::Show);
    app.error("Couldn't save to the data directory: disk full");
    app.poll();
    app.clock = Clock { instant: Some(t0 + Duration::from_secs(60)), ..Default::default() };
    app.poll();
    assert!(footer(&app).1, "it ran out while the spinner hid it");
    // The load is answered: the spinner goes.
    assert!(app.tab.answered(1));
    app.poll();
    app.clock = Clock { instant: Some(t0 + Duration::from_secs(65)), ..Default::default() };
    app.poll();
    assert_eq!(app.footer.get(), None, "5s once it's on screen");
}

#[test]
fn a_board_added_and_opened_but_not_saved_still_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = local_app();
    unwritable(&mut app, dir.path());
    app.popup = Some(Popup::Adding(Adding::Board { site: 0, board: "zz".into(), open: Some("a/zz".into()) }));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let (text, error) = footer(&app);
    assert!(error && text.contains("couldn't save it"), "opening the board replaced the config error: {text:?} (error: {error})");
}

#[test]
fn an_unshown_error_isnt_replaced_by_the_next_info() {
    let mut app = test_app();
    app.error("Couldn't save watched threads: disk full");
    app.info("Sorted by replies");
    let (text, error) = footer(&app);
    assert!(error && text.contains("disk full"), "an info replaced an error before it was drawn: {text:?}");
}

/// A request that couldn't connect, as the HTTP layer reports it.
fn unreachable_request() -> anyhow::Error {
    anyhow::Error::from(ureq::Error::ConnectionFailed).context("GET http://127.0.0.1:3/x/res/1.json")
}

#[test]
fn a_thread_lookup_that_fails_is_said_in_plain_words() {
    let plain = crate::http::plain(&unreachable_request());
    assert_eq!(plain, "Couldn't reach 127.0.0.1:3");
    // Finding which thread a post is in.
    let mut app = local_app();
    app.thread_found(0, Board { uri: "x".into(), title: String::new(), nsfw: None }, 5, None, Err(unreachable_request()));
    assert_eq!(footer(&app).0, plain, "find_thread");
}

#[test]
fn a_site_that_cant_be_asked_is_said_in_plain_words() {
    let plain = crate::http::plain(&unreachable_request());
    let mut app = local_app();
    app.popup = Some(Popup::Adding(Adding::Looking { id: 7, host: "127.0.0.1".into(), open: None }));
    app.detected(7, Err(unreachable_request()));
    assert_eq!(footer(&app).0, plain, "adding a site");
}

#[test]
fn an_archive_search_that_fails_is_said_in_plain_words() {
    let plain = crate::http::plain(&unreachable_request());
    let mut app = app_with(
        "[[site]]\nname = \"chan\"\nkind = \"vichan\"\nurl = \"http://127.0.0.1:3\"\nboards = [\"g\"]\narchive = \"arch\"\n\
         [[site]]\nname = \"arch\"\nkind = \"foolfuuka\"\nurl = \"http://localhost:3\"\nboards = [\"g\"]",
    );
    app.tab.board = Some(Board { uri: "g".into(), title: String::new(), nsfw: None });
    app.tab.navigate(View::Catalog);
    app.act(Action::ArchiveSearch);
    type_text(&mut app, "borrow");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    app.search_results(1, Err(unreachable_request()));
    assert_eq!(footer(&app).0, plain, "archive search");
}

#[test]
fn a_watched_thread_answered_without_posts_keeps_its_counts() {
    let mut app = local_app();
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.store.watch(key.clone(), "One".into(), 2, 2);
    refresh(&mut app, key.clone(), nos(&[1, 2, 3, 4]));
    let before = app.store.watched(&key).unwrap().status;
    assert_eq!(before.counts(), (2, 0));
    // A site in trouble answers with no posts: that's no answer, not a thread without any.
    app.refreshed(key.clone(), Thread::answer(1, Vec::new()));
    let w = app.store.watched(&key).unwrap();
    assert_eq!((w.status, w.posts), (before, 4));
}

#[test]
fn a_watched_thread_answered_without_posts_tells_nothing_twice() {
    let mut app = local_app();
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.store.watch(key.clone(), "One".into(), 2, 2);
    refresh(&mut app, key.clone(), nos(&[1, 2, 3, 4]));
    app.refreshed(key.clone(), Thread::answer(1, Vec::new()));
    // The next good answer has nothing new: posts already told about aren't told again.
    refresh(&mut app, key, nos(&[1, 2, 3, 4]));
    app.flush_notes(Instant::now());
    assert!(app.notified.is_empty(), "{:?}", app.notified);
}

#[test]
fn a_watched_thread_answered_with_another_thread_is_left_as_it_was() {
    let mut app = local_app();
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.store.watch(key.clone(), "One".into(), 2, 2);
    refresh(&mut app, key.clone(), nos(&[1, 2]));
    // The site answers with thread 7: none of its posts are thread 1's.
    app.refreshed(key.clone(), Thread::answer(1, nos(&[7, 8, 9])));
    app.flush_notes(Instant::now());
    let w = app.store.watched(&key).unwrap();
    assert_eq!((w.status.counts(), w.posts), ((0, 0), 2));
    assert!(app.notified.is_empty(), "{:?}", app.notified);
}

#[test]
fn an_answer_with_another_thread_does_not_open_it() {
    let mut app = local_app();
    app.goto_str("a/x/5");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, Thread::answer(5, nos(&[7, 8]))));
    // Thread 5 was asked for; thread 7 isn't shown (or visited) as if it were.
    assert_ne!(app.tab.thread.as_ref().map(|t| t.no), Some(7));
    assert!(app.store.history.iter().all(|v| v.key.no != 7));
}

#[test]
fn a_cut_short_answer_does_not_replace_a_watched_threads_saved_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    let all: Vec<u64> = (1..=10).collect();
    app.store.watch(key.clone(), "One".into(), 10, 10);
    refresh(&mut app, key.clone(), nos(&all));
    app.flush_writes();
    assert_eq!(app.store.saved(&key).unwrap().posts, 10);
    // A refresh with fewer than half the posts is a broken answer when the thread is open;
    // in the background it's the same answer, and the copy kept for when the thread dies
    // shouldn't become it.
    refresh(&mut app, key.clone(), nos(&[1, 11]));
    app.flush_writes();
    assert!(app.store.saved(&key).unwrap().posts >= 10, "{}", app.store.saved(&key).unwrap().posts);
    app.refreshed(key.clone(), Err(gone()));
    assert!(app.store.load_saved(&key).unwrap().posts.len() >= 10);
}

#[test]
fn a_second_answer_just_as_short_is_taken() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.store.watch(key.clone(), "One".into(), 10, 10);
    refresh(&mut app, key.clone(), posts_upto(10));
    app.flush_writes();
    // Cut short: not counted (or kept), but what's known of the thread is now its length...
    refresh(&mut app, key.clone(), nos(&[1, 11, 12]));
    app.flush_writes();
    let counted = |app: &App| (app.store.watched(&key).unwrap().status.counts(), app.store.saved(&key).unwrap().posts);
    assert_eq!(counted(&app), ((0, 0), 10));
    // ...so another answer just as small is taken: moderators did delete most of it.
    refresh(&mut app, key.clone(), nos(&[1, 11, 12]));
    app.flush_writes();
    assert_eq!(counted(&app), ((2, 0), 3));
    // And so is one with half the posts or more.
    refresh(&mut app, key.clone(), nos(&[1, 13]));
    app.flush_writes();
    assert_eq!(counted(&app).1, 2);
}

#[test]
fn a_cut_short_refresh_of_the_open_thread_changes_no_count_or_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(&dir.path().join("data"), 1000);
    app.download_dir = Some(dir.path().join("dl").display().to_string());
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.store.watch(key.clone(), "One".into(), 10, 10);
    app.goto_str("a/x/1");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts_upto(10))));
    app.flush_writes();
    refresh(&mut app, key.clone(), nos(&[1, 11]));
    // Shown as it came, and said so; the watch list and the copy are as they were.
    assert_eq!(app.tab.thread.as_ref().unwrap().posts.len(), 2);
    assert_eq!(app.store.watched(&key).unwrap().posts, 10);
    assert!(app.status().unwrap().text.contains("2 of the 10 posts"), "{:?}", app.status());
    // What's shown isn't kept either: exported, watched again, or a post marked as yours.
    app.act(Action::Export);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(dir.path().join("dl/thread.json").exists() && !app.status().unwrap().text.contains("(and in Saved)"));
    app.act(Action::Watch);
    app.act(Action::Watch);
    app.act(Action::Mine);
    app.flush_writes();
    assert_eq!((app.store.saved(&key).unwrap().posts, app.store.load_saved(&key).unwrap().posts.len()), (10, 10));
}

/// Thread /x/1 open with posts 1..=10 (unwatched), and its key.
fn ten_open() -> (App, ThreadKey) {
    let mut app = local_app();
    app.goto_str("a/x/1");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts_upto(10))));
    let key = app.key("x", 1);
    (app, key)
}

#[test]
fn answers_cut_short_again_are_told_against_the_posts_last_known_whole() {
    let (mut app, key) = ten_open();
    refresh(&mut app, key.clone(), nos(&[1, 2, 3, 4]));
    refresh(&mut app, key, nos(&[1]));
    assert!(app.status().unwrap().text.contains("1 of the 10 posts"), "{:?}", app.status());
    assert_eq!(app.tab.thread.as_ref().unwrap().known, 10);
}

#[test]
fn watching_a_thread_shown_cut_short_counts_the_posts_known() {
    let (mut app, key) = ten_open();
    refresh(&mut app, key.clone(), nos(&[1, 11]));
    app.act(Action::Watch);
    assert_eq!(app.store.watched(&key).unwrap().posts, 10);
}

#[test]
fn an_answer_cut_short_long_ago_is_not_the_bar_for_opening_it_again() {
    let (mut app, key) = ten_open();
    refresh(&mut app, key.clone(), nos(&[1, 11, 12]));
    // Left, watched later (known with 10 posts), and opened again to a broken answer: it's
    // judged against those 10, not the 3 of before.
    app.goto_str("a/x/5");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(nos(&[5]))));
    app.store.watch(key.clone(), "One".into(), 10, 10);
    app.goto_str("a/x/1");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(nos(&[1, 13, 14, 15]))));
    assert_eq!(app.store.watched(&key).unwrap().posts, 10);
}

#[test]
fn a_mass_deletion_on_the_open_thread_is_taken_and_stays_taken() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = saving_app(dir.path(), 1000);
    let key = ThreadKey { site: "a".into(), board: "x".into(), no: 1 };
    app.store.watch(key.clone(), "One".into(), 10, 10);
    app.goto_str("a/x/1");
    app.handle(answer(app.tab.req().unwrap(), thread_arrived, arrived(posts_upto(10))));
    // Moderators delete all but two posts: the first answer is doubted, the second taken.
    refresh(&mut app, key.clone(), nos(&[1, 2]));
    refresh(&mut app, key.clone(), nos(&[1, 2]));
    app.flush_writes();
    // Taken is taken: the thread is now known with two posts, and the same answer again
    // isn't doubted (it brought nothing new, so it isn't a visit either).
    app.footer = Footer::default();
    refresh(&mut app, key.clone(), nos(&[1, 2]));
    assert!(app.status().is_none(), "{:?}", app.status());
    assert_eq!((app.tab.thread.as_ref().unwrap().known, app.store.watched(&key).unwrap().posts), (2, 2));
    assert_eq!(app.store.saved(&key).unwrap().posts, 2);
}

#[test]
fn a_thread_opened_again_shows_what_came_since_as_new() {
    let mut app = local_app();
    for (no, posts) in [(1, nos(&[1, 2])), (5, nos(&[5])), (1, nos(&[1, 2, 3]))] {
        app.goto_str(&format!("a/x/{no}"));
        app.handle(answer(app.tab.req().unwrap(), thread_arrived, Thread::answer(no, posts)));
    }
    // New since the last visit, which this one is (counted once it's shown).
    assert_eq!(app.tab.thread.as_ref().map(|t| t.new_after), Some(2));
    assert_eq!(app.store.last_seen(&app.key("x", 1)), 3);
}

/// A thread's answer is for the thread asked for: on a tab that's on another site by then
/// (switched under the load), it isn't shown, counted or visited as that site's thread.
#[test]
fn a_thread_answered_after_its_tab_changed_site_isnt_taken_for_that_sites() {
    let mut app = local_app();
    let other = ThreadKey { site: "b".into(), board: "x".into(), no: 1 };
    app.store.watch(other.clone(), String::new(), 1, 1);
    app.goto_str("a/x/1");
    let (req, asked) = (app.tab.req().unwrap(), app.key("x", 1));
    app.switch_site(1);
    let before = (app.store.watched(&other).unwrap().posts, app.store.last_seen(&other));
    app.handle(Msg::answer(req, arrived(nos(&[1, 2, 3])), move |app, r| app.thread_arrived(&asked, r)));
    assert!(app.tab.thread.is_none(), "a/x/1 shown as b/x/1");
    assert_eq!((app.store.watched(&other).unwrap().posts, app.store.last_seen(&other)), before);
}

/// Moving to another thread shows the same view, but what the last frame drew is stale: no
/// click lands on it before the next frame.
#[test]
fn a_click_after_moving_to_another_thread_waits_for_the_next_frame() {
    let mut app = thread_app();
    app.set_thread(nos(&(1..=60).collect::<Vec<_>>()));
    draw_at(&mut app, 80, 20);
    assert!(app.drawn().is_some_and(|d| d.body.is_some()));
    // Another thread, as a link followed opens it (same view, same tab).
    app.tab.navigate(View::Thread);
    assert!(app.drawn().is_none(), "the last thread's posts as drawn are still clickable");
    app.set_thread(nos(&(100..=160).collect::<Vec<_>>()));
    draw_at(&mut app, 80, 20);
    assert!(app.drawn().is_some_and(|d| d.body.is_some()));
}

/// Opening the settings over a view, or closing them, makes the last frame stale.
#[test]
fn a_click_after_the_settings_open_or_close_waits_for_the_next_frame() {
    let mut app = local_app();
    app.switch_site(0);
    app.tab.navigate(View::Boards);
    draw_at(&mut app, 80, 20);
    assert!(app.drawn().is_some());
    app.tab.navigate(View::Settings);
    assert!(app.drawn().is_none(), "the boards drawn are clickable over the settings");
    draw_at(&mut app, 80, 20);
    assert!(app.drawn().is_some());
    app.tab.close_settings();
    assert!(app.drawn().is_none(), "the settings drawn are clickable over the boards");
}
