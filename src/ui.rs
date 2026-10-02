use chrono::{Local, TimeZone, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph};
use ratatui_image::Image;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Clock, Hit, SiteRow, Sort, ThreadLayout, ThreadView, View};
use crate::http;
use crate::images::{Images, Kind, State};
use crate::keys::{Action, KeyMap};
use crate::markup;
use crate::model::{Attachment, Post};
use crate::theme::theme;

fn accent() -> Color {
    theme().accent
}

fn dim() -> Style {
    Style::new().fg(theme().dim)
}

fn selected() -> Style {
    Style::new().bg(theme().selected).add_modifier(Modifier::BOLD)
}

fn search_hl() -> Style {
    Style::new().fg(Color::Black).bg(theme().search)
}

fn new_style() -> Style {
    Style::new().fg(theme().new).bold()
}
const SPINNER: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
/// Thumbnail sizes in cells (roughly square at a 1:2 cell aspect).
const THUMB: Size = Size::new(16, 8);
const CAT_THUMB: Size = Size::new(10, 4);
/// Below this width there's no room for thumbnails next to text.
const MIN_THUMB_WIDTH: u16 = 60;

pub fn draw(f: &mut Frame, app: &mut App) {
    if app.viewer.is_some() {
        draw_viewer(f, app);
        app.images.end_frame();
        return;
    }
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(f.area());

    draw_header(f, app, header);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(dim())
        .title(view_title(app))
        .title_style(Style::new().fg(accent()).bold());
    let inner = block.inner(body);
    f.render_widget(block, body);

    match app.view {
        View::Sites => draw_sites(f, app, inner),
        View::Boards => draw_boards(f, app, inner),
        View::Catalog => draw_catalog(f, app, inner),
        View::Thread => draw_thread(f, app, inner),
        View::Watched => draw_watched(f, app, inner),
        View::History => draw_history(f, app, inner),
    }
    draw_footer(f, app, footer);

    if app.preview.is_some() {
        draw_preview(f, app);
    }
    if app.show_help {
        draw_help(f, app);
    }
    app.images.end_frame();
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let sep = Span::styled(" › ", dim());
    let mut spans = vec![Span::styled(" ck ", Style::new().fg(Color::Black).bg(accent()).bold())];
    match app.view {
        View::Sites => {}
        View::Watched | View::History => {
            spans.push(sep.clone());
            spans.push(Span::raw(if app.view == View::Watched { "Watched" } else { "History" }));
        }
        _ => {
            spans.push(sep.clone());
            spans.push(Span::raw(app.current_site().cfg.name.clone()));
        }
    }
    if matches!(app.view, View::Catalog | View::Thread)
        && let Some(b) = &app.board
    {
        spans.push(sep.clone());
        spans.push(Span::raw(format!("/{}/", b.uri)));
    }
    if let (View::Thread, Some(t)) = (app.view, &app.thread) {
        spans.push(sep);
        spans.push(Span::raw(format!("{}", t.no)));
    }
    f.render_widget(Line::from(spans), area);
}

fn view_title(app: &App) -> Line<'static> {
    let (title, filter) = match app.view {
        View::Sites => ("Sites".to_string(), &app.site_list.filter),
        View::Boards => {
            let n = app.boards().len();
            (format!("Boards ({n})"), &app.board_list.filter)
        }
        View::Catalog => {
            let b = app.board.as_ref().map(|b| {
                if b.title.is_empty() { format!("/{}/", b.uri) } else { format!("/{}/ - {}", b.uri, b.title) }
            });
            let sort = if app.catalog_sort == Sort::Bump { String::new() } else { format!(" · {}", app.catalog_sort.label()) };
            (format!("{} ({} threads){sort}", b.unwrap_or_default(), app.catalog.len()), &app.catalog_list.filter)
        }
        View::Thread => {
            let t = app.thread.as_ref();
            let subject = t.and_then(|t| t.posts.first()?.subject.clone());
            let n = t.map_or(0, |t| t.posts.len());
            let title = format!(" {} ({n} post{}) ", subject.unwrap_or_else(|| "Thread".into()), if n == 1 { "" } else { "s" });
            let mut spans = vec![Span::raw(title)];
            if let Some(t) = t.filter(|t| !t.search.is_empty() || app.searching) {
                let k = t.matches.len();
                let s = format!("[/{}] {k} match{} ", t.search, if k == 1 { "" } else { "es" });
                spans.push(Span::styled(s, Style::new().fg(Color::Cyan)));
            }
            if t.is_some_and(|t| t.reveal_all) {
                spans.push(Span::styled("[spoilers shown] ", dim()));
            }
            return Line::from(spans);
        }
        View::Watched => (format!("Watched ({})", app.store.watched.len()), &app.watched_list.filter),
        View::History => (format!("History ({})", app.store.history.len()), &app.history_list.filter),
    };
    let mut spans = vec![Span::raw(format!(" {title} "))];
    if !filter.is_empty() || app.filtering {
        spans.push(Span::styled(format!("[/{filter}] "), Style::new().fg(Color::Cyan)));
    }
    Line::from(spans)
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = if app.searching {
        let q = app.thread.as_ref().map_or("", |t| t.search.as_str());
        Line::from(vec![
            Span::styled(" search: ", Style::new().fg(Color::Cyan)),
            Span::raw(q.to_string()),
            Span::styled("█", Style::new().fg(Color::Cyan)),
            Span::styled("  enter accept · esc clear", dim()),
        ])
    } else if app.filtering {
        Line::from(vec![
            Span::styled(" filter: ", Style::new().fg(Color::Cyan)),
            Span::raw(current_filter(app).to_string()),
            Span::styled("█", Style::new().fg(Color::Cyan)),
            Span::styled("  enter accept · esc clear", dim()),
        ])
    } else if let Some(label) = &app.loading {
        Line::from(vec![
            Span::styled(format!(" {} ", SPINNER[app.tick % SPINNER.len()]), Style::new().fg(accent())),
            Span::raw(format!("{label}…")),
        ])
    } else if let Some((msg, is_err)) = &app.status {
        let style = if *is_err { Style::new().fg(Color::Red) } else { Style::new().fg(Color::Green) };
        Line::styled(format!(" {msg}"), style)
    } else {
        Line::styled(format!(" {}", footer_hints(app)), dim())
    };
    // Background work is shown at the right without hiding the rest of the footer.
    let d = &app.downloads;
    let mut indicator = String::new();
    if d.running > 0 {
        indicator.push_str(&format!(" ⇣ {}/{} ", d.done + d.skipped + d.failed, d.total));
    }
    if !app.refreshing.is_empty() {
        indicator.push_str(&format!(" ↻ refreshing {} ", app.refreshing.len()));
    }
    let indicator = (!indicator.is_empty()).then_some(indicator);
    let width = indicator.as_ref().map_or(0, |s| s.width() as u16);
    let [left, right] = Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(area);
    f.render_widget(line, left);
    if let Some(s) = indicator {
        f.render_widget(Line::styled(s, Style::new().fg(Color::Cyan)), right);
    }
}

/// Key hints for the footer, with the configured keys.
fn footer_hints(app: &App) -> String {
    let k = |a| app.keys.key(a);
    match app.view {
        View::Thread => format!(
            "j/k post · enter quote · {} preview · {} back · {} search · {} unread · {} view · {} watch · {} help",
            k(Action::Preview),
            k(Action::JumpBack),
            k(Action::Search),
            k(Action::Unread),
            k(Action::View),
            k(Action::Watch),
            k(Action::Help)
        ),
        View::Catalog => format!(
            "j/k move · enter open · esc back · {} filter · {} view · {} watch · {} sort · {} compact · {} reload · {} help",
            k(Action::Search),
            k(Action::View),
            k(Action::Watch),
            k(Action::Sort),
            k(Action::Compact),
            k(Action::Reload),
            k(Action::Help)
        ),
        View::Watched | View::History => format!(
            "j/k move · enter open · {} remove · esc back · {} filter · {} browser · {} help",
            k(Action::Remove),
            k(Action::Search),
            k(Action::Browser),
            k(Action::Help)
        ),
        _ => format!(
            "j/k move · enter open · esc back · {} filter · {} browser · {} reload · {} help · {} quit",
            k(Action::Search),
            k(Action::Browser),
            k(Action::Reload),
            k(Action::Help),
            k(Action::Quit)
        ),
    }
}

fn current_filter(app: &App) -> &str {
    match app.view {
        View::Sites => &app.site_list.filter,
        View::Boards => &app.board_list.filter,
        View::Catalog => &app.catalog_list.filter,
        View::Watched => &app.watched_list.filter,
        View::History => &app.history_list.filter,
        View::Thread => "",
    }
}

fn draw_sites(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .visible_sites()
        .into_iter()
        .map(|row| match row {
            SiteRow::Watched => {
                let n = app.store.watched.len();
                let unread: usize = app.store.watched.iter().map(|w| w.unread).sum();
                let mut spans = vec![
                    Span::styled(format!("{:<16}", "★ Watched"), Style::new().fg(accent()).bold()),
                    Span::styled(format!("{n} thread{}", if n == 1 { "" } else { "s" }), dim()),
                ];
                if unread > 0 {
                    spans.push(Span::styled(format!("  {unread} new"), new_style()));
                }
                ListItem::new(Line::from(spans))
            }
            SiteRow::History => ListItem::new(Line::from(vec![
                Span::styled(format!("{:<16}", "◷ History"), Style::new().fg(accent()).bold()),
                Span::styled(
                    format!("{} recent thread{}", app.store.history.len(), if app.store.history.len() == 1 { "" } else { "s" }),
                    dim(),
                ),
            ])),
            SiteRow::Site(i) => {
                let s = &app.sites[i];
                let url = s.cfg.url.clone().unwrap_or_else(|| "https://4chan.org".into());
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{:<16}", s.cfg.name), Style::new().bold()),
                    Span::styled(format!("{:<10}", format!("{:?}", s.cfg.kind).to_lowercase()), Style::new().fg(Color::Blue)),
                    Span::styled(url, dim()),
                ]))
            }
        })
        .collect();
    app.hit = render_list(f, area, items, &mut app.site_list.state, "No sites match", 1);
}

/// `site  /board/  no  subject`, the shared start of Watched and History rows.
fn thread_row(key: &crate::store::ThreadKey, subject: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(format!("{:<10} ", key.site), Style::new().fg(Color::Blue)),
        Span::styled(format!("{:<9} ", format!("/{}/", key.board)), Style::new().fg(accent())),
        Span::styled(format!("{:<10} ", key.no), dim()),
        Span::raw(truncate(subject, 50)),
    ]
}

fn draw_watched(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .visible_watched()
        .into_iter()
        .map(|i| {
            let w = &app.store.watched[i];
            let mut spans = thread_row(&w.key, &w.subject);
            spans.push(Span::styled(format!("  {} post{}", w.posts, if w.posts == 1 { "" } else { "s" }), dim()));
            if w.dead {
                spans.push(Span::styled("  archived/deleted", Style::new().fg(Color::Red)));
            } else if w.unread > 0 {
                spans.push(Span::styled(format!("  {} new", w.unread), new_style()));
            }
            if app.refreshing.contains(&w.key) {
                spans.push(Span::styled("  ↻", Style::new().fg(Color::Cyan)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let empty = format!("No watched threads. Press {} in a catalog or thread to watch one.", app.keys.key(Action::Watch));
    app.hit = render_list(f, area, items, &mut app.watched_list.state, &empty, 1);
}

fn draw_history(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .visible_history()
        .into_iter()
        .map(|i| {
            let v = &app.store.history[i];
            let mut spans = thread_row(&v.key, &v.subject);
            spans.push(Span::styled(format!("  {}", ago(v.opened, app.clock)), dim()));
            ListItem::new(Line::from(spans))
        })
        .collect();
    app.hit = render_list(f, area, items, &mut app.history_list.state, "No history yet", 1);
}

fn draw_boards(f: &mut Frame, app: &mut App, area: Rect) {
    let width = app.boards().iter().map(|b| b.uri.width()).max().unwrap_or(1) + 3;
    let items: Vec<ListItem> = app
        .visible_boards()
        .into_iter()
        .map(|i| {
            let b = &app.boards()[i];
            let mut spans = vec![
                Span::styled(format!("{:<width$}", format!("/{}/", b.uri)), Style::new().fg(accent()).bold()),
                Span::raw(b.title.clone()),
            ];
            if b.nsfw == Some(true) {
                spans.push(Span::styled(" nsfw", Style::new().fg(Color::Red)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let empty = if app.loading.is_some() { "" } else { "No boards" };
    app.hit = render_list(f, area, items, &mut app.board_list.state, empty, 1);
}

fn draw_catalog(f: &mut Frame, app: &mut App, area: Rect) {
    let thumbs = app.images.enabled() && area.width >= MIN_THUMB_WIDTH && !app.compact;
    let width = area.width.saturating_sub(3) as usize;
    let visible = app.visible_catalog();
    let items: Vec<ListItem> = visible
        .iter()
        .map(|&i| {
            let p = &app.catalog[i];
            let mut head = Vec::new();
            if p.sticky {
                head.push(Span::styled("📌 ", Style::new().fg(Color::Green)));
            }
            if p.locked {
                head.push(Span::styled("🔒 ", Style::new().fg(Color::Red)));
            }
            // Overboards show where each thread lives.
            if let Some(b) = p.board.as_ref().filter(|b| app.board.as_ref().is_some_and(|cur| cur.uri != **b)) {
                head.push(Span::styled(format!("/{b}/ "), Style::new().fg(accent())));
            }
            head.push(Span::styled(format!("{}", p.no), dim()));
            head.push(Span::raw("  "));
            if let Some(s) = &p.subject {
                head.push(Span::styled(s.clone(), Style::new().fg(accent()).bold()));
                head.push(Span::raw("  "));
            }
            // Some overboards don't give counts; show nothing rather than zeros.
            if let Some(r) = p.replies {
                head.push(Span::styled(format!("R:{r} I:{}  ", p.images.unwrap_or(0)), Style::new().fg(Color::Blue)));
            }
            head.push(Span::styled(ago(p.time, app.clock), dim()));
            if app.compact {
                // One line: the header, then as much of the text as fits.
                let used: usize = head.iter().map(|s| s.content.width()).sum();
                head.push(Span::styled(format!("  {}", truncate(p.plain_text(), width.saturating_sub(used + 2))), Style::new().fg(Color::Gray)));
                return ListItem::new(Line::from(head));
            }
            if !thumbs {
                let preview = truncate(p.plain_text(), width);
                return ListItem::new(Text::from(vec![Line::from(head), Line::styled(preview, Style::new().fg(Color::Gray)), Line::raw("")]));
            }
            // Thumbnail on the left, header and up to three preview lines beside it.
            let text_w = width.saturating_sub(CAT_THUMB.width as usize + 1).max(1);
            let mut text = markup::wrap(&Line::from(head), text_w);
            text.truncate(1);
            let mut preview = markup::wrap(&Line::styled(p.plain_text().to_string(), Style::new().fg(Color::Gray)), text_w);
            let rows = CAT_THUMB.height as usize - 1;
            if preview.len() > rows {
                preview.truncate(rows);
                let last = preview.pop().map(|l| format!("{}…", line_text(&l))).unwrap_or_default();
                preview.push(Line::styled(truncate(&last, text_w), Style::new().fg(Color::Gray)));
            }
            text.extend(preview);
            text.resize(CAT_THUMB.height as usize, Line::raw(""));
            let left = p.files.first().map(|f| placeholder(f, p.files.len(), CAT_THUMB)).unwrap_or_default();
            let mut lines = beside(left, text, CAT_THUMB.width);
            lines.push(Line::raw(""));
            ListItem::new(Text::from(lines))
        })
        .collect();
    let empty = if app.loading.is_some() { "" } else { "No threads" };
    let item_height = if app.compact { 1 } else if thumbs { CAT_THUMB.height + 1 } else { 3 };
    app.hit = render_list(f, area, items, &mut app.catalog_list.state, empty, item_height);
    if !thumbs {
        return;
    }
    // Draw thumbnails over the placeholders of fully visible entries, top to bottom; prefetch
    // the next page from media hosts (on rate-limited hosts that would delay visible ones).
    let per = CAT_THUMB.height + 1;
    let on_screen = (area.height / per) as usize;
    let offset = app.catalog_list.state.offset();
    for (k, &i) in visible.iter().enumerate().skip(offset).take(on_screen * 2 + 1) {
        let Some(file) = app.catalog[i].files.first() else { continue };
        let row = (k - offset) as u16 * per;
        if row + CAT_THUMB.height <= area.height {
            draw_thumb(f, &mut app.images, file, Rect::new(area.x + 2, area.y + row, CAT_THUMB.width, CAT_THUMB.height));
        } else if let Some(url) = file.thumb.as_ref().filter(|u| http::is_media_host(u)) {
            app.images.want(url, Kind::Thumb);
        }
    }
}

fn line_text(l: &Line) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// A dim box standing in for a thumbnail: shown while it loads, or for files without one.
fn placeholder(file: &Attachment, count: usize, size: Size) -> Vec<Line<'static>> {
    let (w, h) = (size.width as usize, size.height as usize);
    let mut label = if file.spoiler {
        "spoiler".to_string()
    } else {
        Some(file.ext().to_uppercase()).filter(|e| !e.is_empty()).unwrap_or_else(|| "FILE".into())
    };
    if count > 1 {
        label.push_str(&format!(" +{}", count - 1));
    }
    let label = truncate(&label, w - 2);
    (0..h)
        .map(|r| {
            let s = if r == 0 {
                format!("┌{}┐", "─".repeat(w - 2))
            } else if r == h - 1 {
                format!("└{}┘", "─".repeat(w - 2))
            } else if r == (h - 1) / 2 {
                format!("│{label:^0$}│", w - 2)
            } else {
                format!("│{}│", " ".repeat(w - 2))
            };
            Line::styled(s, dim())
        })
        .collect()
}

/// Put `left` (exactly `width` columns per line, or nothing) beside `right`, one column apart.
fn beside(left: Vec<Line<'static>>, right: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let n = left.len().max(right.len());
    let blank = " ".repeat(width as usize + 1);
    let (mut left, mut right) = (left.into_iter(), right.into_iter());
    (0..n)
        .map(|_| {
            let mut spans = match left.next() {
                Some(l) => {
                    let mut s = l.spans;
                    s.push(Span::raw(" "));
                    s
                }
                None => vec![Span::raw(blank.clone())],
            };
            if let Some(r) = right.next() {
                spans.extend(r.spans);
            }
            Line::from(spans)
        })
        .collect()
}

/// Draw `file`'s thumbnail over its placeholder in `area`, or a loading/failed marker.
fn draw_thumb(f: &mut Frame, images: &mut Images, file: &Attachment, area: Rect) {
    let Some(url) = &file.thumb else { return };
    let mark = |f: &mut Frame, s: &str, style: Style| {
        let r = Rect::new(area.x + 1, area.y + area.height / 2, area.width - 2, 1);
        f.render_widget(Line::styled(s.to_string(), style).centered(), r);
    };
    match images.get(url, Size::new(area.width, area.height), Kind::Thumb) {
        State::Ready(p) => {
            let s = p.size();
            f.render_widget(Clear, area);
            let r = Rect::new(area.x + (area.width - s.width.min(area.width)) / 2, area.y, s.width, s.height);
            f.render_widget(Image::new(p), r.intersection(area));
        }
        State::Loading | State::Rendering => mark(f, "…", dim()),
        State::Failed => mark(f, "✗", Style::new().fg(Color::Red)),
    }
}

/// Draw a list of items `item_height` rows tall; returns where it went, for mouse clicks.
fn render_list(
    f: &mut Frame,
    area: Rect,
    items: Vec<ListItem>,
    state: &mut ratatui::widgets::ListState,
    empty: &str,
    item_height: u16,
) -> Option<Hit> {
    if items.is_empty() {
        f.render_widget(Paragraph::new(Span::styled(format!(" {empty}"), dim())), area);
        return None;
    }
    let list = List::new(items).highlight_style(selected()).highlight_symbol("▌ ");
    f.render_stateful_widget(list, area, state);
    Some(Hit::List { area, offset: state.offset(), item_height })
}

fn draw_thread(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(t) = &mut app.thread else {
        let msg = if app.loading.is_some() { "" } else { "Thread not loaded" };
        f.render_widget(Paragraph::new(Span::styled(format!(" {msg}"), dim())), area);
        return;
    };
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
    let (sel_start, sel_end) = (l.starts[t.selected], l.starts[t.selected + 1]);
    let lines: Vec<Line> = l
        .lines
        .iter()
        .enumerate()
        .skip(t.scroll)
        .take(area.height as usize)
        .map(|(i, line)| {
            let gutter = if (sel_start..sel_end).contains(&i) {
                Span::styled("▌ ", Style::new().fg(accent()))
            } else {
                Span::raw("  ")
            };
            let mut spans = vec![gutter];
            spans.extend(line.spans.iter().cloned());
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);

    // Thumbnails go over their placeholders only when fully on screen, so they never draw
    // outside the thread area. Visible ones are asked for first, top to bottom; the ones
    // within a screen of the view are prefetched from media hosts.
    let (top, h, view) = (t.scroll, THUMB.height as usize, area.height as usize);
    for &(line, i) in &l.thumbs {
        if line >= top && line + h <= top + view {
            let r = Rect::new(area.x + 2, area.y + (line - top) as u16, THUMB.width, THUMB.height);
            draw_thumb(f, &mut app.images, &t.posts[i].files[0], r);
        }
    }
    for &(line, i) in &l.thumbs {
        let near = !(line >= top && line + h <= top + view) && line + h + view > top && line < top + 2 * view;
        if let Some(url) = t.posts[i].files[0].thumb.as_ref().filter(|u| near && http::is_media_host(u)) {
            app.images.want(url, Kind::Thumb);
        }
    }
}

/// Lay out every post of a thread as wrapped lines, recording where each post starts.
/// With `thumbs`, posts with files get a fixed-size thumbnail column on the left.
fn layout_thread(t: &ThreadView, width: u16, thumbs: bool, clock: Clock) -> ThreadLayout {
    let text_width = width.saturating_sub(2).max(10) as usize;
    let mut lines = Vec::new();
    let mut starts = Vec::with_capacity(t.posts.len() + 1);
    let mut thumb_at = Vec::new();
    for (i, p) in t.posts.iter().enumerate() {
        starts.push(lines.len());
        match p.files.first().filter(|_| thumbs) {
            Some(file) => {
                thumb_at.push((lines.len(), i));
                let text = post_lines(p, &post_ctx(t, i, clock), text_width - THUMB.width as usize - 1);
                lines.extend(beside(placeholder(file, p.files.len(), THUMB), text, THUMB.width));
            }
            None => lines.extend(post_lines(p, &post_ctx(t, i, clock), text_width)),
        }
        lines.push(Line::raw(""));
    }
    starts.push(lines.len());
    ThreadLayout { width, lines, starts, thumbs: thumb_at }
}

fn draw_preview(f: &mut Frame, app: &App) {
    let (Some(p), Some(t)) = (&app.preview, &app.thread) else { return };
    let area = f.area();
    let w = area.width.saturating_sub(8).clamp(20, 110).min(area.width);
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
    let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(4)).max(3);
    let popup = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    let scroll = p.scroll.min((lines.len() as u16).saturating_sub(h - 2));
    let lines: Vec<Line> = lines.into_iter().map(|l| Line::from([vec![Span::raw(" ")], l.spans].concat())).collect();
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Line::styled(" Quoted posts ", Style::new().fg(accent()).bold()))
        .title_bottom(Line::styled(" j/k scroll · enter jump · esc close ", dim()).right_aligned());
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(block), popup);
}

fn draw_viewer(f: &mut Frame, app: &mut App) {
    let Some(v) = &app.viewer else { return };
    let file = &v.files[v.index];
    let area = f.area();
    f.render_widget(Clear, area);
    let mut meta = Vec::new();
    if let (Some(w), Some(h)) = (file.width, file.height) {
        meta.push(format!("{w}x{h}"));
    }
    if let Some(s) = file.size {
        meta.push(human_size(s));
    }
    let title = format!(" {} ({}/{}) {} ", file.filename, v.index + 1, v.files.len(), meta.join(", "));
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Line::styled(title, Style::new().fg(accent()).bold()))
        .title_bottom(Line::styled(" h/l previous/next · i open externally · esc close ", dim()).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    // Non-images (videos, pdfs, ...) show their thumbnail, if any, with a hint.
    let url = if file.is_image() { Some(&file.url) } else { file.thumb.as_ref() };
    let mut inner = inner;
    if !file.is_image() {
        let hint = match file.ext().to_uppercase() {
            e if e.is_empty() => "Showing the thumbnail. Press i to open the file externally.".to_string(),
            e => format!("{e} files can't be shown here; showing the thumbnail. Press i to open it externally."),
        };
        let r = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
        f.render_widget(Line::styled(hint, dim()).centered(), r);
        inner.height = inner.height.saturating_sub(2);
    }
    let msg = |f: &mut Frame, s: String, style: Style| {
        let r = Rect::new(inner.x, inner.y + inner.height / 2, inner.width, 1);
        f.render_widget(Line::styled(s, style).centered(), r);
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
        State::Loading => msg(f, format!("{spinner} Loading…"), Style::new().fg(accent())),
        State::Rendering => msg(f, format!("{spinner} Rendering…"), Style::new().fg(accent())),
        State::Failed => msg(f, "Couldn't load this image".into(), Style::new().fg(Color::Red)),
    }
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

fn post_lines(p: &Post, ctx: &PostCtx, width: usize) -> Vec<Line<'static>> {
    let (is_op, is_new, backlinks) = (ctx.is_op, ctx.is_new, ctx.backlinks);
    let mut head = vec![
        Span::styled(p.name.clone(), Style::new().fg(theme().name).bold()),
        Span::raw("  "),
        Span::styled(fmt_time(p.time, ctx.clock), dim()),
        Span::raw("  "),
        Span::styled(format!("No.{}", p.no), Style::new().fg(Color::Blue)),
    ];
    if is_op {
        head.push(Span::styled(" OP", Style::new().fg(accent()).bold()));
    }
    if is_new {
        head.push(Span::styled(" ● new", new_style()));
    }
    let mut out = markup::wrap(&Line::from(head), width);
    if let Some(s) = &p.subject {
        let subject = markup::highlight(&Line::styled(s.clone(), Style::new().fg(accent()).bold()), &ctx.search, search_hl());
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
        let meta = if meta.is_empty() { String::new() } else { format!(" ({})", meta.join(", ")) };
        out.extend(markup::wrap(
            &Line::from(vec![
                Span::styled("File: ", dim()),
                Span::styled(file.filename.clone(), Style::new().fg(Color::Cyan)),
                Span::styled(meta, dim()),
            ]),
            width,
        ));
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
        out.extend(markup::wrap(&line, width));
    }
    if !backlinks.is_empty() {
        let mut spans = vec![Span::styled("Replies: ", dim())];
        for no in backlinks {
            spans.push(Span::styled(format!(">>{no}"), markup::quotelink().fg(theme().dim)));
            spans.push(Span::raw(" "));
        }
        out.extend(markup::wrap(&Line::from(spans), width));
    }
    out
}

/// Key help, by section, with the configured keys. Keep in sync with the README.
fn help_sections(keys: &KeyMap) -> Vec<(&'static str, Vec<(String, &'static str)>)> {
    let k = |a| keys.key(a).to_string();
    let pair = |a, b| format!("{} / {}", keys.key(a), keys.key(b));
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
            ],
        ),
        ("Watched, History", vec![(k(Action::Remove), "remove the entry")]),
        (
            "Image viewer",
            vec![
                ("h / l, ← / →".into(), "previous / next file"),
                ("i".into(), "open externally"),
                ("esc, q".into(), "close"),
            ],
        ),
    ]
}

fn draw_help(f: &mut Frame, app: &App) {
    const COL: u16 = 52;
    let sections: Vec<Vec<Line>> = help_sections(&app.keys)
        .into_iter()
        .map(|(title, rows)| {
            let mut lines = vec![Line::styled(title, Style::new().fg(accent()).bold())];
            lines.extend(rows.into_iter().map(|(k, v)| {
                Line::from(vec![Span::styled(format!("  {k:<20}"), Style::new().fg(Color::Cyan)), Span::raw(v)])
            }));
            lines.push(Line::raw(""));
            lines
        })
        .collect();
    let total: usize = sections.iter().map(Vec::len).sum();
    let area = f.area();
    // Two columns when one doesn't fit, split at the section boundary nearest the middle.
    let two = total as u16 + 2 > area.height && area.width >= 2 * COL + 3;
    let mut cols = vec![Vec::new(), Vec::new()];
    for s in sections {
        let c = usize::from(two && cols[0].len() + s.len() / 2 >= total / 2);
        cols[c].extend(s);
    }
    for c in &mut cols {
        if c.last().is_some_and(|l| l.width() == 0) {
            c.pop();
        }
    }
    let rows = cols.iter().map(Vec::len).max().unwrap_or(0) as u16;
    let w = if two { 2 * COL + 3 } else { COL + 2 }.min(area.width);
    let h = (rows + 2).min(area.height);
    let popup = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Help ")
        .title_bottom(Line::styled(format!(" images: {} ", app.images.protocol_name()), dim()).right_aligned())
        .border_style(Style::new().fg(accent()));
    let block = if rows + 2 > h { block.title_bottom(Line::styled(" j/k scroll ", dim()).left_aligned()) } else { block };
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    let [left, right] = Layout::horizontal([Constraint::Length(COL + 1), Constraint::Min(0)]).areas(inner);
    // In small terminals the help scrolls (j/k).
    let scroll = app.help_scroll.min(rows.saturating_sub(inner.height));
    let [c0, c1] = [std::mem::take(&mut cols[0]), std::mem::take(&mut cols[1])];
    f.render_widget(Paragraph::new(c0).scroll((scroll, 0)), left);
    f.render_widget(Paragraph::new(c1).scroll((scroll, 0)), right);
}

fn truncate(s: &str, width: usize) -> String {
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
        Some(d) => format!("{d} ({})", ago(ts, clock)),
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
