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
    App, Clock, Hit, SETTING_SECTIONS, SettingsPopup, SiteRow, Sort, ThreadLayout, ThreadView, View, key_rows,
    setting_rows,
};
use crate::http;
use crate::images::{Images, Kind, State};
use crate::keys::{self, Action, KeyMap};
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
        let [bar, _, body, footer] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
                .areas(f.area());
        draw_app_bar(f, app, bar);
        let content = body.inner(Margin::new(MARGIN, 0));
        match app.view {
            View::Sites => draw_sites(f, app, content),
            View::Boards => draw_boards(f, app, content),
            View::Catalog => draw_catalog(f, app, content),
            View::Thread => draw_thread(f, app, content),
            View::Watched => draw_watched(f, app, content),
            View::History => draw_history(f, app, content),
            View::Settings => draw_settings(f, app, content),
        }
        draw_footer(f, app, footer);
        if app.preview.is_some() {
            draw_preview(f, app);
        }
        if app.settings.popup.is_some() {
            draw_settings_popup(f, app);
        }
        if app.show_help {
            draw_help(f, app);
        }
    }
    app.images.end_frame();
    if !app.truecolor {
        downgrade(f.buffer_mut());
    }
}

/// Where you are (left) and what's here (right), on a solid bar.
fn draw_app_bar(f: &mut Frame, app: &App, area: Rect) {
    let t = theme();
    fill(f, area, t.bar);
    let (crumbs, meta) = location(app);
    let mut spans = vec![Span::styled(" ck ", bold(t.on_primary).bg(t.primary)), Span::raw(" ")];
    let n = crumbs.len();
    for (i, c) in crumbs.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ›  ", dim()));
        }
        let style = if i + 1 == n { bold(t.on_bar) } else { Style::new().fg(t.on_bar) };
        spans.push(Span::styled(c, style));
    }
    let meta_w = meta.iter().map(|s| s.width()).sum::<usize>() as u16;
    let left_w = area.width.saturating_sub(meta_w + 1);
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
            vec![site(), uri, truncate(&subject, 48)]
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
    let typing = if app.searching {
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
            Span::styled("   enter accept   esc clear", dim()),
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
        View::Thread | View::Settings => "",
    }
}

// ----- lists and cards -----

/// Draw items `height` rows tall with `gap` rows of background between them. With `card`
/// each item sits on that color; the selected one gets the selection color and an accent
/// stripe. Returns where it went, for mouse clicks.
fn draw_rows(
    f: &mut Frame,
    area: Rect,
    items: Vec<Vec<Line<'static>>>,
    state: &mut ListState,
    height: u16,
    gap: u16,
    card: Option<Color>,
) -> Option<Hit> {
    if items.is_empty() || area.is_empty() {
        return None;
    }
    let t = theme();
    let per = height + gap;
    let fit = ((area.height + gap) / per).max(1) as usize;
    let sel = state.selected().unwrap_or(0).min(items.len() - 1);
    let mut off = state.offset().min(items.len() - 1);
    if sel < off {
        off = sel;
    } else if sel >= off + fit {
        off = sel + 1 - fit;
    }
    *state.offset_mut() = off;
    for (k, lines) in items.into_iter().enumerate().skip(off) {
        let y = area.y + (k - off) as u16 * per;
        if y >= area.bottom() {
            break;
        }
        let h = height.min(area.bottom() - y);
        let row = Rect::new(area.x, y, area.width, h);
        let selected = k == sel;
        if selected {
            fill(f, row, t.selection);
            fill(f, Rect::new(row.x, y, 1, h), t.primary);
        } else if let Some(c) = card {
            fill(f, row, c);
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
    let items: Vec<Vec<Line>> = app
        .visible_sites()
        .into_iter()
        .map(|row| match row {
            SiteRow::Watched => {
                let n = app.store.watched.len();
                let unread: usize = app.store.watched.iter().map(|w| w.unread).sum();
                let mut spans = vec![
                    Span::styled("★  ", Style::new().fg(t.primary)),
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
            SiteRow::Site(i) => {
                let s = &app.sites[i];
                let url = s.cfg.url.clone().unwrap_or_else(|| "https://4chan.org".into());
                let kind = format!("{:?}", s.cfg.kind).to_lowercase();
                vec![Line::from(vec![
                    Span::raw("   "),
                    Span::styled(format!("{:<16}", s.cfg.name), bold(t.text)),
                    chip(format!("{kind:<9}"), t.text_dim, t.surface_high),
                    Span::styled(format!("  {url}"), dim()),
                ])]
            }
        })
        .collect();
    app.hit = draw_rows(f, area, items, &mut app.site_list.state, 1, 0, None);
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
    app.hit = draw_rows(f, area, items, &mut app.watched_list.state, 1, 0, None);
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
    app.hit = draw_rows(f, area, items, &mut app.history_list.state, 1, 0, None);
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
    app.hit = draw_rows(f, area, items, &mut app.board_list.state, 1, 0, None);
    if app.hit.is_none() && app.loading.is_none() {
        empty(f, area, "No boards");
    }
}

fn draw_catalog(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let thumbs = app.images.enabled() && area.width >= MIN_THUMB_WIDTH && !app.compact;
    let width = area.width.saturating_sub(PAD + 2) as usize;
    let visible = app.visible_catalog();
    let items: Vec<Vec<Line>> = visible
        .iter()
        .map(|&i| {
            let p = &app.catalog[i];
            let mut head = Vec::new();
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
            match &p.subject {
                Some(s) => head.push(Span::styled(s.clone(), bold(t.text))),
                None => head.push(Span::styled(format!("No.{}", p.no), dim())),
            }
            // Some overboards don't give counts; show nothing rather than zeros.
            let mut meta = Vec::new();
            if let Some(r) = p.replies {
                meta.push(format!("{r} replies"));
                meta.push(format!("{} images", p.images.unwrap_or(0)));
            }
            meta.push(ago(p.time, app.clock));
            let meta = vec![Span::styled(meta.join(" · "), dim())];
            let text_w = if thumbs { width.saturating_sub(CAT_THUMB.width as usize + 2) } else { width };
            if app.compact {
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
        })
        .collect();
    let (height, gap, card) = if app.compact {
        (1, 0, None)
    } else if thumbs {
        (CAT_THUMB.height, 1, Some(t.surface))
    } else {
        (2, 1, Some(t.surface))
    };
    app.hit = draw_rows(f, area, items, &mut app.catalog_list.state, height, gap, card);
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
    let text_w = area.width.saturating_sub(PAD + 2);
    for row in 0..area.height {
        let i = t.scroll + row as usize;
        let Some(line) = l.lines.get(i) else { break };
        let post = l.starts.partition_point(|&s| s <= i) - 1;
        // Each post's last line is the gap before the next card.
        if i + 1 == l.starts[post + 1] {
            continue;
        }
        let y = area.y + row;
        let selected = post == t.selected;
        fill(f, Rect::new(area.x, y, area.width, 1), if selected { th.selection } else { th.surface });
        if selected {
            fill(f, Rect::new(area.x, y, 1, 1), th.primary);
        }
        if line.style == markup::CODE_LINE {
            let code_x = area.x + PAD + if l.thumbs.iter().any(|&(_, p)| p == post) { THUMB.width + 2 } else { 0 };
            fill(f, Rect::new(code_x, y, area.right().saturating_sub(code_x + 1), 1), th.code_bg);
        }
        put(f, area.x + PAD, y, text_w, Line::from(line.spans.clone()));
    }

    // Tiles: images only when fully on screen, so they never draw outside the thread area.
    // Visible ones are asked for first, top to bottom; those within a screen are prefetched
    // from media hosts.
    let (top, h, view) = (t.scroll, THUMB.height as usize, area.height as usize);
    for &(line, i) in &l.thumbs {
        if line + h > top && line < top + view {
            // A tile cut off at the top starts above the area; draw_tile clips it.
            let y = area.y as i32 + line as i32 - top as i32;
            let tile_top = y.max(area.y as i32) as u16;
            let tile = Rect::new(area.x + PAD, tile_top, THUMB.width, (y + THUMB.height as i32 - tile_top as i32) as u16);
            let full = line >= top && line + h <= top + view;
            let p = &t.posts[i];
            if full {
                draw_tile(f, &mut app.images, &p.files[0], p.files.len(), tile, area);
            } else {
                fill(f, tile.intersection(area), th.surface_high);
            }
        }
    }
    for &(line, i) in &l.thumbs {
        let near = !(line >= top && line + h <= top + view) && line + h + view > top && line < top + 2 * view;
        if let Some(url) = t.posts[i].files[0].thumb.as_ref().filter(|u| near && http::is_media_host(u)) {
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

/// Lay out every post as a card of wrapped lines: a padding line above and below the
/// content, then a gap line. Posts with files get a thumbnail tile on the left.
fn layout_thread(t: &ThreadView, width: u16, thumbs: bool, clock: Clock) -> ThreadLayout {
    let text_width = width.saturating_sub(PAD + 2).max(10) as usize;
    let mut lines = Vec::new();
    let mut starts = Vec::with_capacity(t.posts.len() + 1);
    let mut thumb_at = Vec::new();
    for (i, p) in t.posts.iter().enumerate() {
        starts.push(lines.len());
        lines.push(Line::raw(""));
        match p.files.first().filter(|_| thumbs) {
            Some(_) => {
                thumb_at.push((lines.len(), i));
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
            if markup::is_quote_link(s.style) && markup::quote_target(&s.content) == Some(ctx.op_no) {
                s.content.to_mut().push_str(" (OP)");
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
    let hint_w = hint.width() as u16 + 2;
    put(f, r.x, r.y, r.width.saturating_sub(hint_w), Line::styled(format!("  {title}"), bold(t.on_primary_container)));
    put(f, r.right().saturating_sub(hint_w), r.y, hint_w, Line::styled(format!("{hint}  "), Style::new().fg(t.on_primary_container)));
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

/// Key help, by section, with the configured keys. Keep in sync with the README.
fn help_sections(keys: &KeyMap) -> Vec<(&'static str, Vec<(String, &'static str)>)> {
    let k = |a| keys.label(a);
    let pair = |a, b| format!("{} / {}", keys.label(a), keys.label(b));
    vec![
        (
            "Everywhere",
            vec![
                ("j / k, ↓ / ↑".into(), "move"),
                ("g / G".into(), "top / bottom"),
                ("ctrl-d / ctrl-u".into(), "half page down / up"),
                ("enter, l".into(), "open"),
                ("esc, h, backspace".into(), "back"),
                (k(Action::Search), "filter list"),
                (k(Action::Reload), "reload"),
                (k(Action::Browser), "open in browser"),
                (k(Action::Settings), "settings: theme, keys, …"),
                (format!("{}, ctrl-c", k(Action::Quit)), "quit"),
                ("mouse".into(), "wheel scroll, click, dbl-click"),
            ],
        ),
        (
            "Catalog",
            vec![
                (k(Action::View), "view the OP's images"),
                (k(Action::Watch), "watch / unwatch the thread"),
                (k(Action::Sort), "cycle sort order"),
                (k(Action::Compact), "compact layout on / off"),
                (pair(Action::Copy, Action::CopyLink), "copy the OP's text / link"),
            ],
        ),
        (
            "Thread",
            vec![
                ("j / k".into(), "next / previous post"),
                ("J / K, space".into(), "scroll by line / page"),
                ("enter, l".into(), "follow quote (any thread)"),
                (k(Action::Preview), "preview the quoted posts"),
                (k(Action::Replies), "jump to first reply"),
                (k(Action::JumpBack), "back (also to last thread)"),
                (k(Action::Search), "search the thread"),
                (pair(Action::NextMatch, Action::PrevMatch), "next / previous match"),
                (pair(Action::Spoiler, Action::AllSpoilers), "show spoilers: post / all"),
                (k(Action::OpenFile), "open file (videos in mpv)"),
                (k(Action::View), "view the post's images"),
                (pair(Action::Download, Action::DownloadThread), "save files: post / thread"),
                (k(Action::Watch), "watch / unwatch the thread"),
                (k(Action::Unread), "jump to the first unread post"),
                (k(Action::Archive), "open 404'd thread in archive"),
                (pair(Action::Copy, Action::CopyLink), "copy the post's text / link"),
            ],
        ),
        (
            "Watched, History",
            vec![(k(Action::Remove), "remove the entry"), (pair(Action::Copy, Action::CopyLink), "copy subject and link / link")],
        ),
        (
            "Image viewer",
            vec![
                ("h / l, ← / →".into(), "previous / next file"),
                ("i".into(), "open externally"),
                (pair(Action::Copy, Action::CopyLink), "copy the file's URL / post link"),
                ("esc, q".into(), "close"),
            ],
        ),
    ]
}

fn draw_help(f: &mut Frame, app: &App) {
    const COL: u16 = 54;
    let t = theme();
    let sections: Vec<Vec<Line>> = help_sections(&app.keys)
        .into_iter()
        .map(|(title, rows)| {
            let mut lines = vec![Line::styled(title, bold(t.primary))];
            lines.extend(rows.into_iter().map(|(k, v)| {
                Line::from(vec![Span::styled(format!("  {k:<20}"), bold(t.text)), Span::styled(v, Style::new().fg(t.text_dim))])
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
    app.hit = Some(Hit::Settings { area });
    let selected = app.settings_list.state.selected().unwrap_or(0);
    let items: Vec<_> = SETTING_SECTIONS.iter().flat_map(|(_, items)| items.iter().copied()).collect();
    for (row, r) in setting_rows().into_iter().enumerate() {
        let y = area.y + row as u16;
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
    let y = area.y + setting_rows().len() as u16 + 1;
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
                put(
                    f,
                    inner.x,
                    y,
                    inner.width,
                    Line::from(vec![
                        Span::styled(format!("  {:<16}", truncate(&app.keys.label(action), 15)), key_style),
                        Span::styled(format!("{name:<17}"), dim()),
                        Span::styled(format!("{:<37}", truncate(desc, 36)), Style::new().fg(t.text)),
                        Span::styled(truncate(&scopes, (inner.width as usize).saturating_sub(72)), dim()),
                    ]),
                );
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
    for (k, label) in [("h/l", "previous / next"), ("i", "open externally"), ("esc", "close")] {
        hints.push(Span::styled(k, bold(t.primary)));
        hints.push(Span::styled(format!(" {label}   "), dim()));
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
    let Some(url) = url else { return };
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
