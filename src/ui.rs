//! Drawing, in a flat style: no lines or boxes. Regions are told apart by background tone
//! (cards a step above the background, raised panels another), by spacing, and by type.
//! Every color comes from the current theme's roles.

use chrono::{Local, TimeZone, Utc};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, ListState};
use ratatui_image::Image;
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, Clock, Hit, LinkItem, SETTING_SECTIONS, SettingsPopup, SiteRow, Sort, ThreadLayout, ThreadView, View, key_rows,
    setting_rows,
};
use crate::http;
use crate::images::{Images, Kind, State};
use crate::keys::{self, Action, KeyMap};
use crate::config::CatalogLayout;
use crate::filter::Mark;
use crate::markup;
use crate::model::{Attachment, Post};
use crate::theme::{self, ROLES, Theme, theme};

const SPINNER: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
/// Thumbnail sizes in cells (roughly square at a 1:2 cell aspect).
const THUMB: Size = Size::new(16, 8);
const CAT_THUMB: Size = Size::new(10, 4);
/// Below this width there's no room for thumbnails next to text.
const MIN_THUMB_WIDTH: u16 = 60;
/// Space on each side of the content, and inside cards before the text (the first column of
/// which holds the selection stripe).
const MARGIN: u16 = 2;
const PAD: u16 = 2;
/// How far in each level of replies shown inline (`e`) sits.
const INDENT: u16 = 4;

// ----- small helpers -----

fn dim() -> Style {
    Style::new().fg(theme().text_dim)
}

fn bold(c: Color) -> Style {
    Style::new().fg(c).add_modifier(Modifier::BOLD)
}

/// Paint an area's background.
fn fill(f: &mut Frame, area: Rect, bg: Color) {
    let area = area.intersection(f.area());
    f.buffer_mut().set_style(area, Style::new().bg(bg));
}

/// Draw a line at `(x, y)`, at most `w` wide, over whatever background is there.
fn put(f: &mut Frame, x: u16, y: u16, w: u16, line: Line) {
    let r = Rect::new(x, y, w, 1).intersection(f.area());
    if !r.is_empty() {
        f.render_widget(line, r);
    }
}

/// A small label with its own background.
fn chip(text: impl Into<String>, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(format!(" {} ", text.into()), Style::new().fg(fg).bg(bg))
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// A dim message in the middle of an area.
fn empty(f: &mut Frame, area: Rect, msg: &str) {
    if !msg.is_empty() {
        put(f, area.x, area.y + area.height / 3, area.width, Line::styled(msg.to_string(), dim()).centered());
    }
}

/// Map every 24-bit color to the nearest of 256, for terminals without 24-bit color.
fn downgrade(buf: &mut Buffer) {
    for cell in buf.content.iter_mut() {
        cell.fg = theme::to_256(cell.fg);
        cell.bg = theme::to_256(cell.bg);
    }
}

// ----- the frame -----

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = theme();
    let all = f.area();
    f.buffer_mut().set_style(all, Style::new().fg(t.text).bg(t.background));
    if app.viewer.is_some() {
        draw_viewer(f, app);
    } else {
        let [bar, gap, body, footer] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
                .areas(f.area());
        draw_app_bar(f, app, bar);
        draw_tab_row(f, app, gap.inner(Margin::new(MARGIN, 0)));
        let content = body.inner(Margin::new(MARGIN, 0));
        match app.view {
            View::Sites => draw_sites(f, app, content),
            View::Boards => draw_boards(f, app, content),
            View::Catalog => draw_catalog(f, app, content),
            View::Thread if app.gallery.is_some() => draw_gallery(f, app, content),
            View::Thread => draw_thread(f, app, content),
            View::Watched => draw_watched(f, app, content),
            View::History => draw_history(f, app, content),
            View::Settings => draw_settings(f, app, content),
            View::Search => draw_search(f, app, content),
        }
        draw_footer(f, app, footer);
        if app.preview.is_some() {
            draw_preview(f, app);
        }
        if app.links.is_some() {
            draw_links(f, app);
        }
        if app.settings.popup.is_some() {
            draw_settings_popup(f, app);
        }
        if app.show_help {
            draw_help(f, app);
        }
    }
    if app.image_search_panel.is_some() {
        draw_image_search(f, app);
    }
    app.images.end_frame();
    if !app.truecolor {
        downgrade(f.buffer_mut());
    }
}

/// With more than one tab, their chips in the row under the bar (the current one stands
/// out); clicking one switches to it.
fn draw_tab_row(f: &mut Frame, app: &mut App, area: Rect) {
    app.tab_chips.clear();
    let n = app.tabs.len();
    if n < 2 {
        return;
    }
    let t = theme();
    let each = (area.width as usize / n).clamp(8, 28);
    let mut x = area.x;
    for i in 0..n {
        let label = app.tab_label(i);
        let text = format!(" {} {} ", i + 1, truncate(&label, each.saturating_sub(5)));
        let w = (text.width() as u16).min(area.right().saturating_sub(x));
        if w == 0 {
            break;
        }
        let style = if i == app.active { bold(t.on_primary_container).bg(t.primary_container) } else { Style::new().fg(t.text_dim).bg(t.surface_high) };
        let r = Rect::new(x, area.y, w, 1);
        put(f, x, area.y, w, Line::styled(text, style));
        app.tab_chips.push((r, i));
        x += w + 1;
    }
}

/// Where you are (left) and what's here (right), on a solid bar.
fn draw_app_bar(f: &mut Frame, app: &App, area: Rect) {
    let t = theme();
    fill(f, area, t.bar);
    let (crumbs, meta) = location(app);
    let mut spans = vec![Span::styled(" ck ", bold(t.on_primary).bg(t.primary)), Span::raw(" ")];
    let meta_w = meta.iter().map(|s| s.width()).sum::<usize>() as u16;
    let left_w = area.width.saturating_sub(meta_w + 2);
    // The last crumb (the most specific) gives way, with an ellipsis, when space is short.
    let n = crumbs.len();
    let before: usize = 5 + crumbs.iter().take(n.saturating_sub(1)).map(|c| c.width() + 5).sum::<usize>();
    for (i, c) in crumbs.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ›  ", dim()));
        }
        if i + 1 == n {
            spans.push(Span::styled(truncate(&c, (left_w as usize).saturating_sub(before)), bold(t.on_bar)));
        } else {
            spans.push(Span::styled(c, Style::new().fg(t.on_bar)));
        }
    }
    put(f, area.x, area.y, left_w, Line::from(spans));
    put(f, area.x + left_w, area.y, area.width - left_w, Line::from(meta).right_aligned());
}

/// Breadcrumbs, and right-aligned facts about the current view.
fn location(app: &App) -> (Vec<String>, Vec<Span<'static>>) {
    let t = theme();
    let site = || app.current_site().cfg.name.clone();
    let board = || {
        app.board.as_ref().map(|b| if b.title.is_empty() { format!("/{}/", b.uri) } else { format!("/{}/  {}", b.uri, b.title) })
    };
    let mut meta: Vec<String> = Vec::new();
    let crumbs = match app.view {
        View::Sites => {
            meta.push(plural(app.sites.len(), "site"));
            vec!["Sites".to_string()]
        }
        View::Boards => {
            meta.push(plural(app.boards().len(), "board"));
            vec![site(), "Boards".into()]
        }
        View::Catalog => {
            meta.push(plural(app.catalog.len(), "thread"));
            let new = app.catalog.iter().filter(|p| app.catalog_new.contains(&p.no)).count();
            if new > 0 {
                meta.push(format!("{new} new"));
            }
            let hidden = app.catalog_marks.iter().filter(|m| m.hidden.is_some()).count();
            if hidden > 0 {
                meta.push(if app.show_hidden { format!("{hidden} hidden, shown") } else { format!("{hidden} hidden") });
            }
            if app.catalog_sort != Sort::Bump {
                meta.push(app.catalog_sort.label().into());
            }
            vec![site(), board().unwrap_or_default()]
        }
        View::Thread => {
            let th = app.thread.as_ref();
            let subject = th.and_then(|th| th.posts.first()?.subject.clone()).unwrap_or_else(|| {
                th.map_or("Thread".into(), |th| format!("Thread {}", th.no))
            });
            if let Some(th) = th {
                meta.push(plural(th.posts.len(), "post"));
                let new = (0..th.posts.len()).filter(|&i| th.is_new(i)).count();
                if new > 0 {
                    meta.push(format!("{new} new"));
                }
                if th.reveal_all {
                    meta.push("spoilers shown".into());
                }
            }
            let uri = app.board.as_ref().map(|b| format!("/{}/", b.uri)).unwrap_or_default();
            if let Some(g) = &app.gallery {
                meta.insert(0, plural(g.files.len(), "file"));
                vec![site(), uri, truncate(&subject, 40), "Files".into()]
            } else {
                vec![site(), uri, truncate(&subject, 48)]
            }
        }
        View::Watched => {
            let unread: usize = app.store.watched.iter().map(|w| w.unread).sum();
            meta.push(plural(app.store.watched.len(), "thread"));
            if unread > 0 {
                meta.push(format!("{unread} new"));
            }
            vec!["Watched".into()]
        }
        View::History => {
            meta.push(plural(app.store.history.len(), "thread"));
            vec!["History".into()]
        }
        View::Settings => vec!["Settings".into()],
        View::Search => {
            let Some(s) = &app.search else { return (vec!["Search".into()], Vec::new()) };
            match s.total {
                Some(t) => meta.push(format!("{} of {}", s.hits.len(), plural(t as usize, "result"))),
                None => meta.push(plural(s.hits.len(), "result")),
            }
            vec![site(), format!("/{}/", s.board), format!("Search: {}", truncate(&s.query, 40))]
        }
    };
    let mut spans: Vec<Span> = Vec::new();
    // An active filter or search, unless it's being typed (the footer shows that).
    let query = match app.view {
        View::Thread => app.thread.as_ref().filter(|th| !th.search.is_empty() && !app.searching).map(|th| {
            let k = th.matches.len();
            format!("/{}  {}", th.search, plural(k, "match").replace("matchs", "matches"))
        }),
        _ => Some(current_filter(app)).filter(|q| !q.is_empty() && !app.filtering).map(|q| format!("/{q}")),
    };
    if let Some(q) = query {
        spans.push(chip(q, t.on_primary_container, t.primary_container));
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(meta.join("  ·  "), dim()));
    spans.push(Span::raw(" "));
    (crumbs, spans)
}

/// Key hints, input, or status, on a solid bar; background work shows at the right.
fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let t = theme();
    fill(f, area, t.bar);
    let typing = if let Some(g) = &app.goto {
        Some(("go to", g.as_str()))
    } else if let Some(q) = &app.search_input {
        Some(("search the archive", q.as_str()))
    } else if app.searching {
        Some(("search", app.thread.as_ref().map_or("", |th| th.search.as_str())))
    } else if app.filtering {
        Some(("filter", current_filter(app)))
    } else {
        None
    };
    let line = if let Some((what, q)) = typing {
        Line::from(vec![
            Span::raw(" "),
            chip(what, t.on_primary, t.primary),
            Span::styled(format!(" {q}"), Style::new().fg(t.on_bar)),
            Span::styled("▏", Style::new().fg(t.primary)),
            Span::styled(
                match (&app.goto, &app.status) {
                    // Tab completion's candidates.
                    (Some(_), Some((msg, false))) => format!("   {msg}"),
                    (Some(_), _) => "   enter go   tab complete   esc cancel".into(),
                    _ => "   enter accept   esc clear".into(),
                },
                dim(),
            ),
        ])
    } else if let Some(label) = &app.loading {
        Line::from(vec![
            Span::styled(format!(" {} ", SPINNER[app.tick % SPINNER.len()]), bold(t.primary)),
            Span::styled(format!("{label}…"), Style::new().fg(t.on_bar)),
        ])
    } else if let Some((msg, is_err)) = &app.status {
        let (mark, bg) = if *is_err { ("!", t.error) } else { ("✓", t.success) };
        Line::from(vec![
            Span::raw(" "),
            Span::styled(format!(" {mark} "), bold(t.background).bg(bg)),
            Span::styled(format!(" {msg}"), Style::new().fg(t.on_bar)),
        ])
    } else {
        let mut spans = vec![Span::raw(" ")];
        for (key, label) in footer_hints(app) {
            spans.push(Span::styled(key, bold(t.primary)));
            spans.push(Span::styled(format!(" {label}   "), dim()));
        }
        Line::from(spans)
    };
    let d = &app.downloads;
    let mut right = Vec::new();
    if d.running > 0 {
        right.push(chip(format!("⇣ {}/{}", d.done + d.skipped + d.failed, d.total), t.text, t.surface_high));
        right.push(Span::raw(" "));
    }
    if !app.refreshing.is_empty() {
        right.push(chip(format!("↻ {}", app.refreshing.len()), t.text, t.surface_high));
        right.push(Span::raw(" "));
    }
    let right_w = right.iter().map(|s| s.width()).sum::<usize>() as u16;
    put(f, area.x, area.y, area.width.saturating_sub(right_w), line);
    put(f, area.right().saturating_sub(right_w), area.y, right_w, Line::from(right));
}

/// Key hints for the footer, with the configured keys.
fn footer_hints(app: &App) -> Vec<(String, &'static str)> {
    let k = |a| app.keys.key(a).to_string();
    let mut hints: Vec<(String, &'static str)> = match app.view {
        View::Thread if app.gallery.is_some() => vec![
            ("h/j/k/l".into(), "move"),
            ("enter".into(), "view"),
            (k(Action::Download), "save"),
            (k(Action::Copy), "copy URL"),
            ("esc".into(), "back to the post"),
        ],
        View::Thread => vec![
            ("j/k".into(), "post"),
            ("enter".into(), "quote"),
            (k(Action::Preview), "preview"),
            (k(Action::JumpBack), "back"),
            (k(Action::Search), "search"),
            (k(Action::Unread), "unread"),
            (k(Action::View), "view"),
            (k(Action::Watch), "watch"),
        ],
        View::Catalog => vec![
            ("enter".into(), "open"),
            (k(Action::Search), "filter"),
            (k(Action::View), "view"),
            (k(Action::Watch), "watch"),
            (k(Action::Sort), "sort"),
            (k(Action::Compact), "compact"),
            (k(Action::Reload), "reload"),
        ],
        View::Watched | View::History => vec![
            ("enter".into(), "open"),
            (k(Action::Remove), "remove"),
            (k(Action::Search), "filter"),
            (k(Action::Browser), "browser"),
        ],
        View::Settings => vec![("enter".into(), "change"), ("esc".into(), "back")],
        View::Search => vec![("enter".into(), "open the thread"), (k(Action::NextMatch), "more results"), ("esc".into(), "back")],
        _ => vec![
            ("enter".into(), "open"),
            (k(Action::Search), "filter"),
            (k(Action::Browser), "browser"),
            (k(Action::Reload), "reload"),
        ],
    };
    hints.push((k(Action::Settings), "settings"));
    hints.push((k(Action::Help), "help"));
    hints
}

fn current_filter(app: &App) -> &str {
    match app.view {
        View::Sites => &app.site_list.filter,
        View::Boards => &app.board_list.filter,
        View::Catalog => &app.catalog_list.filter,
        View::Watched => &app.watched_list.filter,
        View::History => &app.history_list.filter,
        View::Thread | View::Settings | View::Search => "",
    }
}

// ----- lists and cards -----

/// Draw items `height` rows tall with `gap` rows of background between them. With `card`
/// each item sits on that color; the selected one gets the selection color and an accent
/// stripe, as do items marked in `stripes` (highlighted by a filter). Returns where it
/// went, for mouse clicks.
#[allow(clippy::too_many_arguments)]
fn draw_rows(
    f: &mut Frame,
    area: Rect,
    items: Vec<Vec<Line<'static>>>,
    state: &mut ListState,
    height: u16,
    gap: u16,
    card: Option<Color>,
    stripes: &[bool],
) -> Option<Hit> {
    let mut items: Vec<Option<Vec<Line<'static>>>> = items.into_iter().map(Some).collect();
    let n = items.len();
    draw_rows_with(f, area, n, &mut |k| items[k].take().unwrap_or_default(), state, height, gap, card, stripes)
}

/// `draw_rows` for long lists: `build` makes only the items that are on screen.
#[allow(clippy::too_many_arguments)]
fn draw_rows_with(
    f: &mut Frame,
    area: Rect,
    count: usize,
    build: &mut dyn FnMut(usize) -> Vec<Line<'static>>,
    state: &mut ListState,
    height: u16,
    gap: u16,
    card: Option<Color>,
    stripes: &[bool],
) -> Option<Hit> {
    if count == 0 || area.is_empty() {
        return None;
    }
    let t = theme();
    let per = height + gap;
    let fit = ((area.height + gap) / per).max(1) as usize;
    let sel = state.selected().unwrap_or(0).min(count - 1);
    let mut off = state.offset().min(count - 1);
    if sel < off {
        off = sel;
    } else if sel >= off + fit {
        off = sel + 1 - fit;
    }
    *state.offset_mut() = off;
    for k in off..count {
        let y = area.y + (k - off) as u16 * per;
        if y >= area.bottom() {
            break;
        }
        let lines = build(k);
        let h = height.min(area.bottom() - y);
        let row = Rect::new(area.x, y, area.width, h);
        let selected = k == sel;
        if selected {
            fill(f, row, t.selection);
            fill(f, Rect::new(row.x, y, 1, h), t.primary);
        } else if let Some(c) = card {
            fill(f, row, c);
        }
        if stripes.get(k).copied().unwrap_or(false) {
            fill(f, Rect::new(row.x, y, 1, h), t.primary);
        }
        for (r, line) in lines.into_iter().take(h as usize).enumerate() {
            put(f, row.x + PAD, y + r as u16, row.width.saturating_sub(PAD + 1), line);
        }
    }
    Some(Hit::List { area, offset: off, item_height: per })
}

/// Left and right parts of a line, the right one pushed to `width`.
fn spread(mut left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let lw: usize = left.iter().map(|s| s.width()).sum();
    let rw: usize = right.iter().map(|s| s.width()).sum();
    if lw + rw + 2 <= width {
        left.push(Span::raw(" ".repeat(width - lw - rw)));
        left.extend(right);
    }
    Line::from(left)
}

fn draw_sites(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let width = area.width.saturating_sub(PAD + 1) as usize;
    let items: Vec<Vec<Line>> = app
        .visible_sites()
        .into_iter()
        .map(|row| match row {
            SiteRow::Watched => {
                let n = app.store.watched.len();
                let unread: usize = app.store.watched.iter().map(|w| w.unread).sum();
                let mut spans = vec![
                    Span::styled("◉  ", Style::new().fg(t.primary)),
                    Span::styled(format!("{:<16}", "Watched"), bold(t.text)),
                    Span::styled(plural(n, "thread"), dim()),
                ];
                if unread > 0 {
                    spans.push(Span::raw("  "));
                    spans.push(chip(format!("{unread} new"), t.background, t.new));
                }
                vec![Line::from(spans)]
            }
            SiteRow::History => vec![Line::from(vec![
                Span::styled("◷  ", Style::new().fg(t.primary)),
                Span::styled(format!("{:<16}", "History"), bold(t.text)),
                Span::styled(format!("{} recent", plural(app.store.history.len(), "thread")), dim()),
            ])],
            SiteRow::Favorite(i) => {
                let b = &app.favorites[i];
                let left = vec![
                    Span::styled("★  ", Style::new().fg(t.primary)),
                    Span::styled(format!("{:<16}", truncate(&format!("{} /{}/", b.site, b.board), 15)), bold(t.text)),
                    Span::styled(app.board_title(b).to_string(), dim()),
                ];
                // 1-9 open the first nine.
                let key = if i < 9 { vec![chip(format!("{}", i + 1), t.text_dim, t.surface_high)] } else { Vec::new() };
                vec![spread(left, key, width)]
            }
            SiteRow::Recent(i) => {
                let r = crate::app::BoardRef::parse(&app.store.recent_boards[i]);
                let (name, title) = r.as_ref().map_or((String::new(), ""), |r| (format!("{} /{}/", r.site, r.board), app.board_title(r)));
                vec![Line::from(vec![
                    Span::styled("↺  ", Style::new().fg(t.text_dim)),
                    Span::styled(format!("{:<16}", truncate(&name, 15)), Style::new().fg(t.text)),
                    Span::styled(title.to_string(), dim()),
                ])]
            }
            SiteRow::Site(i) => {
                let s = &app.sites[i];
                let url = s.cfg.url.clone().unwrap_or_else(|| "https://4chan.org".into());
                let kind = format!("{:?}", s.cfg.kind).to_lowercase();
                let hidden = app.is_site_hidden(i);
                let spans = vec![
                    Span::raw("   "),
                    Span::styled(format!("{:<16}", s.cfg.name), if hidden { dim() } else { bold(t.text) }),
                    chip(format!("{kind:<9}"), t.text_dim, t.surface_high),
                    Span::styled(format!("  {url}"), dim()),
                ];
                let right = if hidden { vec![chip("hidden", t.text_dim, t.surface_high)] } else { Vec::new() };
                vec![spread(spans, right, width)]
            }
            SiteRow::HiddenSites => {
                let n = app.hidden_sites.len();
                let what = if app.show_hidden_sites { "enter hides them again" } else { "enter shows them" };
                vec![Line::from(vec![Span::raw("   "), Span::styled(format!("{} · {what}", plural(n, "hidden site")), dim())])]
            }
        })
        .collect();
    app.hit = draw_rows(f, area, items, &mut app.site_list.state, 1, 0, None, &[]);
    if app.hit.is_none() {
        empty(f, area, "No sites match");
    }
}

/// `site  /board/  subject`, the shared start of Watched and History rows.
fn thread_row(key: &crate::store::ThreadKey, subject: &str) -> Vec<Span<'static>> {
    let t = theme();
    vec![
        Span::styled(format!("{:<11}", key.site), dim()),
        Span::styled(format!("{:<10}", format!("/{}/", key.board)), bold(t.primary)),
        Span::styled(truncate(subject, 52), Style::new().fg(t.text)),
    ]
}

fn draw_watched(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let width = area.width.saturating_sub(PAD + 1) as usize;
    let items: Vec<Vec<Line>> = app
        .visible_watched()
        .into_iter()
        .map(|i| {
            let w = &app.store.watched[i];
            let mut right = Vec::new();
            if app.refreshing.contains(&w.key) {
                right.push(Span::styled("↻  ", Style::new().fg(t.primary)));
            }
            if w.replies > 0 {
                let n = w.replies;
                right.push(chip(format!("{n} repl{} to you", if n == 1 { "y" } else { "ies" }), t.on_primary, t.primary));
                right.push(Span::raw(" "));
            }
            if w.dead {
                right.push(chip("archived/deleted", t.background, t.error));
                right.push(Span::raw("  "));
            } else if w.unread > 0 {
                right.push(chip(format!("{} new", w.unread), t.background, t.new));
                right.push(Span::raw("  "));
            }
            right.push(Span::styled(plural(w.posts, "post"), dim()));
            vec![spread(thread_row(&w.key, &w.subject), right, width)]
        })
        .collect();
    app.hit = draw_rows(f, area, items, &mut app.watched_list.state, 1, 0, None, &[]);
    if app.hit.is_none() {
        let msg = format!("No watched threads. Press {} in a catalog or thread to watch one.", app.keys.key(Action::Watch));
        empty(f, area, &msg);
    }
}

fn draw_history(f: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.saturating_sub(PAD + 1) as usize;
    let items: Vec<Vec<Line>> = app
        .visible_history()
        .into_iter()
        .map(|i| {
            let v = &app.store.history[i];
            vec![spread(thread_row(&v.key, &v.subject), vec![Span::styled(ago(v.opened, app.clock), dim())], width)]
        })
        .collect();
    app.hit = draw_rows(f, area, items, &mut app.history_list.state, 1, 0, None, &[]);
    if app.hit.is_none() {
        empty(f, area, "No history yet");
    }
}

fn draw_boards(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let width = app.boards().iter().map(|b| b.uri.width()).max().unwrap_or(1) + 4;
    let items: Vec<Vec<Line>> = app
        .visible_boards()
        .into_iter()
        .map(|i| {
            let b = &app.boards()[i];
            let mut spans = vec![
                Span::styled(format!("{:<width$}", format!("/{}/", b.uri)), bold(t.primary)),
                Span::styled(b.title.clone(), Style::new().fg(t.text)),
            ];
            if b.nsfw == Some(true) {
                spans.push(Span::styled("  nsfw", Style::new().fg(t.error)));
            }
            vec![Line::from(spans)]
        })
        .collect();
    app.hit = draw_rows(f, area, items, &mut app.board_list.state, 1, 0, None, &[]);
    if app.hit.is_none() && app.loading.is_none() {
        empty(f, area, "No boards");
    }
}

fn draw_catalog(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let images = app.images.enabled() && area.width >= MIN_THUMB_WIDTH;
    // The grid needs thumbnails; without them it's cards.
    if app.layout == CatalogLayout::Grid && images {
        draw_grid(f, app, area);
        return;
    }
    app.grid_cols = 0;
    let compact = app.layout == CatalogLayout::Compact;
    let thumbs = images && !compact;
    let width = area.width.saturating_sub(PAD + 2) as usize;
    let visible = app.visible_catalog();
    let mut state = std::mem::take(&mut app.catalog_list.state);
    let mut build = |k: usize| -> Vec<Line<'static>> {
        let i = visible[k];
        {
            let p = &app.catalog[i];
            let mark = app.catalog_marks.get(i).cloned().unwrap_or_default();
            let mut head = Vec::new();
            if let Some(label) = &mark.hidden {
                head.push(chip(hidden_label(label), t.text_dim, t.surface_high));
                head.push(Span::raw(" "));
            }
            if let Some(label) = &mark.highlight {
                head.push(chip(label.clone(), t.on_primary_container, t.primary_container));
                head.push(Span::raw(" "));
            }
            if app.catalog_new.contains(&p.no) {
                head.push(chip("new", t.background, t.new));
                head.push(Span::raw(" "));
            }
            if p.sticky {
                head.push(chip("pinned", t.text_dim, t.surface_high));
                head.push(Span::raw(" "));
            }
            if p.locked {
                head.push(chip("locked", t.text_dim, t.surface_high));
                head.push(Span::raw(" "));
            }
            // Overboards show where each thread lives.
            if let Some(b) = p.board.as_ref().filter(|b| app.board.as_ref().is_some_and(|cur| cur.uri != **b)) {
                head.push(chip(format!("/{b}/"), t.on_primary_container, t.primary_container));
                head.push(Span::raw(" "));
            }
            let subject_style = if mark.hidden.is_some() { dim() } else { bold(t.text) };
            match &p.subject {
                Some(s) => head.push(Span::styled(s.clone(), subject_style)),
                None => head.push(Span::styled(format!("No.{}", p.no), dim())),
            }
            // Some overboards don't give counts; show nothing rather than zeros.
            let mut meta = Vec::new();
            if let Some(n) = app.new_replies(p) {
                meta.push(Span::styled(format!("+{n} "), bold(t.new)));
            }
            let mut facts = Vec::new();
            if let Some(r) = p.replies {
                facts.push(format!("{r} replies"));
                facts.push(format!("{} images", p.images.unwrap_or(0)));
            }
            facts.push(ago(p.time, app.clock));
            meta.push(Span::styled(facts.join(" · "), dim()));
            let text_w = if thumbs { width.saturating_sub(CAT_THUMB.width as usize + 2) } else { width };
            if compact {
                let used: usize = head.iter().chain(&meta).map(|s| s.width()).sum();
                let room = text_w.saturating_sub(used + 4);
                if room > 8 {
                    head.push(Span::styled(format!("  {}", truncate(p.plain_text(), room)), dim()));
                }
                return vec![spread(head, meta, text_w)];
            }
            let mut lines = vec![spread(head, meta, text_w)];
            let rows = if thumbs { CAT_THUMB.height as usize - 1 } else { 1 };
            let mut preview = markup::wrap(&Line::styled(p.plain_text().to_string(), dim()), text_w);
            if preview.len() > rows {
                preview.truncate(rows);
                let last = preview.pop().map(|l| format!("{}…", line_text(&l))).unwrap_or_default();
                preview.push(Line::styled(truncate(&last, text_w), dim()));
            }
            lines.extend(preview);
            if thumbs {
                lines.resize(CAT_THUMB.height as usize, Line::raw(""));
                lines = beside(Vec::new(), lines, CAT_THUMB.width + 1);
            }
            lines
        }
    };
    let (height, gap, card) = if compact {
        (1, 0, None)
    } else if thumbs {
        (CAT_THUMB.height, 1, Some(t.surface))
    } else {
        (2, 1, Some(t.surface))
    };
    let stripes: Vec<bool> = visible.iter().map(|&i| app.catalog_marks.get(i).is_some_and(|m| m.highlight.is_some())).collect();
    let hit = draw_rows_with(f, area, visible.len(), &mut build, &mut state, height, gap, card, &stripes);
    app.catalog_list.state = state;
    app.hit = hit;
    if app.hit.is_none() {
        if app.loading.is_none() {
            empty(f, area, "No threads");
        }
        return;
    }
    if !thumbs {
        return;
    }
    // Tiles for the visible cards, top to bottom; prefetch the next page from media hosts
    // (on rate-limited hosts that would delay visible ones).
    let per = height + gap;
    let on_screen = (area.height / per) as usize + 1;
    let offset = app.catalog_list.state.offset();
    for (k, &i) in visible.iter().enumerate().skip(offset).take(on_screen * 2) {
        let Some(file) = app.catalog[i].files.first() else { continue };
        let row = (k - offset) as u16 * per;
        if row < area.height {
            let tile = Rect::new(area.x + PAD, area.y + row, CAT_THUMB.width, CAT_THUMB.height);
            draw_tile(f, &mut app.images, file, app.catalog[i].files.len(), tile, area);
        } else if let Some(url) = file.thumb.as_ref().filter(|u| http::is_media_host(u)) {
            app.images.want(url, Kind::Thumb);
        }
    }
}

/// A hidden item's chip: by which filter, or by hand.
fn hidden_label(filter: &str) -> String {
    if filter.is_empty() { "hidden".into() } else { format!("hidden: {filter}") }
}

/// Archive search results: each post with its thread, as cards.
fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let Some(s) = &app.search else { return };
    let width = area.width.saturating_sub(PAD + 2) as usize;
    let more = app.more_results();
    let mut build = |k: usize| -> Vec<Line<'static>> {
        let (thread, p) = &s.hits[k];
        let mut head = vec![Span::styled(p.name.clone(), bold(t.name)), Span::raw("  ")];
        if p.no == *thread {
            head.push(chip("OP", t.on_primary_container, t.primary_container));
            head.push(Span::raw(" "));
        } else {
            head.push(Span::styled(format!("in thread {thread}  "), dim()));
        }
        if let Some(subject) = &p.subject {
            head.push(Span::styled(subject.clone(), bold(t.text)));
        }
        let right = vec![Span::styled(format!("No.{}  ·  {}", p.no, ago(p.time, app.clock)), dim())];
        let mut lines = vec![spread(head, right, width)];
        let mut text = markup::wrap(&Line::styled(p.plain_text().to_string(), Style::new().fg(t.text)), width);
        if text.len() > 2 {
            text.truncate(2);
            let last = text.pop().map(|l| format!("{}…", line_text(&l))).unwrap_or_default();
            text.push(Line::styled(truncate(&last, width), Style::new().fg(t.text)));
        }
        lines.extend(text);
        lines
    };
    let mut state = std::mem::take(&mut app.search_list.state);
    let hit = draw_rows_with(f, area, s.hits.len(), &mut build, &mut state, 3, 1, Some(t.surface), &[]);
    app.search_list.state = state;
    if hit.is_none() && app.loading.is_none() {
        empty(f, area, "No results");
    }
    // Below the last card: more to load.
    if more && let Some(Hit::List { offset, item_height, .. }) = hit {
        let shown = (s.hits.len() - offset) as u16 * item_height;
        if shown < area.height {
            let hint = format!("{} more: {} or go down to load them", s.total.unwrap_or(0) as usize - s.hits.len(), app.keys.key(Action::NextMatch));
            put(f, area.x, area.y + shown, area.width, Line::styled(hint, dim()).centered());
        }
    }
    app.hit = hit;
}

/// The thread's files (`V`): thumbnails with their post number and type.
fn draw_gallery(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let Some(g) = &mut app.gallery else { return };
    let (card_w, card_h) = (THUMB.width + 4, THUMB.height + 1);
    let (cell_w, cell_h) = (card_w + 2, card_h + 1);
    let cols = ((area.width + 2) / cell_w).max(1) as usize;
    let rows = ((area.height + 1) / cell_h).max(1) as usize;
    g.cols = cols;
    let n = g.files.len();
    let sel = g.state.selected().unwrap_or(0).min(n - 1);
    let mut top = g.state.offset() / cols;
    if sel / cols < top {
        top = sel / cols;
    } else if sel / cols >= top + rows {
        top = sel / cols + 1 - rows;
    }
    *g.state.offset_mut() = top * cols;
    let posts = app.thread.as_ref().map(|t| &t.posts);
    for (k, (post, file)) in g.files.iter().enumerate().skip(top * cols).take((rows + 1) * cols) {
        let (r, c) = (k / cols - top, k % cols);
        let (x, y) = (area.x + c as u16 * cell_w, area.y + r as u16 * cell_h);
        let card = Rect::new(x, y, card_w, card_h).intersection(area);
        if card.is_empty() {
            if let Some(url) = file.thumb.as_ref().filter(|u| http::is_media_host(u)) {
                app.images.want(url, Kind::Thumb);
            }
            continue;
        }
        fill(f, card, if k == sel { t.selection } else { t.surface });
        if k == sel {
            fill(f, Rect::new(x, card.y, 1, card.height), t.primary);
        }
        draw_tile(f, &mut app.images, file, 1, Rect::new(x + PAD, y, THUMB.width, THUMB.height), area);
        let no = posts.and_then(|p| p.get(*post)).map_or(0, |p| p.no);
        let kind = file.ext().to_uppercase();
        let label = Line::from(vec![Span::styled(format!("No.{no}"), Style::new().fg(t.text)), Span::styled(format!("  {kind}"), dim())]);
        if y + THUMB.height < area.bottom() {
            put(f, x + PAD, y + THUMB.height, card_w - PAD - 1, label);
        }
    }
    app.hit = Some(Hit::Grid { area, offset: top * cols, cols, cell: (cell_w, cell_h) });
}

/// Grid cards: a thumbnail over three lines of text.
const GRID_CARD: Size = Size::new(20, 11);

/// The catalog as a grid of thumbnails, as many columns as fit.
fn draw_grid(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let visible = app.visible_catalog();
    if visible.is_empty() {
        app.hit = None;
        if app.loading.is_none() {
            empty(f, area, "No threads");
        }
        return;
    }
    let (cell_w, cell_h) = (GRID_CARD.width + 2, GRID_CARD.height + 1);
    let cols = ((area.width + 2) / cell_w).max(1) as usize;
    let rows = ((area.height + 1) / cell_h).max(1) as usize;
    app.grid_cols = cols;
    let state = &mut app.catalog_list.state;
    let sel = state.selected().unwrap_or(0).min(visible.len() - 1);
    let mut top = state.offset() / cols;
    if sel / cols < top {
        top = sel / cols;
    } else if sel / cols >= top + rows {
        top = sel / cols + 1 - rows;
    }
    *state.offset_mut() = top * cols;
    for (k, &i) in visible.iter().enumerate().skip(top * cols).take((rows + 1) * cols) {
        let (r, c) = (k / cols - top, k % cols);
        let (x, y) = (area.x + c as u16 * cell_w, area.y + r as u16 * cell_h);
        let card = Rect::new(x, y, GRID_CARD.width, GRID_CARD.height).intersection(area);
        if card.is_empty() {
            // Below the screen: prefetch from media hosts.
            if let Some(url) = app.catalog[i].files.first().and_then(|f| f.thumb.as_ref()).filter(|u| http::is_media_host(u)) {
                app.images.want(url, Kind::Thumb);
            }
            continue;
        }
        let p = &app.catalog[i];
        let mark = app.catalog_marks.get(i).cloned().unwrap_or_default();
        fill(f, card, if k == sel { t.selection } else { t.surface });
        if k == sel || mark.highlight.is_some() {
            fill(f, Rect::new(x, card.y, 1, card.height), t.primary);
        }
        let tile = Rect::new(x + PAD, y, THUMB.width, THUMB.height);
        match p.files.first() {
            Some(file) => draw_tile(f, &mut app.images, file, p.files.len(), tile, area),
            None => {
                fill(f, tile.intersection(area), t.surface_high);
                put(f, tile.x, tile.y + tile.height / 2, tile.width, Line::styled("no file", dim()).centered());
            }
        }
        let w = GRID_CARD.width - PAD - 1;
        let mut head = Vec::new();
        if app.catalog_new.contains(&p.no) {
            head.push(chip("new", t.background, t.new));
            head.push(Span::raw(" "));
        }
        if let Some(label) = &mark.hidden {
            head.push(chip(hidden_label(label), t.text_dim, t.surface_high));
            head.push(Span::raw(" "));
        }
        let (title, rest) = match &p.subject {
            Some(s) => (s.clone(), p.plain_text().to_string()),
            None => (p.plain_text().to_string(), String::new()),
        };
        let used: usize = head.iter().map(|s| s.width()).sum();
        head.push(Span::styled(truncate(&title, (w as usize).saturating_sub(used)), if mark.hidden.is_some() { dim() } else { bold(t.text) }));
        let mut facts = Vec::new();
        if let Some(n) = app.new_replies(p) {
            facts.push(Span::styled(format!("+{n} "), bold(t.new)));
        }
        let mut counts = Vec::new();
        if let Some(r) = p.replies {
            counts.push(format!("R{r} I{}", p.images.unwrap_or(0)));
        }
        counts.push(ago(p.time, app.clock).trim_end_matches(" ago").to_string());
        facts.push(Span::styled(counts.join(" · "), dim()));
        let text_y = y + THUMB.height;
        for (row, line) in [Line::from(head), Line::styled(truncate(&rest, w as usize), dim()), Line::from(facts)].into_iter().enumerate() {
            if text_y + (row as u16) < area.bottom() {
                put(f, x + PAD, text_y + row as u16, w, line);
            }
        }
    }
    app.hit = Some(Hit::Grid { area, offset: top * cols, cols, cell: (cell_w, cell_h) });
}

fn line_text(l: &Line) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Put `right` after a blank column `width` wide (where a tile is painted), keeping the
/// right line's style (code lines are marked by it).
fn beside(left: Vec<Line<'static>>, right: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let n = left.len().max(right.len());
    let blank = " ".repeat(width as usize);
    let (mut left, mut right) = (left.into_iter(), right.into_iter());
    (0..n)
        .map(|_| {
            let mut spans = match left.next() {
                Some(l) => l.spans,
                None => vec![Span::raw(blank.clone())],
            };
            match right.next() {
                Some(r) => {
                    spans.extend(r.spans);
                    Line::from(spans).style(r.style)
                }
                None => Line::from(spans),
            }
        })
        .collect()
}

/// A thumbnail tile: a flat square with the file's type, and the image over it once it's
/// loaded and the tile is fully on screen (`clip`).
fn draw_tile(f: &mut Frame, images: &mut Images, file: &Attachment, count: usize, tile: Rect, clip: Rect) {
    let t = theme();
    let shown = tile.intersection(clip);
    if shown.is_empty() {
        return;
    }
    fill(f, shown, t.surface_high);
    let whole = shown == tile;
    let label_row = tile.y + tile.height / 2;
    let label = |f: &mut Frame, s: String, style: Style| {
        if (shown.y..shown.bottom()).contains(&label_row) {
            put(f, tile.x, label_row, tile.width, Line::styled(truncate(&s, tile.width as usize), style).centered());
        }
    };
    let mut kind = if file.spoiler {
        "spoiler".to_string()
    } else {
        Some(file.ext().to_uppercase()).filter(|e| !e.is_empty()).unwrap_or_else(|| "FILE".into())
    };
    if count > 1 {
        kind.push_str(&format!(" +{}", count - 1));
    }
    let Some(url) = file.thumb.as_ref().filter(|_| whole) else {
        label(f, kind, dim());
        return;
    };
    match images.get(url, Size::new(tile.width, tile.height), Kind::Thumb) {
        State::Ready(p) => {
            let s = p.size();
            let r = Rect::new(tile.x + (tile.width - s.width.min(tile.width)) / 2, tile.y, s.width, s.height);
            f.render_widget(Image::new(p), r.intersection(tile));
        }
        State::Loading | State::Rendering => label(f, "…".into(), dim()),
        State::Failed => label(f, "✗".into(), Style::new().fg(t.error)),
    }
}

// ----- the thread -----

fn draw_thread(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(t) = &mut app.thread else {
        if app.loading.is_none() {
            empty(f, area, "Thread not loaded");
        }
        return;
    };
    let th = theme();
    t.viewport = area.height as usize;
    app.hit = Some(Hit::Thread { area });
    let thumbs = app.images.enabled() && area.width >= MIN_THUMB_WIDTH;
    if t.layout.as_ref().is_none_or(|l| l.width != area.width) {
        let l = layout_thread(t, area.width, thumbs, app.clock);
        // After a refresh, keep the same post at the top even if lines above it changed.
        if let Some((i, off)) = t.anchor.take() {
            t.scroll = (l.starts[i] + off).min(l.lines.len().saturating_sub(t.viewport));
        }
        t.layout = Some(l);
        t.scroll_to_selected();
    }
    let l = t.layout.as_ref().unwrap();
    let cursor = t.entry();
    for row in 0..area.height {
        let i = t.scroll + row as usize;
        let Some(line) = l.lines.get(i) else { break };
        let e = l.starts.partition_point(|&s| s <= i) - 1;
        // Each entry's last line is the gap before the next card.
        if i + 1 == l.starts[e + 1] {
            continue;
        }
        let entry = &t.entries[e];
        // Replies shown inline sit further in, on another tone.
        let x = area.x + INDENT * entry.depth as u16;
        let width = area.right().saturating_sub(x);
        let y = area.y + row;
        let card = if entry.depth % 2 == 1 { th.surface_high } else { th.surface };
        fill(f, Rect::new(x, y, width, 1), if e == cursor { th.selection } else { card });
        if e == cursor || t.marks.get(entry.post).is_some_and(|m| m.highlight.is_some()) {
            fill(f, Rect::new(x, y, 1, 1), th.primary);
        }
        if line.style == markup::CODE_LINE {
            let code_x = x + PAD + if l.thumbs.iter().any(|&(_, k)| k == e) { THUMB.width + 2 } else { 0 };
            fill(f, Rect::new(code_x, y, area.right().saturating_sub(code_x + 1), 1), th.code_bg);
        }
        put(f, x + PAD, y, width.saturating_sub(PAD + 2), Line::from(line.spans.clone()));
    }

    // Tiles: images only when fully on screen, so they never draw outside the thread area.
    // Visible ones are asked for first, top to bottom; those within a screen are prefetched
    // from media hosts.
    let (top, h, view) = (t.scroll, THUMB.height as usize, area.height as usize);
    for &(line, e) in &l.thumbs {
        if line + h > top && line < top + view {
            // A tile cut off at the top starts above the area; draw_tile clips it.
            let y = area.y as i32 + line as i32 - top as i32;
            let tile_top = y.max(area.y as i32) as u16;
            let x = area.x + INDENT * t.entries[e].depth as u16 + PAD;
            let tile = Rect::new(x, tile_top, THUMB.width, (y + THUMB.height as i32 - tile_top as i32) as u16);
            let full = line >= top && line + h <= top + view;
            let p = &t.posts[t.entries[e].post];
            if full {
                draw_tile(f, &mut app.images, &p.files[0], p.files.len(), tile, area);
            } else {
                fill(f, tile.intersection(area), th.surface_high);
            }
        }
    }
    for &(line, e) in &l.thumbs {
        let near = !(line >= top && line + h <= top + view) && line + h + view > top && line < top + 2 * view;
        if let Some(url) = t.posts[t.entries[e].post].files[0].thumb.as_ref().filter(|u| near && http::is_media_host(u)) {
            app.images.want(url, Kind::Thumb);
        }
    }

    // A scrollbar in the right margin.
    let total = l.lines.len();
    if total > view && area.right() < f.area().right() {
        let len = ((view * view) / total).max(1) as u16;
        let pos = (t.scroll * (view - len as usize) / (total - view).max(1)) as u16;
        fill(f, Rect::new(area.right() + 1, area.y + pos, 1, len), th.surface_high);
    }
}

/// Lay out every entry (post, or reply shown inline) as a card of wrapped lines: a padding
/// line above and below the content, then a gap line. Posts with files get a thumbnail
/// tile on the left.
fn layout_thread(t: &ThreadView, width: u16, thumbs: bool, clock: Clock) -> ThreadLayout {
    let mut lines = Vec::new();
    let mut starts = Vec::with_capacity(t.entries.len() + 1);
    let mut thumb_at = Vec::new();
    for (e, entry) in t.entries.iter().enumerate() {
        let (i, p) = (entry.post, &t.posts[entry.post]);
        let text_width = width.saturating_sub(PAD + 2 + INDENT * entry.depth as u16).max(10) as usize;
        starts.push(lines.len());
        // A hidden post is one line, so replies to it still make sense.
        if t.is_collapsed(i) {
            let why = t.marks[i].hidden.as_deref().filter(|l| !l.is_empty());
            let why = why.map_or("hidden".to_string(), |l| format!("hidden by the filter \"{l}\""));
            lines.push(Line::styled(format!("No.{}  {why}", p.no), Style::new().fg(theme().text_dim)));
            lines.push(Line::raw(""));
            continue;
        }
        lines.push(Line::raw(""));
        match p.files.first().filter(|_| thumbs && text_width > THUMB.width as usize + 12) {
            Some(_) => {
                thumb_at.push((lines.len(), e));
                let text = post_lines(p, &post_ctx(t, i, clock), text_width - THUMB.width as usize - 2);
                let mut text = beside(Vec::new(), text, THUMB.width + 2);
                text.resize(text.len().max(THUMB.height as usize), Line::raw(""));
                lines.extend(text);
            }
            None => lines.extend(post_lines(p, &post_ctx(t, i, clock), text_width)),
        }
        lines.push(Line::raw(""));
        lines.push(Line::raw(""));
    }
    starts.push(lines.len());
    ThreadLayout { width, lines, starts, thumbs: thumb_at }
}

/// How to render one post of a thread.
struct PostCtx<'a> {
    is_op: bool,
    is_new: bool,
    backlinks: &'a [u64],
    op_no: u64,
    reveal: bool,
    /// Lowercase search query to highlight.
    search: String,
    clock: Clock,
    mark: Option<&'a Mark>,
    /// Posts marked as yours.
    mine: &'a std::collections::HashSet<u64>,
}

fn post_ctx(t: &ThreadView, i: usize, clock: Clock) -> PostCtx<'_> {
    PostCtx {
        clock,
        is_op: i == 0,
        is_new: t.is_new(i),
        backlinks: &t.backlinks[i],
        op_no: t.no,
        reveal: t.is_revealed(i),
        search: t.search.to_lowercase(),
        mark: t.marks.get(i),
        mine: &t.mine,
    }
}

fn search_hl() -> Style {
    let t = theme();
    Style::new().fg(t.on_search).bg(t.search)
}

fn post_lines(p: &Post, ctx: &PostCtx, width: usize) -> Vec<Line<'static>> {
    let t = theme();
    let mut head = vec![Span::styled(p.name.clone(), bold(t.name)), Span::raw("  ")];
    if ctx.is_op {
        head.push(chip("OP", t.on_primary_container, t.primary_container));
        head.push(Span::raw(" "));
    }
    if ctx.is_new {
        head.push(chip("new", t.background, t.new));
        head.push(Span::raw(" "));
    }
    if ctx.mine.contains(&p.no) {
        head.push(chip("you", t.on_primary, t.primary));
        head.push(Span::raw(" "));
    }
    if let Some(m) = ctx.mark {
        if let Some(label) = &m.hidden {
            head.push(chip(hidden_label(label), t.text_dim, t.surface_high));
            head.push(Span::raw(" "));
        }
        if let Some(label) = &m.highlight {
            head.push(chip(label.clone(), t.on_primary_container, t.primary_container));
            head.push(Span::raw(" "));
        }
    }
    head.push(Span::styled(format!("{}  No.{}", fmt_time(p.time, ctx.clock), p.no), dim()));
    let mut out = markup::wrap(&Line::from(head), width);
    if let Some(s) = &p.subject {
        let subject = markup::highlight(&Line::styled(s.clone(), bold(t.primary)), &ctx.search, search_hl());
        out.extend(markup::wrap(&subject, width));
    }
    for file in &p.files {
        let mut meta = Vec::new();
        if let (Some(w), Some(h)) = (file.width, file.height) {
            meta.push(format!("{w}x{h}"));
        }
        if let Some(s) = file.size {
            meta.push(human_size(s));
        }
        let meta = if meta.is_empty() { String::new() } else { format!("  {}", meta.join(" · ")) };
        out.extend(markup::wrap(
            &Line::from(vec![Span::styled(file.filename.clone(), Style::new().fg(t.text)), Span::styled(meta, dim())]),
            width,
        ));
    }
    if !p.body.is_empty() {
        out.push(Line::raw(""));
    }
    for line in &p.body {
        let mut line = if ctx.reveal { markup::reveal(line) } else { line.clone() };
        // Mark quotes of the OP like 4chan does. Quote links are always their own span.
        for s in &mut line.spans {
            if markup::is_quote_link(s.style) {
                let target = markup::quote_target(&s.content);
                if target == Some(ctx.op_no) {
                    s.content.to_mut().push_str(" (OP)");
                }
                if target.is_some_and(|n| ctx.mine.contains(&n)) {
                    s.content.to_mut().push_str(" (You)");
                }
            }
        }
        if !ctx.search.is_empty() {
            line = markup::highlight(&line, &ctx.search, search_hl());
        }
        // Markup colors follow the current theme.
        for s in &mut line.spans {
            s.style = theme::paint(s.style);
        }
        out.extend(markup::wrap(&line, width));
    }
    if !ctx.backlinks.is_empty() {
        out.push(Line::raw(""));
        let mut spans = vec![Span::styled("Replies  ", dim())];
        for no in ctx.backlinks {
            spans.push(Span::styled(format!(">>{no}"), Style::new().fg(t.quotelink)));
            spans.push(Span::raw("  "));
        }
        out.extend(markup::wrap(&Line::from(spans), width));
    }
    out
}

// ----- panels: preview, help, settings pickers -----

/// A raised panel centered in the frame, with a title bar; returns the area inside.
fn panel(f: &mut Frame, width: u16, height: u16, title: &str, hint: &str) -> Rect {
    let t = theme();
    let area = f.area();
    let (w, h) = (width.min(area.width.saturating_sub(4)), height.min(area.height.saturating_sub(2)));
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, r);
    fill(f, r, t.surface_highest);
    let title_row = Rect::new(r.x, r.y, r.width, 1);
    fill(f, title_row, t.primary_container);
    // When both don't fit, the title wins.
    let fits = title.width() + hint.width() + 6 <= r.width as usize;
    let hint_w = if fits { hint.width() as u16 + 2 } else { 0 };
    let title = truncate(title, (r.width.saturating_sub(hint_w) as usize).saturating_sub(3));
    put(f, r.x, r.y, r.width.saturating_sub(hint_w), Line::styled(format!("  {title}"), bold(t.on_primary_container)));
    if fits {
        put(f, r.right().saturating_sub(hint_w), r.y, hint_w, Line::styled(format!("{hint}  "), Style::new().fg(t.on_primary_container)));
    }
    Rect::new(r.x + 2, r.y + 2, r.width.saturating_sub(4), r.height.saturating_sub(3))
}

fn draw_preview(f: &mut Frame, app: &App) {
    let (Some(p), Some(t)) = (&app.preview, &app.thread) else { return };
    let w = f.area().width.saturating_sub(8).clamp(20, 110);
    let width = w.saturating_sub(4) as usize;
    let mut lines = Vec::new();
    for &i in &p.posts {
        lines.extend(post_lines(&t.posts[i], &post_ctx(t, i, app.clock), width));
        lines.push(Line::raw(""));
    }
    for n in &p.elsewhere {
        lines.push(Line::styled(format!(">>{n} is in another thread or board; enter in the thread follows it"), dim()));
    }
    while lines.last().is_some_and(|l| l.width() == 0) {
        lines.pop();
    }
    let inner = panel(f, w, lines.len() as u16 + 3, "Quoted posts", "j/k scroll · enter jump · esc close");
    let scroll = (p.scroll as usize).min(lines.len().saturating_sub(inner.height as usize));
    for (row, line) in lines.into_iter().skip(scroll).take(inner.height as usize).enumerate() {
        put(f, inner.x, inner.y + row as u16, inner.width, line);
    }
}

/// The selected post's links: quotes leading elsewhere, web links, files.
fn draw_links(f: &mut Frame, app: &mut App) {
    let t = theme();
    let Some(p) = &mut app.links else { return };
    let w = f.area().width.saturating_sub(8).clamp(20, 110);
    let inner = panel(f, w, p.items.len() as u16 + 3, "Links", "enter open · y copy · esc close");
    let rows = inner.height as usize;
    let sel = p.list.selected().unwrap_or(0);
    let off = p.list.offset().min(sel).max((sel + 1).saturating_sub(rows));
    *p.list.offset_mut() = off;
    p.area = inner;
    for (k, item) in p.items.iter().enumerate().skip(off).take(rows) {
        let y = inner.y + (k - off) as u16;
        if k == sel {
            fill(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), t.selection);
            fill(f, Rect::new(inner.x - 2, y, 1, 1), t.primary);
        }
        let (kind, text, extra) = match item {
            LinkItem::Quote(_, label) => ("quote", label.clone(), String::new()),
            LinkItem::Url(u) => ("web", u.clone(), String::new()),
            LinkItem::File(file) => ("file", file.filename.clone(), format!("  {}", file.url)),
        };
        let room = (inner.width as usize).saturating_sub(9);
        let text = truncate(&text, room);
        let extra = truncate(&extra, room.saturating_sub(text.width()));
        put(
            f,
            inner.x,
            y,
            inner.width,
            Line::from(vec![
                chip(format!("{kind:<5}"), t.text_dim, t.surface_high),
                Span::raw("  "),
                Span::styled(text, Style::new().fg(if kind == "file" { t.text } else { t.quotelink })),
                Span::styled(extra, dim()),
            ]),
        );
    }
}

/// `R`: the reverse image search engines, per file.
fn draw_image_search(f: &mut Frame, app: &mut App) {
    let t = theme();
    let names: Vec<String> = app.image_search.iter().map(|e| e.name.clone()).collect();
    let Some(p) = &mut app.image_search_panel else { return };
    let inner = panel(f, 64, p.rows.len() as u16 + 3, "Search for this image", "enter open · y copy · esc close");
    let rows = inner.height as usize;
    let sel = p.list.selected().unwrap_or(0);
    let off = p.list.offset().min(sel).max((sel + 1).saturating_sub(rows));
    *p.list.offset_mut() = off;
    p.area = inner;
    for (k, row) in p.rows.iter().enumerate().skip(off).take(rows) {
        let y = inner.y + (k - off) as u16;
        let line = match row {
            Err(file) => Line::styled(truncate(file, inner.width as usize), bold(t.primary)),
            Ok((_, e)) => {
                if k == sel {
                    fill(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), t.selection);
                    fill(f, Rect::new(inner.x - 2, y, 1, 1), t.primary);
                }
                Line::styled(format!("  {}", names[*e]), Style::new().fg(t.text))
            }
        };
        put(f, inner.x, y, inner.width, line);
    }
}

/// Key help, by section, with the configured keys. Keep in sync with the README.
fn help_sections(keys: &KeyMap) -> Vec<(&'static str, Vec<(String, &'static str)>)> {
    let k = |a| keys.label(a);
    let pair = |a, b| format!("{} / {}", keys.label(a), keys.label(b));
    // In this order, the two columns split evenly (Everywhere to the viewer, then the rest).
    vec![
        (
            "Everywhere",
            vec![
                ("j k g G, ↑ ↓".into(), "move, top / bottom"),
                ("ctrl-d / ctrl-u".into(), "half page down / up"),
                ("enter l / esc h".into(), "open / back"),
                (k(Action::Search), "filter (thread: search)"),
                (pair(Action::Reload, Action::Browser), "reload / open in browser"),
                (k(Action::Goto), "go to a URL or site/board"),
                (k(Action::Settings), "settings: theme, keys, …"),
                (pair(Action::NextTab, Action::PrevTab), "next / previous tab"),
                (k(Action::CloseTab), "close the tab"),
                (format!("{}, ctrl-c", k(Action::Quit)), "quit (mouse works too)"),
            ],
        ),
        (
            "Watched, History",
            vec![
                (k(Action::Remove), "remove the entry"),
                (k(Action::NewTab), "open in a new tab"),
                (pair(Action::Copy, Action::CopyLink), "copy subject+link / link"),
            ],
        ),
        (
            "Image viewer",
            vec![
                ("h / l, ← / →".into(), "previous / next file"),
                ("space".into(), "pause an animated GIF"),
                ("i".into(), "open externally"),
                (pair(Action::Copy, Action::CopyLink), "copy file URL / post link"),
                (k(Action::ImageSearch), "reverse image search"),
                ("esc, q".into(), "close"),
            ],
        ),
        (
            "Catalog",
            vec![
                (k(Action::View), "view the OP's images"),
                (pair(Action::Watch, Action::NewTab), "watch / open in a new tab"),
                (pair(Action::Sort, Action::Compact), "sort / layout (grid, …)"),
                (k(Action::Links), "the OP's links and files"),
                (pair(Action::Hide, Action::ShowHidden), "hide / show hidden"),
                (k(Action::ArchiveSearch), "search the board's archive"),
                (pair(Action::Copy, Action::CopyLink), "copy text / link"),
            ],
        ),
        (
            "Thread",
            vec![
                ("J / K, space".into(), "scroll by line / page"),
                ("enter, l".into(), "follow quote (any thread)"),
                (pair(Action::Preview, Action::Replies), "preview quotes / 1st reply"),
                (pair(Action::JumpBack, Action::Unread), "jump back / first unread"),
                (pair(Action::NextMatch, Action::PrevMatch), "next / previous match"),
                (pair(Action::Spoiler, Action::AllSpoilers), "spoilers: post / all"),
                (k(Action::Expand), "replies under the post"),
                (pair(Action::View, Action::Gallery), "view images / gallery"),
                (pair(Action::OpenFile, Action::ImageSearch), "open file / image search"),
                (k(Action::Links), "the post's links and files"),
                (format!("{} / {}", pair(Action::Download, Action::DownloadThread), k(Action::Export)), "save: files / all / page"),
                (pair(Action::Watch, Action::NewTab), "watch / quote in a new tab"),
                (pair(Action::Hide, Action::ShowHidden), "hide post / show hidden"),
                (k(Action::Mine), "mark as yours (replies)"),
                (k(Action::Archive), "open a 404'd thread archived"),
                (pair(Action::Copy, Action::CopyLink), "copy text / link"),
            ],
        ),
    ]
}

fn draw_help(f: &mut Frame, app: &App) {
    const COL: u16 = 50;
    let t = theme();
    let sections: Vec<Vec<Line>> = help_sections(&app.keys)
        .into_iter()
        .map(|(title, rows)| {
            let mut lines = vec![Line::styled(title, bold(t.primary))];
            lines.extend(rows.into_iter().map(|(k, v)| {
                Line::from(vec![Span::styled(format!("  {k:<18} "), bold(t.text)), Span::styled(v, Style::new().fg(t.text_dim))])
            }));
            lines.push(Line::raw(""));
            lines
        })
        .collect();
    let total: usize = sections.iter().map(Vec::len).sum();
    let area = f.area();
    // Two columns when one doesn't fit, split at the section boundary nearest the middle.
    let two = total as u16 + 4 > area.height && area.width >= 2 * COL + 8;
    let mut cols = vec![Vec::new(), Vec::new()];
    for s in sections {
        let c = usize::from(two && cols[0].len() + s.len() / 2 >= total / 2);
        cols[c].extend(s);
    }
    for c in &mut cols {
        while c.last().is_some_and(|l| l.width() == 0) {
            c.pop();
        }
    }
    let rows = cols.iter().map(Vec::len).max().unwrap_or(0) as u16;
    let w = if two { 2 * COL + 6 } else { COL + 4 };
    let hint = format!("images: {} · esc close", app.images.protocol_name());
    let inner = panel(f, w, rows + 3, "Keys", &hint);
    // In small terminals the help scrolls (j/k).
    let scroll = app.help_scroll.min(rows.saturating_sub(inner.height)) as usize;
    for (c, lines) in cols.into_iter().enumerate() {
        let x = inner.x + c as u16 * (COL + 2);
        for (row, line) in lines.into_iter().skip(scroll).take(inner.height as usize).enumerate() {
            put(f, x, inner.y + row as u16, COL, line);
        }
    }
}

// ----- settings -----

fn draw_settings(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let selected = app.settings_list.state.selected().unwrap_or(0);
    let rows = setting_rows();
    // Scroll so the selected setting (and the note under the list, at the end) shows.
    let at = rows.iter().position(|r| *r == Ok(selected)).unwrap_or(0);
    let total = rows.len() + 2;
    let offset = if at + 1 == rows.len() { total } else { at + 2 }.saturating_sub(area.height as usize);
    app.hit = Some(Hit::Settings { area, offset });
    let items: Vec<_> = SETTING_SECTIONS.iter().flat_map(|(_, items)| items.iter().copied()).collect();
    for (row, r) in rows.into_iter().enumerate().skip(offset) {
        let y = area.y + (row - offset) as u16;
        if y >= area.bottom() {
            break;
        }
        match r {
            Err(title) => put(f, area.x, y, area.width, Line::styled(title.to_string(), bold(t.primary))),
            Ok(i) => {
                let item = items[i];
                let line = Rect::new(area.x, y, area.width, 1);
                if i == selected {
                    fill(f, line, t.selection);
                    fill(f, Rect::new(area.x, y, 1, 1), t.primary);
                }
                let value = app.setting_value(item);
                let hint_w = (area.width as usize).saturating_sub(PAD as usize + 1 + 18 + 34);
                put(
                    f,
                    area.x + PAD,
                    y,
                    area.width.saturating_sub(PAD + 1),
                    Line::from(vec![
                        Span::styled(format!("{:<18}", item.label()), Style::new().fg(t.text)),
                        Span::styled(format!("{:<34}", truncate(&value, 32)), bold(t.text)),
                        Span::styled(if hint_w >= 16 { truncate(item.hint(), hint_w) } else { String::new() }, dim()),
                    ]),
                );
            }
        }
    }
    let y = (area.y as usize + setting_rows().len() + 1).checked_sub(offset).map_or(area.bottom(), |y| y as u16);
    if y < area.bottom() {
        let note = match &app.config_path {
            Some(p) => format!(
                "Saved to {}, comments kept. Themes, colors and keys can be edited there too.",
                crate::app::tilde(&p.display().to_string())
            ),
            None => "There's no config file to save to (no home directory); changes last until ck quits.".into(),
        };
        put(f, area.x, y, area.width, Line::styled(note, dim()));
    }
}

/// A row of colored cells: a theme at a glance.
fn swatch(t: &Theme) -> Vec<Span<'static>> {
    [t.background, t.surface, t.selection, t.primary, t.primary_container, t.greentext, t.quotelink, t.heading, t.new]
        .into_iter()
        .map(|c| Span::styled("  ", Style::new().bg(c)))
        .collect()
}

fn draw_settings_popup(f: &mut Frame, app: &App) {
    let t = theme();
    match &app.settings.popup {
        Some(SettingsPopup::Themes { list, names, .. }) => {
            let inner = panel(f, 52, names.len() as u16 + 3, "Theme", "enter keep · esc cancel");
            let sel = list.selected().unwrap_or(0);
            for (k, name) in names.iter().enumerate().take(inner.height as usize) {
                let y = inner.y + k as u16;
                if k == sel {
                    fill(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), t.selection);
                    fill(f, Rect::new(inner.x - 2, y, 1, 1), t.primary);
                }
                let mut spans = vec![Span::styled(format!("{name:<22}"), Style::new().fg(t.text))];
                if let Ok(th) = theme::resolve(name, &app.themes) {
                    spans.extend(swatch(&th));
                }
                put(f, inner.x, y, inner.width, Line::from(spans));
            }
        }
        Some(SettingsPopup::Colors { list, editing }) => {
            let title = format!("Colors · {}", app.theme_name);
            let hint = if editing.is_some() { "enter save · esc cancel" } else { "enter edit · x reset · esc close" };
            let h = (ROLES.len() as u16 + 5).min(f.area().height.saturating_sub(4));
            let inner = panel(f, 96, h, &title, hint);
            let rows = inner.height.saturating_sub(2) as usize;
            let sel = list.selected().unwrap_or(0);
            let first = sel.saturating_sub(rows.saturating_sub(1));
            for (k, (role, desc)) in ROLES.iter().enumerate().skip(first).take(rows) {
                let y = inner.y + (k - first) as u16;
                if k == sel {
                    fill(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), t.selection);
                    fill(f, Rect::new(inner.x - 2, y, 1, 1), t.primary);
                }
                let c = t.get(role).unwrap_or(Color::Reset);
                put(
                    f,
                    inner.x,
                    y,
                    inner.width,
                    Line::from(vec![
                        Span::styled("    ", Style::new().bg(c)),
                        Span::styled(format!("  {role:<22}"), Style::new().fg(t.text)),
                        Span::styled(format!("{:<10}", theme::color_string(c)), bold(t.text)),
                        Span::styled(*desc, dim()),
                    ]),
                );
            }
            let y = inner.bottom().saturating_sub(1);
            let line = match editing {
                Some(text) => Line::from(vec![
                    Span::styled(format!("New {} color: ", ROLES[sel].0), Style::new().fg(t.text)),
                    Span::styled(text.clone(), bold(t.text)),
                    Span::styled("▏", Style::new().fg(t.primary)),
                    Span::styled("   #rrggbb, a name, 0-255, or default", dim()),
                ]),
                None => Line::styled("Changing a color of a built-in theme saves it as a copy: NAME-custom.", dim()),
            };
            put(f, inner.x, y, inner.width, line);
        }
        Some(SettingsPopup::Keys { list, capture }) => {
            let rows = key_rows();
            let hint = if capture.is_some() { "press a key · esc cancel" } else { "enter rebind · a add · x reset · esc close" };
            let h = (rows.len() as u16 + 5).min(f.area().height.saturating_sub(4));
            let inner = panel(f, 96, h, "Keys", hint);
            let view = inner.height.saturating_sub(2) as usize;
            let sel = list.selected().unwrap_or(0);
            let first = (sel + 2).saturating_sub(view).min(rows.len().saturating_sub(view));
            for (k, row) in rows.iter().enumerate().skip(first).take(view) {
                let y = inner.y + (k - first) as u16;
                let i = match row {
                    Err(title) => {
                        put(f, inner.x, y, inner.width, Line::styled(title.to_string(), bold(t.primary)));
                        continue;
                    }
                    Ok(i) => *i,
                };
                if k == sel {
                    fill(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), t.selection);
                    fill(f, Rect::new(inner.x - 2, y, 1, 1), t.primary);
                }
                let (action, name, _, scopes, desc) = keys::ACTIONS[i];
                let changed = !app.keys.is_default(action);
                let key_style = if changed { bold(t.primary) } else { bold(t.text) };
                let scopes = scopes.iter().map(|s| s.label()).collect::<Vec<_>>().join(", ");
                let mut spans = vec![Span::styled(format!("  {:<16}", truncate(&app.keys.label(action), 15)), key_style)];
                // Narrow: just the key and what it does.
                if inner.width >= 80 {
                    spans.push(Span::styled(format!("{name:<17}"), dim()));
                    spans.push(Span::styled(format!("{:<37}", truncate(desc, 36)), Style::new().fg(t.text)));
                    spans.push(Span::styled(truncate(&scopes, (inner.width as usize).saturating_sub(72)), dim()));
                } else {
                    spans.push(Span::styled(truncate(desc, (inner.width as usize).saturating_sub(18)), Style::new().fg(t.text)));
                }
                put(f, inner.x, y, inner.width, Line::from(spans));
            }
            let y = inner.bottom().saturating_sub(1);
            let line = match (capture, rows.get(sel)) {
                (Some(add), Some(Ok(i))) => {
                    let verb = if *add { "Press a key to add to" } else { "Press the new key for" };
                    Line::from(vec![
                        Span::styled(format!("{verb} "), Style::new().fg(t.text)),
                        Span::styled(keys::ACTIONS[*i].1, bold(t.primary)),
                        Span::styled("   esc cancels", dim()),
                    ])
                }
                _ => Line::styled("Changed keys are saved in [keys]; navigation keys are fixed.", dim()),
            };
            put(f, inner.x, y, inner.width, line);
        }
        Some(SettingsPopup::Folder { value }) => {
            let inner = panel(f, 90, 7, "Download folder", "enter save · esc cancel");
            let field = Rect::new(inner.x, inner.y, inner.width, 1);
            fill(f, field, t.surface);
            put(
                f,
                inner.x + 1,
                inner.y,
                inner.width.saturating_sub(2),
                Line::from(vec![Span::styled(value.clone(), Style::new().fg(t.text)), Span::styled("▏", Style::new().fg(t.primary))]),
            );
            let help = "{site}, {board}, {thread} and {downloads} are filled in. Empty for the default.";
            put(f, inner.x, inner.y + 2, inner.width, Line::styled(help, dim()));
        }
        None => {}
    }
}

// ----- the image viewer -----

fn draw_viewer(f: &mut Frame, app: &mut App) {
    let t = theme();
    let Some(v) = &app.viewer else { return };
    let file = &v.files[v.index];
    let [top, _, middle, _, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let mut meta = Vec::new();
    if let (Some(w), Some(h)) = (file.width, file.height) {
        meta.push(format!("{w}x{h}"));
    }
    if let Some(s) = file.size {
        meta.push(human_size(s));
    }
    meta.push(format!("{} of {}", v.index + 1, v.files.len()));
    fill(f, top, t.bar);
    put(
        f,
        top.x,
        top.y,
        top.width,
        Line::from(vec![
            Span::styled(" ck ", bold(t.on_primary).bg(t.primary)),
            Span::styled(format!("  {}", file.filename), bold(t.on_bar)),
            Span::styled(format!("    {}", meta.join("  ·  ")), dim()),
        ]),
    );
    fill(f, bottom, t.bar);
    let mut hints = vec![Span::raw(" ")];
    if let Some((msg, is_err)) = &app.status {
        let (mark, bg) = if *is_err { ("!", t.error) } else { ("✓", t.success) };
        hints.push(Span::styled(format!(" {mark} "), bold(t.background).bg(bg)));
        hints.push(Span::styled(format!(" {msg}"), Style::new().fg(t.on_bar)));
    } else {
        for (k, label) in [("h/l", "previous / next"), ("i", "open externally"), ("esc", "close")] {
            hints.push(Span::styled(k, bold(t.primary)));
            hints.push(Span::styled(format!(" {label}   "), dim()));
        }
    }
    put(f, bottom.x, bottom.y, bottom.width, Line::from(hints));
    let area = middle.inner(Margin::new(MARGIN, 0));
    // Non-images (videos, pdfs, ...) show their thumbnail, if any, with a hint.
    let url = if file.is_image() { Some(&file.url) } else { file.thumb.as_ref() };
    let mut inner = area;
    if !file.is_image() {
        let hint = match file.ext().to_uppercase() {
            e if e.is_empty() => "Showing the thumbnail. Press i to open the file externally.".to_string(),
            e => format!("{e} files can't be shown here; showing the thumbnail. Press i to open it externally."),
        };
        put(f, inner.x, inner.bottom().saturating_sub(1), inner.width, Line::styled(hint, dim()).centered());
        inner.height = inner.height.saturating_sub(2);
    }
    let msg = |f: &mut Frame, s: String, style: Style| {
        put(f, inner.x, inner.y + inner.height / 2, inner.width, Line::styled(s, style).centered());
    };
    // Terminal graphics would cover a panel on top.
    let Some(url) = url.filter(|_| app.image_search_panel.is_none()) else { return };
    let spinner = SPINNER[app.tick % SPINNER.len()];
    let kind = if file.is_image() { Kind::Full } else { Kind::Thumb };
    match app.images.get(url, Size::new(inner.width, inner.height), kind) {
        State::Ready(p) => {
            let s = p.size();
            let r = Rect::new(
                inner.x + inner.width.saturating_sub(s.width) / 2,
                inner.y + inner.height.saturating_sub(s.height) / 2,
                s.width,
                s.height,
            );
            f.render_widget(Image::new(p), r.intersection(inner));
        }
        State::Loading => msg(f, format!("{spinner} Loading…"), Style::new().fg(t.primary)),
        State::Rendering => msg(f, format!("{spinner} Rendering…"), Style::new().fg(t.primary)),
        State::Failed => msg(f, "Couldn't load this image".into(), Style::new().fg(t.error)),
    }
}

// ----- text -----

fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > width.saturating_sub(1) {
            out.push('…');
            return out;
        }
        out.push(c);
        w += cw;
    }
    out
}

/// Local date and time with the relative time; UTC when the clock is fixed (tests).
fn fmt_time(ts: i64, clock: Clock) -> String {
    let date = match clock.fixed {
        Some(_) => Utc.timestamp_opt(ts, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string()),
        None => Local.timestamp_opt(ts, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string()),
    };
    match date {
        Some(d) => format!("{d} · {}", ago(ts, clock)),
        None => String::new(),
    }
}

fn ago(ts: i64, clock: Clock) -> String {
    if ts == 0 {
        return String::new();
    }
    let secs = (clock.now() - ts).max(0);
    match secs {
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s if s < 86400 * 365 => format!("{}d ago", s / 86400),
        s => format!("{}y ago", s / (86400 * 365)),
    }
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}
