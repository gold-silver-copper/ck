//! Drawing, in a flat style: no lines or boxes. Regions are told apart by background tone
//! (cards a step above the background, raised panels another), by spacing, and by type.
//! Every color comes from the current theme's roles.

use chrono::{Local, TimeZone, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, ListState};
use std::rc::Rc;

use ratatui_image::Image;
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, Clock, Hit, LineCache, LinkItem, Part, SETTING_SECTIONS, Spot, SettingsPopup, SiteRow, Sort, Status, ThreadLayout, ThreadView, View, key_rows,
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
pub(crate) const PAD: u16 = 2;
/// How far in each level of replies shown inline (`e`) sits.
pub(crate) const INDENT: u16 = 4;

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
/// Draw a line (text every terminal measures alike, see `markup::for_terminal`).
fn put(f: &mut Frame, x: u16, y: u16, w: u16, mut line: Line) {
    let r = Rect::new(x, y, w, 1).intersection(f.area());
    if !r.is_empty() {
        for s in &mut line.spans {
            if let std::borrow::Cow::Owned(safe) = markup::for_terminal(&s.content) {
                s.content = safe.into();
            }
        }
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

// ----- the frame -----

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = theme();
    let all = f.area();
    // A saved copy is read offline: its images only come from disk.
    app.images.offline = app.tab.view == View::Thread && app.tab.offline.is_some();
    f.buffer_mut().set_style(all, Style::new().fg(t.text).bg(t.background));
    if app.tab.viewer.is_some() {
        draw_viewer(f, app);
    } else {
        let [bar, gap, body, footer] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
                .areas(f.area());
        draw_app_bar(f, app, bar);
        draw_tab_row(f, app, gap.inner(Margin::new(MARGIN, 0)));
        let content = body.inner(Margin::new(MARGIN, 0));
        match app.tab.view {
            View::Sites => draw_sites(f, app, content),
            View::Boards => draw_boards(f, app, content),
            View::Catalog => draw_catalog(f, app, content),
            View::Thread if app.tab.gallery.is_some() => draw_gallery(f, app, content),
            View::Thread => {
                draw_thread(f, app, content);
                draw_peek(f, app, content);
            }
            View::Watched => draw_watched(f, app, content),
            View::History => draw_history(f, app, content),
            View::Saved => draw_saved(f, app, content),
            View::Settings => draw_settings(f, app, content),
            View::Search => draw_search(f, app, content),
        }
        draw_footer(f, app, footer);
        if app.tab.preview.is_some() {
            draw_preview(f, app);
        }
        if app.tab.links.is_some() {
            draw_links(f, app);
        }
        if app.settings_popup.is_some() {
            draw_settings_popup(f, app);
        }
        if app.filter_add.is_some() {
            draw_add_filter(f, app);
        }
        if app.show_help {
            draw_help(f, app);
        }
    }
    if app.hints.is_some() {
        draw_hints(f, app);
    }
    if app.menu.is_some() {
        draw_menu(f, app);
    }
    if app.image_search_panel.is_some() {
        draw_image_search(f, app);
    }
    app.images.end_frame();
    // Every 24-bit color to the nearest of 256, for terminals without 24-bit color.
    if !app.truecolor {
        for cell in f.buffer_mut().content.iter_mut() {
            cell.fg = theme::to_256(cell.fg);
            cell.bg = theme::to_256(cell.bg);
        }
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
        app.tab.board.as_ref().map(|b| if b.title.is_empty() { format!("/{}/", b.uri) } else { format!("/{}/  {}", b.uri, b.title) })
    };
    let mut meta: Vec<String> = Vec::new();
    let crumbs = match app.tab.view {
        View::Sites => {
            meta.push(plural(app.sites.len(), "site"));
            vec!["Sites".to_string()]
        }
        View::Boards => {
            meta.push(plural(app.boards().len(), "board"));
            vec![site(), "Boards".into()]
        }
        View::Catalog => {
            meta.push(plural(app.tab.catalog.len(), "thread"));
            let new = app.tab.catalog.iter().filter(|p| app.tab.catalog_new.contains(&p.no)).count();
            if new > 0 {
                meta.push(format!("{new} new"));
            }
            let hidden = app.tab.catalog_marks.iter().filter(|m| m.hidden.is_some()).count();
            if hidden > 0 {
                meta.push(if app.show_hidden { format!("{hidden} hidden, shown") } else { format!("{hidden} hidden") });
            }
            if app.tab.catalog_sort != Sort::Bump {
                meta.push(app.tab.catalog_sort.as_str().into());
            }
            vec![site(), board().unwrap_or_default()]
        }
        View::Thread => {
            let mut return_crumbs = None;
            let th = app.tab.thread.as_ref();
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
            let uri = app.tab.board.as_ref().map(|b| format!("/{}/", b.uri)).unwrap_or_default();
            if let Some(c) = th.and_then(|th| th.conversation.as_ref()).filter(|_| app.tab.gallery.is_none()) {
                // The conversation's posts instead of the thread's.
                meta.retain(|m| !m.ends_with("posts") && !m.ends_with("post"));
                meta.insert(0, if c.capped { format!("first {}", plural(c.depth.len(), "post")) } else { plural(c.depth.len(), "post") });
                return_crumbs = Some(vec![site(), uri.clone(), truncate(&subject, 24), format!("Conversation of No.{}", c.anchor)]);
            }
            if let Some(crumbs) = return_crumbs {
                crumbs
            } else if let Some(g) = &app.tab.gallery {
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
        View::Saved => {
            meta.push(plural(app.store.saved.len(), "thread"));
            meta.push(human_size(app.store.saved.iter().map(|m| m.bytes).sum()));
            vec!["Saved".into()]
        }
        View::Settings => vec!["Settings".into()],
        View::Search => {
            let Some(s) = &app.tab.search else { return (vec!["Search".into()], Vec::new()) };
            match s.total {
                Some(t) => meta.push(format!("{} of {}", s.hits.len(), plural(t as usize, "result"))),
                None => meta.push(plural(s.hits.len(), "result")),
            }
            vec![site(), format!("/{}/", s.board), format!("Search: {}", truncate(&s.query, 40))]
        }
    };
    let mut spans: Vec<Span> = Vec::new();
    // An active filter or search, unless it's being typed (the footer shows that).
    let query = match app.tab.view {
        View::Thread => app.tab.thread.as_ref().filter(|th| !th.search.is_empty() && !app.searching).map(|th| {
            let k = th.matches.len();
            format!("/{}  {}", th.search, plural(k, "match").replace("matchs", "matches"))
        }),
        _ => Some(current_filter(app)).filter(|q| !q.is_empty() && !app.filtering).map(|q| format!("/{q}")),
    };
    if let Some(q) = query {
        spans.extend([chip(q, t.on_primary_container, t.primary_container), Span::raw("  ")]);
    }
    // A saved copy, read offline.
    if let Some(off) = app.tab.offline.filter(|_| app.tab.view == View::Thread) {
        spans.extend([chip(format!("saved {}", ago(off.saved, app.clock)), t.on_primary_container, t.primary_container), Span::raw(" ")]);
        if off.dead {
            spans.push(chip("dead", t.background, t.error));
        }
        spans.push(Span::raw("  "));
    }
    spans.extend([Span::styled(meta.join("  ·  "), dim()), Span::raw(" ")]);
    (crumbs, spans)
}

/// A status message: a ✓ or ! badge, then the text.
fn status_spans(s: &Status, t: Theme) -> Vec<Span<'static>> {
    let (mark, bg) = if s.error { ("!", t.error) } else { ("✓", t.success) };
    vec![
        Span::styled(format!(" {mark} "), bold(t.background).bg(bg)),
        Span::styled(format!(" {}", s.text), Style::new().fg(t.on_bar)),
    ]
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
        Some(("search", app.tab.thread.as_ref().map_or("", |th| th.search.as_str())))
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
                    (Some(_), Some(Status { text, error: false })) => format!("   {text}"),
                    (Some(_), _) => "   enter go   tab complete   esc cancel".into(),
                    _ => "   enter accept   esc clear".into(),
                },
                dim(),
            ),
        ])
    } else if let Some(label) = &app.tab.loading {
        Line::from(vec![
            Span::styled(format!(" {} ", SPINNER[app.tick % SPINNER.len()]), bold(t.primary)),
            Span::styled(format!("{label}…"), Style::new().fg(t.on_bar)),
        ])
    } else if let Some(s) = &app.status {
        Line::from([vec![Span::raw(" ")], status_spans(s, t)].concat())
    } else {
        let mut spans = vec![Span::raw(" ")];
        for (key, label) in footer_hints(app) {
            spans.extend([Span::styled(key, bold(t.primary)), Span::styled(format!(" {label}   "), dim())]);
        }
        Line::from(spans)
    };
    let d = &app.downloads;
    let mut right = Vec::new();
    if d.running > 0 {
        right.extend([chip(format!("⇣ {}/{}", d.done + d.skipped + d.failed, d.total), t.text, t.surface_high), Span::raw(" ")]);
    }
    if !app.refreshing.is_empty() {
        right.extend([chip(format!("↻ {}", app.refreshing.len()), t.text, t.surface_high), Span::raw(" ")]);
    }
    let right_w = right.iter().map(|s| s.width()).sum::<usize>() as u16;
    put(f, area.x, area.y, area.width.saturating_sub(right_w), line);
    put(f, area.right().saturating_sub(right_w), area.y, right_w, Line::from(right));
}

/// Key hints for the footer, with the configured keys.
fn footer_hints(app: &App) -> Vec<(String, &'static str)> {
    let k = |a| app.keys.key(a).to_string();
    let mut hints: Vec<(String, &'static str)> = match app.tab.view {
        View::Thread if app.tab.gallery.is_some() => vec![
            ("h/j/k/l".into(), "move"),
            ("enter".into(), "view"),
            (k(Action::Download), "save"),
            (k(Action::Copy), "copy URL"),
            ("esc".into(), "back to the post"),
        ],
        // What the keys do to the focused part, when there is one.
        View::Thread => match app.focused() {
            Some(Part::File(_)) => vec![
                ("enter".into(), "view"),
                (k(Action::Download), "save"),
                (k(Action::Copy), "copy URL"),
                (k(Action::Browser), "browser"),
                (k(Action::NextPart), "next"),
                (k(Action::Menu), "more"),
                ("esc".into(), "the post"),
            ],
            Some(Part::Link(crate::model::Target::Url(_))) => vec![
                ("enter".into(), "open"),
                (k(Action::Copy), "copy"),
                (k(Action::NextPart), "next"),
                (k(Action::Menu), "more"),
                ("esc".into(), "the post"),
            ],
            Some(Part::Link(crate::model::Target::Quote(_))) => vec![
                ("enter".into(), "go to it"),
                (k(Action::Copy), "copy link"),
                (k(Action::NextPart), "next"),
                (k(Action::Menu), "more"),
                ("esc".into(), "the post"),
            ],
            Some(Part::Replies) => vec![
                ("enter".into(), "show / hide replies"),
                (k(Action::NextPart), "next"),
                (k(Action::Menu), "more"),
                ("esc".into(), "the post"),
            ],
            None if app.tab.thread.as_ref().is_some_and(|t| t.conversation.is_some()) => {
                let capped = app.tab.thread.as_ref().and_then(|t| t.conversation.as_ref()).is_some_and(|c| c.capped);
                let mut hints = vec![
                    ("j/k".into(), "post"),
                    (k(Action::NextPart), "images & links"),
                    (k(Action::Menu), "more"),
                    ("enter".into(), "quote"),
                    (format!("esc/{}", k(Action::Conversation)), "whole thread"),
                ];
                if capped {
                    hints.push(("".into(), "the nearest 500 posts"));
                }
                hints
            }
            None => {
                let mut hints = vec![
                    ("j/k".into(), "post"),
                    (k(Action::NextPart), "images & links"),
                    (k(Action::Hints), "hints"),
                    (k(Action::Menu), "more"),
                    ("enter".into(), "quote"),
                ];
                // A post that's part of a conversation.
                let talks = app.tab.thread.as_ref().is_some_and(|t| {
                    t.current().is_some_and(|p| !t.backlinks[t.selected].is_empty() || p.quotes.iter().any(|q| t.index.contains_key(q)))
                });
                if talks {
                    hints.push((k(Action::Conversation), "conversation"));
                }
                hints.extend([(k(Action::JumpBack), "back"), (k(Action::Search), "search"), (k(Action::Watch), "watch")]);
                hints
            }
        },
        View::Catalog => vec![
            ("enter".into(), "open"),
            (k(Action::Hints), "hints"),
            (k(Action::Menu), "more"),
            (k(Action::Search), "filter"),
            (k(Action::View), "view"),
            (k(Action::Watch), "watch"),
            (k(Action::Sort), "sort"),
            (k(Action::Compact), "compact"),
            (k(Action::Reload), "reload"),
        ],
        View::Watched | View::History => vec![
            ("enter".into(), "open"),
            (k(Action::Menu), "more"),
            (k(Action::Remove), "remove"),
            (k(Action::Search), "filter"),
            (k(Action::Browser), "browser"),
        ],
        View::Saved => vec![
            ("enter".into(), "read"),
            (k(Action::Menu), "more"),
            (k(Action::Remove), "remove"),
            (k(Action::Search), "filter"),
        ],
        View::Settings => vec![("enter".into(), "change"), ("esc".into(), "back")],
        View::Search => vec![("enter".into(), "open the thread"), (k(Action::NextMatch), "more results"), ("esc".into(), "back")],
        _ => vec![
            ("enter".into(), "open"),
            (k(Action::Menu), "more"),
            (k(Action::Search), "filter"),
            (k(Action::Browser), "browser"),
            (k(Action::Reload), "reload"),
        ],
    };
    // A saved copy of a thread that's still up: the live one is a key away.
    if app.tab.view == View::Thread && app.tab.gallery.is_none() && app.focused().is_none() && app.tab.offline.is_some_and(|o| !o.dead) {
        hints.insert(0, (k(Action::Reload), "live thread"));
    }
    hints.push((k(Action::Settings), "settings"));
    hints.push((k(Action::Help), "help"));
    hints
}

fn current_filter(app: &App) -> &str {
    match app.tab.view {
        View::Sites => &app.site_list.filter,
        View::Boards => &app.tab.board_list.filter,
        View::Catalog => &app.tab.catalog_list.filter,
        View::Watched => &app.watched_list.filter,
        View::History => &app.history_list.filter,
        View::Saved => &app.saved_list.filter,
        View::Thread | View::Settings | View::Search => "",
    }
}

// ----- lists and cards -----

/// The first row to show so that row `sel` is among the `fit` shown, moving as little as
/// possible from `offset`.
fn scroll_to(offset: usize, sel: usize, fit: usize) -> usize {
    offset.min(sel).max((sel + 1).saturating_sub(fit))
}

/// A row's background (the selection's when selected, else `bg` if any), and the accent
/// stripe at its left when it's selected or marked.
fn paint_row(f: &mut Frame, row: Rect, bg: Option<Color>, selected: bool, marked: bool) {
    let t = theme();
    if let Some(c) = if selected { Some(t.selection) } else { bg } {
        fill(f, row, c);
    }
    if selected || marked {
        fill(f, Rect { width: 1, ..row }, t.primary);
    }
}

/// Draw `count` items `height` rows tall with `gap` rows of background between them;
/// `build` makes only the ones on screen. With `card` each item sits on that color; the
/// selected one gets the selection color and an accent stripe, as do those `stripe` marks
/// (highlighted by a filter). Returns where they went, for mouse clicks.
#[allow(clippy::too_many_arguments)]
fn draw_rows(
    f: &mut Frame,
    area: Rect,
    count: usize,
    state: &mut ListState,
    (height, gap): (u16, u16),
    card: Option<Color>,
    stripe: &dyn Fn(usize) -> bool,
    build: &mut dyn FnMut(usize) -> Vec<Line<'static>>,
) -> Option<Hit> {
    if count == 0 || area.is_empty() {
        return None;
    }
    let per = height + gap;
    let fit = ((area.height + gap) / per).max(1) as usize;
    let sel = state.selected().unwrap_or(0).min(count - 1);
    let off = scroll_to(state.offset(), sel, fit);
    *state.offset_mut() = off;
    for k in off..count {
        let y = area.y + (k - off) as u16 * per;
        if y >= area.bottom() {
            break;
        }
        let h = height.min(area.bottom() - y);
        let row = Rect::new(area.x, y, area.width, h);
        paint_row(f, row, card, k == sel, stripe(k));
        for (r, line) in build(k).into_iter().take(h as usize).enumerate() {
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
    let rows = app.visible_sites();
    let mut state = app.site_list.state;
    app.hit = draw_rows(f, area, rows.len(), &mut state, (1, 0), None, &|_| false, &mut |k| {
        match rows[k] {
            SiteRow::Watched => {
                let n = app.store.watched.len();
                let unread: usize = app.store.watched.iter().map(|w| w.unread).sum();
                let mut spans = vec![
                    Span::styled("◉  ", Style::new().fg(t.primary)),
                    Span::styled(format!("{:<16}", "Watched"), bold(t.text)),
                    Span::styled(plural(n, "thread"), dim()),
                ];
                if unread > 0 {
                    spans.extend([Span::raw("  "), chip(format!("{unread} new"), t.background, t.new)]);
                }
                vec![Line::from(spans)]
            }
            SiteRow::History => vec![Line::from(vec![
                Span::styled("◷  ", Style::new().fg(t.primary)),
                Span::styled(format!("{:<16}", "History"), bold(t.text)),
                Span::styled(format!("{} recent", plural(app.store.history.len(), "thread")), dim()),
            ])],
            SiteRow::Saved => {
                let dead = app.store.saved.iter().filter(|m| m.dead).count();
                let mut spans = vec![
                    Span::styled("▤  ", Style::new().fg(t.primary)),
                    Span::styled(format!("{:<16}", "Saved"), bold(t.text)),
                    Span::styled(plural(app.store.saved.len(), "thread"), dim()),
                ];
                if dead > 0 {
                    spans.push(Span::styled(format!(", {dead} gone from the site"), dim()));
                }
                vec![Line::from(spans)]
            }
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
                let r = app.recent_board(i);
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
        }
    });
    app.site_list.state = state;
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
    let rows = app.visible_watched();
    let mut state = app.watched_list.state;
    app.hit = draw_rows(f, area, rows.len(), &mut state, (1, 0), None, &|_| false, &mut |k| {
        let w = &app.store.watched[rows[k]];
        let mut right = Vec::new();
        if app.refreshing.contains(&w.key) {
            right.push(Span::styled("↻  ", Style::new().fg(t.primary)));
        }
        if let Some(g) = &w.general {
            right.extend([chip(format!("follows {g}"), t.on_primary_container, t.primary_container), Span::raw(" ")]);
        }
        if w.at_limit && !w.dead {
            right.extend([chip("bump limit", t.text_dim, t.surface_high), Span::raw(" ")]);
        }
        if w.replies > 0 {
            let n = w.replies;
            right.extend([chip(format!("{n} repl{} to you", if n == 1 { "y" } else { "ies" }), t.on_primary, t.primary), Span::raw(" ")]);
        }
        if w.dead {
            right.extend([chip("archived/deleted", t.background, t.error), Span::raw("  ")]);
        } else if w.unread > 0 {
            right.extend([chip(format!("{} new", w.unread), t.background, t.new), Span::raw("  ")]);
        }
        right.push(Span::styled(plural(w.posts, "post"), dim()));
        vec![spread(thread_row(&w.key, &w.subject), right, width)]
    });
    app.watched_list.state = state;
    if app.hit.is_none() {
        let msg = format!("No watched threads. Press {} in a catalog or thread to watch one.", app.keys.key(Action::Watch));
        empty(f, area, &msg);
    }
}

fn draw_history(f: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.saturating_sub(PAD + 1) as usize;
    let rows = app.visible_history();
    let mut state = app.history_list.state;
    app.hit = draw_rows(f, area, rows.len(), &mut state, (1, 0), None, &|_| false, &mut |k| {
        let v = &app.store.history[rows[k]];
        vec![spread(thread_row(&v.key, &v.subject), vec![Span::styled(ago(v.opened, app.clock), dim())], width)]
    });
    app.history_list.state = state;
    if app.hit.is_none() {
        empty(f, area, "No history yet");
    }
}

fn draw_saved(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let width = area.width.saturating_sub(PAD + 1) as usize;
    let rows = app.visible_saved();
    let mut state = app.saved_list.state;
    app.hit = draw_rows(f, area, rows.len(), &mut state, (1, 0), None, &|_| false, &mut |k| {
        let m = &app.store.saved[rows[k]];
        let mut right = Vec::new();
        if m.dead {
            right.extend([chip("dead", t.background, t.error), Span::raw(" ")]);
        } else if app.store.watched(&m.key).is_some() {
            right.extend([chip("watching", t.on_primary_container, t.primary_container), Span::raw(" ")]);
        }
        right.push(Span::styled(format!(" {}  ·  {}", plural(m.posts, "post"), ago(m.saved, app.clock)), dim()));
        let mut left = thread_row(&m.key, &m.subject);
        left.push(Span::styled(format!("  No.{}", m.key.no), dim()));
        vec![spread(left, right, width)]
    });
    app.saved_list.state = state;
    if app.hit.is_none() {
        let msg = format!(
            "No saved threads. Watched threads are saved as they refresh ({} watches one), and {} saves one.",
            app.keys.key(Action::Watch),
            app.keys.key(Action::Export)
        );
        empty(f, area, &msg);
    }
}

fn draw_boards(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let width = app.boards().iter().map(|b| b.uri.width()).max().unwrap_or(1) + 4;
    let rows = app.visible_boards();
    let mut state = app.tab.board_list.state;
    app.hit = draw_rows(f, area, rows.len(), &mut state, (1, 0), None, &|_| false, &mut |k| {
        let b = &app.boards()[rows[k]];
        let mut spans = vec![
            Span::styled(format!("{:<width$}", format!("/{}/", b.uri)), bold(t.primary)),
            Span::styled(b.title.clone(), Style::new().fg(t.text)),
        ];
        if b.nsfw == Some(true) {
            spans.push(Span::styled("  nsfw", Style::new().fg(t.error)));
        }
        vec![Line::from(spans)]
    });
    app.tab.board_list.state = state;
    if app.hit.is_none() && app.tab.loading.is_none() {
        empty(f, area, "No boards");
    }
}

fn draw_catalog(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let images = app.images.enabled() && area.width >= MIN_THUMB_WIDTH;
    // The grid needs thumbnails; without them it's cards.
    let layout = app.layout();
    if layout == CatalogLayout::Grid && images {
        draw_grid(f, app, area);
        return;
    }
    app.grid_cols = 0;
    let compact = layout == CatalogLayout::Compact;
    let thumbs = images && !compact;
    let width = area.width.saturating_sub(PAD + 2) as usize;
    let visible = app.visible_catalog();
    let mut build = |k: usize| -> Vec<Line<'static>> {
        let i = visible[k];
        let p = &app.tab.catalog[i];
        let mark = app.tab.catalog_marks.get(i).cloned().unwrap_or_default();
        let mut head = Vec::new();
        if let Some(label) = &mark.hidden {
            head.extend([chip(hidden_label(label), t.text_dim, t.surface_high), Span::raw(" ")]);
        }
        if let Some(label) = &mark.highlight {
            head.extend([chip(label.clone(), t.on_primary_container, t.primary_container), Span::raw(" ")]);
        }
        if app.tab.catalog_new.contains(&p.no) {
            head.extend([chip("new", t.background, t.new), Span::raw(" ")]);
        }
        if p.sticky {
            head.extend([chip("pinned", t.text_dim, t.surface_high), Span::raw(" ")]);
        }
        if p.locked {
            head.extend([chip("locked", t.text_dim, t.surface_high), Span::raw(" ")]);
        }
        // Overboards show where each thread lives.
        if let Some(b) = p.board.as_ref().filter(|b| app.tab.board.as_ref().is_some_and(|cur| cur.uri != **b)) {
            head.extend([chip(format!("/{b}/"), t.on_primary_container, t.primary_container), Span::raw(" ")]);
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
        lines.extend(excerpt(p.plain_text(), dim(), text_w, rows));
        if thumbs {
            lines.resize(CAT_THUMB.height as usize, Line::raw(""));
            lines = beside_tile(lines, CAT_THUMB.width + 1);
        }
        lines
    };
    let (height, gap, card) = if compact {
        (1, 0, None)
    } else if thumbs {
        (CAT_THUMB.height, 1, Some(t.surface))
    } else {
        (2, 1, Some(t.surface))
    };
    let highlighted = |k: usize| app.tab.catalog_marks.get(visible[k]).is_some_and(|m| m.highlight.is_some());
    let mut state = app.tab.catalog_list.state;
    app.hit = draw_rows(f, area, visible.len(), &mut state, (height, gap), card, &highlighted, &mut build);
    app.tab.catalog_list.state = state;
    if app.hit.is_none() {
        if app.tab.loading.is_none() {
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
    let offset = app.tab.catalog_list.state.offset();
    for (k, &i) in visible.iter().enumerate().skip(offset).take(on_screen * 2) {
        let Some(file) = app.tab.catalog[i].files.first() else { continue };
        let row = (k - offset) as u16 * per;
        if row < area.height {
            let tile = Rect::new(area.x + PAD, area.y + row, CAT_THUMB.width, CAT_THUMB.height);
            draw_tile(f, &mut app.images, file, app.tab.catalog[i].files.len(), tile, area);
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
    let Some(s) = &app.tab.search else { return };
    let width = area.width.saturating_sub(PAD + 2) as usize;
    let more = app.more_results();
    let mut build = |k: usize| -> Vec<Line<'static>> {
        let (thread, p) = &s.hits[k];
        let mut head = vec![Span::styled(p.name.clone(), bold(t.name)), Span::raw("  ")];
        if p.no == *thread {
            head.extend([chip("OP", t.on_primary_container, t.primary_container), Span::raw(" ")]);
        } else {
            head.push(Span::styled(format!("in thread {thread}  "), dim()));
        }
        if let Some(subject) = &p.subject {
            head.push(Span::styled(subject.clone(), bold(t.text)));
        }
        let right = vec![Span::styled(format!("No.{}  ·  {}", p.no, ago(p.time, app.clock)), dim())];
        let mut lines = vec![spread(head, right, width)];
        lines.extend(excerpt(p.plain_text(), Style::new().fg(t.text), width, 2));
        lines
    };
    let mut state = app.tab.search_list.state;
    let hit = draw_rows(f, area, s.hits.len(), &mut state, (3, 1), Some(t.surface), &|_| false, &mut build);
    app.tab.search_list.state = state;
    if hit.is_none() && app.tab.loading.is_none() {
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
    let Some(g) = &mut app.tab.gallery else { return };
    let (card_w, card_h) = (THUMB.width + 4, THUMB.height + 1);
    let (cell_w, cell_h) = (card_w + 2, card_h + 1);
    let cols = ((area.width + 2) / cell_w).max(1) as usize;
    let rows = ((area.height + 1) / cell_h).max(1) as usize;
    g.cols = cols;
    let n = g.files.len();
    let sel = g.state.selected().unwrap_or(0).min(n - 1);
    let top = scroll_to(g.state.offset() / cols, sel / cols, rows);
    *g.state.offset_mut() = top * cols;
    let posts = app.tab.thread.as_ref().map(|t| &t.posts);
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
        paint_row(f, card, Some(t.surface), k == sel, false);
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
        if app.tab.loading.is_none() {
            empty(f, area, "No threads");
        }
        return;
    }
    let (cell_w, cell_h) = (GRID_CARD.width + 2, GRID_CARD.height + 1);
    let cols = ((area.width + 2) / cell_w).max(1) as usize;
    let rows = ((area.height + 1) / cell_h).max(1) as usize;
    app.grid_cols = cols;
    let state = &mut app.tab.catalog_list.state;
    let sel = state.selected().unwrap_or(0).min(visible.len() - 1);
    let top = scroll_to(state.offset() / cols, sel / cols, rows);
    *state.offset_mut() = top * cols;
    for (k, &i) in visible.iter().enumerate().skip(top * cols).take((rows + 1) * cols) {
        let (r, c) = (k / cols - top, k % cols);
        let (x, y) = (area.x + c as u16 * cell_w, area.y + r as u16 * cell_h);
        let card = Rect::new(x, y, GRID_CARD.width, GRID_CARD.height).intersection(area);
        if card.is_empty() {
            // Below the screen: prefetch from media hosts.
            if let Some(url) = app.tab.catalog[i].files.first().and_then(|f| f.thumb.as_ref()).filter(|u| http::is_media_host(u)) {
                app.images.want(url, Kind::Thumb);
            }
            continue;
        }
        let p = &app.tab.catalog[i];
        let mark = app.tab.catalog_marks.get(i).cloned().unwrap_or_default();
        paint_row(f, card, Some(t.surface), k == sel, mark.highlight.is_some());
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
        if app.tab.catalog_new.contains(&p.no) {
            head.extend([chip("new", t.background, t.new), Span::raw(" ")]);
        }
        if let Some(label) = &mark.hidden {
            head.extend([chip(hidden_label(label), t.text_dim, t.surface_high), Span::raw(" ")]);
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

/// `text` wrapped to `width`, cut to `rows` lines with "…" if there's more.
fn excerpt(text: &str, style: Style, width: usize, rows: usize) -> Vec<Line<'static>> {
    let mut lines = markup::wrap(&Line::styled(text.to_string(), style), width);
    if lines.len() > rows {
        lines.truncate(rows);
        let last = lines.pop().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>() + "…").unwrap_or_default();
        lines.push(Line::styled(truncate(&last, width), style));
    }
    lines
}

/// `lines` after a blank column `width` wide (where a tile is painted), each keeping its
/// style (code lines are marked by it).
fn beside_tile(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let blank = Span::raw(" ".repeat(width as usize));
    lines.into_iter().map(|l| Line::from([vec![blank.clone()], l.spans].concat()).style(l.style)).collect()
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
        // Offline, not cached: what kind of file it is.
        State::Unavailable => label(f, kind, dim()),
    }
}

// ----- the thread -----

fn draw_thread(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(t) = &mut app.tab.thread else {
        if app.tab.loading.is_some() {
            return;
        }
        match app.tab.saved_offer.as_ref().and_then(|k| app.store.saved(k)) {
            Some(m) => empty(f, area, &format!("The thread is gone. enter opens its saved copy from {}.", ago(m.saved, app.clock))),
            None => empty(f, area, "Thread not loaded"),
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
            t.scroll = (l.starts[i] + off).min(l.len().saturating_sub(t.viewport));
        }
        t.layout = Some(l);
        t.scroll_to_selected();
    }
    let Some(l) = t.layout.as_ref() else { return };
    let cursor = t.entry();
    // A part just focused is scrolled into view (the post itself: its top).
    if std::mem::take(&mut t.follow_focus) {
        let at = l.spots.get(cursor).and_then(|s| s.iter().find(|s| Some(&s.part) == t.focus.as_ref()));
        let line = l.starts[cursor] + at.map_or(0, |s| s.line);
        if line < t.scroll {
            t.scroll = line;
        } else if line >= t.scroll + t.viewport {
            t.scroll = (line + 2).saturating_sub(t.viewport).min(l.len().saturating_sub(t.viewport));
        }
    }
    for row in 0..area.height {
        let i = t.scroll + row as usize;
        let Some((e, line)) = l.line(i) else { break };
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
        let marked = t.marks.get(entry.post).is_some_and(|m| m.highlight.is_some());
        paint_row(f, Rect::new(x, y, width, 1), Some(card), e == cursor, marked);
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
    let total = l.len();
    if view > 0 && total > view && area.right() < f.area().right() {
        let len = ((view * view) / total).max(1) as u16;
        let pos = (t.scroll * (view - len as usize) / (total - view).max(1)) as u16;
        fill(f, Rect::new(area.right() + 1, area.y + pos, 1, len), th.surface_high);
    }
}

/// Lay out every entry (post, or reply shown inline) as a card of wrapped lines: a padding
/// line above and below the content, then a gap line. Posts with files get a thumbnail
/// tile on the left.
fn layout_thread(t: &mut ThreadView, width: u16, thumbs: bool, clock: Clock) -> ThreadLayout {
    let old = std::mem::take(&mut t.cache);
    let mut cache = LineCache::new();
    let n = t.entries.len();
    let (mut blocks, mut spots, mut starts, mut thumb_at) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n + 1), Vec::new());
    let cursor = t.entry();
    let mut len = 0;
    for (e, entry) in t.entries.iter().enumerate() {
        let (i, p) = (entry.post, &t.posts[entry.post]);
        let text_width = width.saturating_sub(PAD + 2 + INDENT * entry.depth as u16).max(10) as usize;
        let thumb = thumbs && !p.files.is_empty() && text_width > THUMB.width as usize + 12;
        starts.push(len);
        let (block, at): (Rc<[Line<'static>]>, Rc<[Spot]>) = if t.is_collapsed(i) {
            // A hidden post is one line, so replies to it still make sense.
            let why = t.marks.get(i).and_then(|m| m.hidden.as_deref()).filter(|l| !l.is_empty());
            let why = why.map_or("hidden".to_string(), |l| format!("hidden by the filter \"{l}\""));
            (vec![Line::styled(format!("No.{}  {why}", p.no), Style::new().fg(theme().text_dim)), Line::raw("")].into(), Rc::from([]))
        } else {
            let mut ctx = post_ctx(t, i, clock);
            ctx.focus = t.focus.as_ref().filter(|_| e == cursor);
            let key = (p.no, text_width as u16, thumb);
            let shows = shown_with(t, i, p, &ctx);
            let (block, at) = match cache.get(&key).or(old.get(&key)).filter(|(h, ..)| *h == shows) {
                Some((_, block, at)) => (block.clone(), at.clone()),
                None => {
                    // A padding line above: the spots are a line further down.
                    let mut lines = vec![Line::raw("")];
                    let (text, mut at) = if thumb {
                        let (text, at) = post_lines(p, &ctx, text_width - THUMB.width as usize - 2);
                        let mut text = beside_tile(text, THUMB.width + 2);
                        text.resize(text.len().max(THUMB.height as usize), Line::raw(""));
                        (text, at.into_iter().map(|s| Spot { col: s.col + THUMB.width + 2, ..s }).collect())
                    } else {
                        post_lines(p, &ctx, text_width)
                    };
                    lines.extend(text);
                    lines.extend([Line::raw(""), Line::raw("")]);
                    for s in &mut at {
                        s.line += 1;
                    }
                    (Rc::from(lines), Rc::from(at))
                }
            };
            cache.insert(key, (shows, block.clone(), at.clone()));
            if thumb {
                thumb_at.push((len + 1, e));
            }
            (block, at)
        };
        len += block.len();
        blocks.push(block);
        spots.push(at);
    }
    starts.push(len);
    t.cache = cache;
    ThreadLayout { width, blocks, starts, thumbs: thumb_at, spots }
}

/// A hash of what a post's lines show besides the post itself, for the line cache: its
/// "3h ago", OP/new/yours chips, revealed spoilers, filter marks, backlinks, and the search
/// highlight where it can appear.
fn shown_with(t: &ThreadView, i: usize, p: &Post, ctx: &PostCtx) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let quotes_marked = p.quotes.iter().any(|q| *q == ctx.op_no || ctx.mine.contains(q));
    // Matches are found in the post's own text; the " (OP)" / " (You)" added to quotes can
    // only be highlighted by a search for (part of) them.
    let s = ctx.search.as_str();
    let in_added = !s.is_empty() && (s.contains(['(', ')']) || " (op)".contains(s) || " (you)".contains(s));
    let highlighted = t.matches.binary_search(&i).is_ok() || (quotes_marked && in_added);
    (ago(p.time, ctx.clock), ctx.is_op, ctx.is_new, ctx.reveal, ctx.mark, ctx.mine.contains(&p.no), quotes_marked, ctx.backlinks).hash(&mut h);
    (ctx.focus, ctx.anchor).hash(&mut h);
    if highlighted {
        ctx.search.hash(&mut h);
    }
    h.finish()
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
    /// The focused part, when this is the selected entry.
    focus: Option<&'a Part>,
    /// The post a conversation is shown for.
    anchor: bool,
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
        focus: None,
        anchor: t.conversation.as_ref().is_some_and(|c| c.anchor == t.posts[i].no),
    }
}

fn search_hl() -> Style {
    let t = theme();
    Style::new().fg(t.on_search).bg(t.search)
}

/// A post as wrapped lines, and where its parts landed in them. Parts are drawn with a
/// tag (an underline color nothing else uses) that's found after wrapping, then cleared.
fn post_lines(p: &Post, ctx: &PostCtx, width: usize) -> (Vec<Line<'static>>, Vec<Spot>) {
    let t = theme();
    let parts = crate::app::parts(p, ctx.backlinks);
    let index = |part: &Part| parts.iter().position(|x| x == part);
    let tag = |style: Style, k: usize| Style { underline_color: Some(Color::Rgb(0xfe, (k >> 8) as u8, k as u8)), ..style };
    let mut head = vec![Span::styled(p.name.clone(), bold(t.name)), Span::raw("  ")];
    if ctx.is_op {
        head.extend([chip("OP", t.on_primary_container, t.primary_container), Span::raw(" ")]);
    }
    if ctx.is_new {
        head.extend([chip("new", t.background, t.new), Span::raw(" ")]);
    }
    if ctx.anchor {
        head.extend([chip("conversation", t.on_primary, t.primary), Span::raw(" ")]);
    }
    if ctx.mine.contains(&p.no) {
        head.extend([chip("you", t.on_primary, t.primary), Span::raw(" ")]);
    }
    if let Some(m) = ctx.mark {
        if let Some(label) = &m.hidden {
            head.extend([chip(hidden_label(label), t.text_dim, t.surface_high), Span::raw(" ")]);
        }
        if let Some(label) = &m.highlight {
            head.extend([chip(label.clone(), t.on_primary_container, t.primary_container), Span::raw(" ")]);
        }
    }
    head.push(Span::styled(format!("{}  No.{}", fmt_time(p.time, ctx.clock), p.no), dim()));
    let mut out = markup::wrap(&Line::from(head), width);
    if let Some(s) = &p.subject {
        let subject = markup::highlight(&Line::styled(s.clone(), bold(t.primary)), &ctx.search, search_hl());
        out.extend(markup::wrap(&subject, width));
    }
    for (k, file) in p.files.iter().enumerate() {
        let meta = file_facts(file);
        let meta = if meta.is_empty() { String::new() } else { format!("  {}", meta.join(" · ")) };
        let k = index(&Part::File(k)).unwrap_or(usize::MAX);
        let line = Line::from(vec![Span::styled(file.filename.clone(), tag(Style::new().fg(t.text), k)), Span::styled(meta, dim())]);
        out.extend(markup::wrap(&line, width));
    }
    if !p.body.is_empty() {
        out.push(Line::raw(""));
    }
    for (n, line) in p.body.iter().enumerate() {
        // Links first, by where they are in the parsed text (before anything is added to it).
        let links: Vec<(usize, usize, usize)> = p
            .anchors
            .iter()
            .filter(|a| a.line == n)
            .filter_map(|a| Some((a.start, a.end, index(&Part::Link(a.to.clone()))?)))
            .collect();
        let line = if links.is_empty() {
            line.clone()
        } else {
            let ranges: Vec<(usize, usize)> = links.iter().map(|&(a, b, _)| (a, b)).collect();
            markup::restyle(line, &ranges, |style, r| tag(style, links[r].2))
        };
        let mut line = if ctx.reveal { markup::reveal(&line) } else { line };
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
        let replies = index(&Part::Replies).unwrap_or(usize::MAX);
        let mut spans = vec![Span::styled("Replies", tag(dim(), replies)), Span::raw("  ")];
        for &no in ctx.backlinks {
            let k = index(&Part::Link(crate::model::Target::Quote(crate::model::Link { board: None, thread: None, post: Some(no) })));
            spans.extend([Span::styled(format!(">>{no}"), tag(Style::new().fg(t.quotelink), k.unwrap_or(usize::MAX))), Span::raw("  ")]);
        }
        out.extend(markup::wrap(&Line::from(spans), width));
    }
    // Find the tags: where each part is, and the focused one's look.
    let focused = ctx.focus.and_then(index);
    let mut spots: Vec<Spot> = Vec::new();
    for (row, line) in out.iter_mut().enumerate() {
        let mut col = 0u16;
        for s in &mut line.spans {
            let w = s.width() as u16;
            if let Some(Color::Rgb(0xfe, hi, lo)) = s.style.underline_color {
                s.style.underline_color = None;
                let k = (hi as usize) << 8 | lo as usize;
                if let Some(part) = parts.get(k) {
                    if focused == Some(k) {
                        s.style = s.style.fg(t.on_primary).bg(t.primary);
                    }
                    match spots.last_mut() {
                        Some(last) if last.part == *part && last.line == row && last.col + last.width == col => last.width += w,
                        _ => spots.push(Spot { part: part.clone(), line: row, col, width: w }),
                    }
                }
            }
            col += w;
        }
    }
    (out, spots)
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

/// A focused quote of a post in this thread shows that post, without taking the keys: at
/// the bottom of the thread, or the top when the quote is down there.
fn draw_peek(f: &mut Frame, app: &App, area: Rect) {
    use crate::model::Target;
    let Some(Part::Link(Target::Quote(l))) = app.focused() else { return };
    let Some(t) = &app.tab.thread else { return };
    let Some(&i) = l.post.filter(|_| l.board.is_none() && l.thread.is_none_or(|n| n == t.no)).and_then(|n| t.index.get(&n)) else { return };
    let th = theme();
    let width = area.width.saturating_sub(6);
    let mut lines = post_lines(&t.posts[i], &post_ctx(t, i, app.clock), width as usize).0;
    let h = (lines.len() as u16 + 2).min(area.height / 2).max(3);
    lines.truncate(h.saturating_sub(2) as usize);
    // Where the quote is on screen decides where the peek goes.
    let quote_y = t.layout.as_ref().and_then(|lay| {
        let e = t.entry();
        let s = lay.spots.get(e)?.iter().find(|s| app.focused() == Some(&s.part))?;
        Some((lay.starts.get(e)? + s.line).saturating_sub(t.scroll))
    });
    let top = quote_y.is_some_and(|y| y as u16 >= area.height / 2);
    let y = if top { area.y } else { area.bottom().saturating_sub(h) };
    let r = Rect::new(area.x + 2, y, area.width.saturating_sub(4), h);
    f.render_widget(Clear, r);
    fill(f, r, th.surface_highest);
    fill(f, Rect::new(r.x, r.y, 1, r.height), th.primary);
    for (row, line) in lines.into_iter().enumerate() {
        put(f, r.x + 2, r.y + 1 + row as u16, r.width.saturating_sub(4), line);
    }
}

/// The menu for what's selected: each thing that can be done, with its key.
fn draw_menu(f: &mut Frame, app: &mut App) {
    let t = theme();
    let keys: Vec<String> = app.menu.as_ref().map(|m| m.items.iter().map(|i| app.menu_key(i)).collect()).unwrap_or_default();
    let Some(m) = &mut app.menu else { return };
    let key_w = keys.iter().map(|k| k.width()).max().unwrap_or(1).max(5);
    let label = |i: &crate::app::MenuItem| match i {
        crate::app::MenuItem::Enter(l) | crate::app::MenuItem::Act(_, l) => l.clone(),
    };
    let w = (m.items.iter().map(|i| label(i).width()).max().unwrap_or(10) + key_w + 8).max(m.title.width() + 24) as u16;
    let title = if m.title.is_empty() { "Actions".to_string() } else { m.title.clone() };
    let inner = panel(f, w.min(90), m.items.len() as u16 + 3, &title, "enter run · esc close");
    let rows = inner.height as usize;
    let sel = m.list.selected().unwrap_or(0);
    let off = scroll_to(m.list.offset(), sel, rows);
    *m.list.offset_mut() = off;
    m.area = inner;
    for (k, item) in m.items.iter().enumerate().skip(off).take(rows) {
        let y = inner.y + (k - off) as u16;
        paint_row(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), None, k == sel, false);
        let key = keys.get(k).cloned().unwrap_or_default();
        let line = Line::from(vec![Span::styled(format!("{key:<key_w$}  "), bold(t.primary)), Span::styled(label(item), Style::new().fg(t.text))]);
        put(f, inner.x, y, inner.width, line);
    }
}

/// Link hints: each label where its target is; typed letters dim, the rest bright.
fn draw_hints(f: &mut Frame, app: &App) {
    let t = theme();
    let Some(h) = &app.hints else { return };
    let area = f.area();
    for target in h.targets.iter().filter(|x| x.label.starts_with(&h.typed)) {
        if !area.contains(ratatui::layout::Position::new(target.x, target.y)) {
            continue;
        }
        let rest = target.label.get(h.typed.len()..).unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(h.typed.clone(), Style::new().fg(t.text_dim).bg(t.new)),
            Span::styled(rest.to_string(), bold(t.background).bg(t.new)),
        ]);
        put(f, target.x, target.y, (target.label.len() as u16).min(area.right().saturating_sub(target.x)), line);
    }
}

fn draw_preview(f: &mut Frame, app: &App) {
    let (Some(p), Some(t)) = (&app.tab.preview, &app.tab.thread) else { return };
    let w = f.area().width.saturating_sub(8).clamp(20, 110);
    let width = w.saturating_sub(4) as usize;
    let mut lines = Vec::new();
    for &i in &p.posts {
        lines.extend(post_lines(&t.posts[i], &post_ctx(t, i, app.clock), width).0);
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
    let Some(p) = &mut app.tab.links else { return };
    let w = f.area().width.saturating_sub(8).clamp(20, 110);
    let inner = panel(f, w, p.items.len() as u16 + 3, "Links", "enter open · y copy · esc close");
    let rows = inner.height as usize;
    let sel = p.list.selected().unwrap_or(0);
    let off = scroll_to(p.list.offset(), sel, rows);
    *p.list.offset_mut() = off;
    p.area = inner;
    for (k, item) in p.items.iter().enumerate().skip(off).take(rows) {
        let y = inner.y + (k - off) as u16;
        paint_row(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), None, k == sel, false);
        let (kind, text, extra) = match item {
            LinkItem::Quote(_, label) => ("quote", label.clone(), String::new()),
            LinkItem::Url(u) => ("web", u.clone(), String::new()),
            LinkItem::File(file) => ("file", file.filename.clone(), format!("  {}", file.url)),
        };
        let room = (inner.width as usize).saturating_sub(9);
        let text = truncate(&text, room);
        let extra = truncate(&extra, room.saturating_sub(text.width()));
        let line = Line::from(vec![
            chip(format!("{kind:<5}"), t.text_dim, t.surface_high),
            Span::raw("  "),
            Span::styled(text, Style::new().fg(if kind == "file" { t.text } else { t.quotelink })),
            Span::styled(extra, dim()),
        ]);
        put(f, inner.x, y, inner.width, line);
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
    let off = scroll_to(p.list.offset(), sel, rows);
    *p.list.offset_mut() = off;
    p.area = inner;
    for (k, row) in p.rows.iter().enumerate().skip(off).take(rows) {
        let y = inner.y + (k - off) as u16;
        let line = match row {
            Err(file) => Line::styled(truncate(file, inner.width as usize), bold(t.primary)),
            Ok((_, e)) => {
                paint_row(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), None, k == sel, false);
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
                ("j k g G ^d ^u".into(), "move, top/bottom, page"),
                ("enter l / esc h".into(), "open / back"),
                (k(Action::Search), "filter (thread: search)"),
                (pair(Action::Reload, Action::Browser), "reload / open in browser"),
                (k(Action::Goto), "go to a URL or site/board"),
                (k(Action::Settings), "settings: theme, keys, …"),
                (pair(Action::NextTab, Action::CloseTab), "next tab / close tab"),
                (format!("{}, ctrl-c", k(Action::Quit)), "quit (mouse works too)"),
            ],
        ),
        (
            "Home screen",
            vec![
                (format!("1-9 / {}", k(Action::Favorite)), "open / favorite a board"),
                (k(Action::Remove), "unfavorite, hide a site"),
            ],
        ),
        (
            "Watched, History, Saved",
            vec![
                (k(Action::Remove), "remove the entry (Saved: asks first)"),
                (pair(Action::NewTab, Action::Follow), "new tab / follow general"),
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
                (format!("{} / {} / {}", k(Action::Watch), k(Action::NewTab), k(Action::Follow)), "watch / new tab / general"),
                (k(Action::Favorite), "favorite this board"),
                (pair(Action::Sort, Action::Compact), "sort / layout (grid, …)"),
                (k(Action::Links), "the OP's links and files"),
                (format!("{} / {}", pair(Action::Hide, Action::ShowHidden), k(Action::Filter)), "hide / show hidden / filter"),
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
                (pair(Action::Expand, Action::Conversation), "replies under it / conversation"),
                (pair(Action::View, Action::Gallery), "view images / gallery"),
                (pair(Action::OpenFile, Action::ImageSearch), "open file / image search"),
                (k(Action::Links), "the post's links and files"),
                (format!("{} / {}", pair(Action::Download, Action::DownloadThread), k(Action::Export)), "save: files / all / page"),
                (format!("{} / {} / {}", k(Action::Watch), k(Action::NewTab), k(Action::Follow)), "watch / quote tab / general"),
                (format!("{} / {}", pair(Action::Hide, Action::ShowHidden), k(Action::Filter)), "hide / show hidden / filter"),
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
                let (item, label, hint) = items[i];
                paint_row(f, Rect::new(area.x, y, area.width, 1), None, i == selected, false);
                let value = app.setting_value(item);
                let hint_w = (area.width as usize).saturating_sub(PAD as usize + 1 + 18 + 34);
                let line = Line::from(vec![
                    Span::styled(format!("{label:<18}"), Style::new().fg(t.text)),
                    Span::styled(format!("{:<34}", truncate(&value, 32)), bold(t.text)),
                    Span::styled(if hint_w >= 16 { truncate(hint, hint_w) } else { String::new() }, dim()),
                ]);
                put(f, area.x + PAD, y, area.width.saturating_sub(PAD + 1), line);
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

fn draw_settings_popup(f: &mut Frame, app: &App) {
    let t = theme();
    match &app.settings_popup {
        Some(SettingsPopup::Themes { list, names, .. }) => {
            let inner = panel(f, 52, names.len() as u16 + 3, "Theme", "enter keep · esc cancel");
            let sel = list.selected().unwrap_or(0);
            for (k, name) in names.iter().enumerate().take(inner.height as usize) {
                let y = inner.y + k as u16;
                paint_row(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), None, k == sel, false);
                let mut spans = vec![Span::styled(format!("{name:<22}"), Style::new().fg(t.text))];
                // A row of colored cells: the theme at a glance.
                if let Ok(th) = theme::resolve(name, &app.themes) {
                    let colors = [th.background, th.surface, th.selection, th.primary, th.primary_container, th.greentext, th.quotelink, th.heading, th.new];
                    spans.extend(colors.map(|c| Span::styled("  ", Style::new().bg(c))));
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
                paint_row(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), None, k == sel, false);
                let c = t.get(role).unwrap_or(Color::Reset);
                let line = Line::from(vec![
                    Span::styled("    ", Style::new().bg(c)),
                    Span::styled(format!("  {role:<22}"), Style::new().fg(t.text)),
                    Span::styled(format!("{:<10}", theme::color_string(c)), bold(t.text)),
                    Span::styled(*desc, dim()),
                ]);
                put(f, inner.x, y, inner.width, line);
            }
            let y = inner.bottom().saturating_sub(1);
            let line = match editing {
                Some(text) => Line::from(vec![
                    Span::styled(format!("New {} color: ", ROLES.get(sel).map_or("", |r| r.0)), Style::new().fg(t.text)),
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
                paint_row(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), None, k == sel, false);
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
        Some(SettingsPopup::Filters { list, counts }) => draw_filter_list(f, app, list, counts),
        Some(SettingsPopup::FilterEdit { index, draft, row, typing }) => draw_filter_edit(f, app, *index, draft, *row, typing.as_deref()),
        Some(SettingsPopup::Folder { value }) => {
            let inner = panel(f, 90, 7, "Download folder", "enter save · esc cancel");
            let field = Rect::new(inner.x, inner.y, inner.width, 1);
            fill(f, field, t.surface);
            let line = Line::from(vec![Span::styled(value.clone(), Style::new().fg(t.text)), Span::styled("▏", Style::new().fg(t.primary))]);
            put(f, inner.x + 1, inner.y, inner.width.saturating_sub(2), line);
            let help = "{site}, {board}, {thread} and {downloads} are filled in. Empty for the default.";
            put(f, inner.x, inner.y + 2, inner.width, Line::styled(help, dim()));
        }
        None => {}
    }
}

// ----- filters -----

/// Where a filter applies, in words.
fn filter_scope(c: &crate::filter::FilterConfig) -> String {
    let boards = c.boards.iter().map(|b| format!("/{b}/")).collect::<Vec<_>>().join(" ");
    match (c.sites.is_empty(), c.boards.is_empty()) {
        (true, true) => "everywhere".into(),
        (false, true) => c.sites.join(", "),
        (true, false) => format!("{boards} on any site"),
        (false, false) => format!("{boards} on {}", c.sites.join(", ")),
    }
}

fn counts_text((posts, threads): (usize, usize)) -> String {
    match (posts, threads) {
        (0, 0) => "nothing here".into(),
        (p, 0) => plural(p, "post"),
        (0, t) => plural(t, "thread"),
        (p, t) => format!("{}, {}", plural(p, "post"), plural(t, "thread")),
    }
}

/// `X`: a filter like the selected post.
fn draw_add_filter(f: &mut Frame, app: &App) {
    use crate::filter::FilterAction;
    let t = theme();
    let Some(a) = &app.filter_add else { return };
    let h = a.candidates.len() as u16 + 9;
    let inner = panel(f, 72, h, &format!("Filter like No.{}", a.post), "enter add · esc cancel");
    let sel = a.list.selected().unwrap_or(0);
    for (k, c) in a.candidates.iter().enumerate().take(inner.height as usize) {
        let y = inner.y + k as u16;
        paint_row(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), None, k == sel, false);
        let room = (inner.width as usize).saturating_sub(c.field.as_str().len() + 2);
        let left = vec![Span::styled(truncate(&c.what, room), Style::new().fg(t.text))];
        put(f, inner.x, y, inner.width, spread(left, vec![Span::styled(c.field.as_str(), dim())], inner.width as usize));
    }
    let y = inner.y + a.candidates.len() as u16 + 1;
    let row = |f: &mut Frame, k: u16, name: &str, value: Vec<Span<'static>>, key: &str| {
        let mut spans = vec![Span::styled(format!("{name:<8}"), dim())];
        spans.extend(value);
        put(f, inner.x, y + k, inner.width, spread(spans, vec![Span::styled(key.to_string(), dim())], inner.width as usize));
    };
    let (verb, other) = match a.action {
        FilterAction::Hide => ("hide them", "a: highlight instead"),
        FilterAction::Highlight => ("highlight them", "a: hide instead"),
    };
    row(f, 0, "Do", vec![Span::styled(verb, bold(t.text))], other);
    row(f, 1, "Where", vec![Span::styled(a.reach_text(), bold(t.text))], "s: change");
    match &a.typing {
        Some(text) => row(f, 2, "Label", vec![Span::styled(text.clone(), bold(t.text)), Span::styled("▏", Style::new().fg(t.primary))], "enter: keep"),
        None => row(f, 2, "Label", vec![Span::styled(a.label(), bold(t.text))], "e: edit"),
    }
    let note = "Saved in the config as a [[filter]]; u right after takes it back.";
    put(f, inner.x, y + 4, inner.width, Line::styled(note, dim()));
}

/// Settings › Filters: every `[[filter]]`, with what it catches on screen now.
fn draw_filter_list(f: &mut Frame, app: &App, list: &ratatui::widgets::ListState, counts: &[(usize, usize)]) {
    let t = theme();
    let cfgs = &app.filter_cfgs;
    let h = (cfgs.len().max(1) as u16 + 6).min(f.area().height.saturating_sub(4));
    let inner = panel(f, 110, h, "Filters", "enter edit · space on/off · a add · x remove · esc close");
    let view = inner.height.saturating_sub(2) as usize;
    let sel = list.selected().unwrap_or(0);
    let first = (sel + 1).saturating_sub(view);
    if cfgs.is_empty() {
        let x = app.keys.key(Action::Filter);
        put(f, inner.x, inner.y, inner.width, Line::styled(format!("No filters yet. a adds one; {x} on a post makes one like it."), dim()));
    }
    let wide = inner.width >= 90;
    for (k, c) in cfgs.iter().enumerate().skip(first).take(view) {
        let y = inner.y + (k - first) as u16;
        paint_row(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), None, k == sel, false);
        let style = if c.enabled { Style::new().fg(t.text) } else { dim() };
        let fields = c.fields().iter().map(|f| f.as_str()).collect::<Vec<_>>().join("+");
        let count = counts.get(k).map_or(String::new(), |&n| counts_text(n));
        // Narrow: the label and fields share what the count leaves.
        let (label_w, fields_w) = if wide { (22, 18) } else { ((inner.width as usize).saturating_sub(count.width() + 15) * 3 / 5, (inner.width as usize).saturating_sub(count.width() + 15) * 2 / 5) };
        let mut left = vec![
            if c.enabled { chip(format!("{:<9}", c.action.as_str()), t.on_primary_container, t.primary_container) } else { chip(format!("{:<9}", "off"), t.text_dim, t.surface_high) },
            Span::raw("  "),
            Span::styled(format!("{:<label_w$}", truncate(c.label(), label_w.saturating_sub(1))), if c.enabled { bold(t.text) } else { dim() }),
            Span::styled(format!("{:<fields_w$}", truncate(&fields, fields_w.saturating_sub(1))), style),
        ];
        if wide {
            left.push(Span::styled(format!("{:<20}", truncate(&filter_scope(c), 19)), style));
            // The pattern gets what's left, beside the count.
            let used: usize = left.iter().map(|s| s.width()).sum::<usize>() + count.width() + 2;
            left.push(Span::styled(truncate(&c.pattern, (inner.width as usize).saturating_sub(used)), dim()));
        }
        let right = vec![Span::styled(count, dim())];
        put(f, inner.x, y, inner.width, spread(left, right, inner.width as usize));
    }
    let note = "Counts are for the open catalog and thread. Kept in the config as [[filter]] tables.";
    put(f, inner.x, inner.bottom().saturating_sub(1), inner.width, Line::styled(note, dim()));
}

/// One filter in the editor.
fn draw_filter_edit(f: &mut Frame, app: &App, index: Option<usize>, draft: &crate::filter::FilterConfig, row: usize, typing: Option<&str>) {
    use crate::app::{EDIT_ROWS, EditRow};
    let t = theme();
    let title = match index {
        Some(_) => format!("Filter: {}", draft.label()),
        None => "New filter".into(),
    };
    let hint = if typing.is_some() { "enter keep · esc cancel" } else { "enter change · esc back" };
    let inner = panel(f, 84, EDIT_ROWS.len() as u16 + 6, &title, hint);
    let fields = draft.fields();
    for (k, r) in EDIT_ROWS.iter().enumerate().take(inner.height.saturating_sub(2) as usize) {
        let y = inner.y + k as u16;
        paint_row(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), None, k == row, false);
        let (name, value): (String, String) = match r {
            EditRow::Pattern => ("Pattern".into(), draft.pattern.clone()),
            EditRow::Label => ("Label".into(), draft.label.clone().unwrap_or_else(|| "(the pattern)".into())),
            EditRow::Action => ("Action".into(), draft.action.as_str().into()),
            EditRow::Field(field) => (format!("  {}", field.as_str()), if fields.contains(field) { "✓ looked at".into() } else { "·".into() }),
            EditRow::Sites => ("Sites".into(), if draft.sites.is_empty() { "(any)".into() } else { draft.sites.join(", ") }),
            EditRow::Boards => ("Boards".into(), if draft.boards.is_empty() { "(any)".into() } else { draft.boards.join(", ") }),
            EditRow::Enabled => ("On".into(), if draft.enabled { "yes".into() } else { "no (kept, not applied)".into() }),
        };
        let mut spans = vec![Span::styled(format!("{name:<12}"), dim())];
        match typing.filter(|_| k == row) {
            Some(text) => spans.extend([Span::styled(text.to_string(), bold(t.text)), Span::styled("▏", Style::new().fg(t.primary))]),
            None => spans.push(Span::styled(truncate(&value, (inner.width as usize).saturating_sub(13)), Style::new().fg(t.text))),
        }
        put(f, inner.x, y, inner.width, Line::from(spans));
    }
    // What's wrong with what's being typed, as it's typed; else what it catches.
    let r = EDIT_ROWS.get(row).copied().unwrap_or(EditRow::Pattern);
    let line = match typing.map(|text| crate::app::filters_problem(&crate::app::filters_with_text(draft, r, text))) {
        Some(Some(e)) => Line::styled(e, Style::new().fg(t.error)),
        _ if draft.pattern.is_empty() => Line::styled("Type a pattern (a regex; an MD5 for the md5 field).", dim()),
        _ => Line::styled(format!("Catches {} now. Each change is saved in the config.", counts_text(app.filter_counts(draft))), dim()),
    };
    put(f, inner.x, inner.bottom().saturating_sub(1), inner.width, line);
}

// ----- the image viewer -----

fn draw_viewer(f: &mut Frame, app: &mut App) {
    let t = theme();
    let Some(v) = &app.tab.viewer else { return };
    let Some(file) = v.files.get(v.index) else { return };
    let [top, _, middle, _, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let mut meta = file_facts(file);
    meta.push(format!("{} of {}", v.index + 1, v.files.len()));
    fill(f, top, t.bar);
    let line = Line::from(vec![
        Span::styled(" ck ", bold(t.on_primary).bg(t.primary)),
        Span::styled(format!("  {}", file.filename), bold(t.on_bar)),
        Span::styled(format!("    {}", meta.join("  ·  ")), dim()),
    ]);
    put(f, top.x, top.y, top.width, line);
    fill(f, bottom, t.bar);
    let mut hints = vec![Span::raw(" ")];
    if let Some(s) = &app.status {
        hints.extend(status_spans(s, t));
    } else {
        for (k, label) in [("h/l", "previous / next"), ("i", "open externally"), ("esc", "close")] {
            hints.extend([Span::styled(k, bold(t.primary)), Span::styled(format!(" {label}   "), dim())]);
        }
    }
    put(f, bottom.x, bottom.y, bottom.width, Line::from(hints));
    let area = middle.inner(Margin::new(MARGIN, 0));
    // Non-images (videos, pdfs, ...) show their thumbnail, if any, with a hint; so do images
    // of a saved copy that weren't downloaded.
    let source = app.viewer_source(file);
    let mut inner = area;
    let thumb_only = source.as_ref().is_some_and(|(_, k)| *k == Kind::Thumb);
    if !file.is_image() || thumb_only {
        let hint = match file.ext().to_uppercase() {
            _ if file.is_image() => format!("Not downloaded: showing the saved thumbnail ({} downloads it).", app.keys.key(Action::Download)),
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
    let Some((url, kind)) = source.filter(|_| app.image_search_panel.is_none()) else { return };
    let spinner = SPINNER[app.tick % SPINNER.len()];
    match app.images.get(&url, Size::new(inner.width, inner.height), kind) {
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
        State::Unavailable => msg(f, format!("Not saved: the image wasn't downloaded or cached ({} downloads it)", app.keys.key(Action::Download)), dim()),
    }
}

// ----- text -----

pub(crate) fn truncate(s: &str, width: usize) -> String {
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
    date.map(|d| format!("{d} · {}", ago(ts, clock))).unwrap_or_default()
}

pub fn ago(ts: i64, clock: Clock) -> String {
    if ts == 0 {
        return String::new();
    }
    match clock.now().saturating_sub(ts).max(0) {
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s if s < 86400 * 365 => format!("{}d ago", s / 86400),
        s => format!("{}y ago", s / (86400 * 365)),
    }
}

/// A file's dimensions and size, as far as they're known.
fn file_facts(file: &Attachment) -> Vec<String> {
    file.width.zip(file.height).map(|(w, h)| format!("{w}x{h}")).into_iter().chain(file.size.map(human_size)).collect()
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}
