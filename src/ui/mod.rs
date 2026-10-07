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
    App, Clock, Hit, LineCache, LinkItem, Part, Popup, Reveal, TabPopup, Spot, SettingsPopup, SiteRow, Sort, Status, ThreadLayout, ThreadCopy, ThreadView, Typing, View, key_rows,
    setting_rows, settings,
};
use std::collections::HashMap;

use crate::http;
use crate::images::{Images, Kind, State};
use crate::keys::{self, Action, KeyMap};
use crate::config::CatalogLayout;
use crate::filter::Mark;
use crate::markup;
use crate::model::{Attachment, Post};
use crate::theme::{self, ROLES, Theme, theme};

mod lists;
mod popups;
mod settings;
mod thread;
mod viewer;

use lists::*;
use popups::*;
use settings::*;
use thread::*;
use viewer::*;

// Used by the tests, from their old place.
#[cfg(test)]
pub use thread::layout_all;
#[cfg(test)]
pub(crate) use popups::wrap_path;

const SPINNER: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
/// The spinner's frame at this tick.
fn spinner(tick: usize) -> &'static str {
    SPINNER.get(tick % SPINNER.len()).copied().unwrap_or_default()
}
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

/// A count of rows or columns as a size on screen, which can't be bigger anyway.
fn cells(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// "1 thread", "2 threads".
fn plural(n: usize, word: &str) -> String {
    count(n, word, &format!("{word}s"))
}

/// "1 reply", "2 replies": for words that don't just take an s.
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Words of `text` in lines at most `width` wide.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let Some(line) = lines.last_mut() else { break };
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}

/// A dim message in the middle of an area, wrapped to fit.
fn empty(f: &mut Frame, area: Rect, msg: &str) {
    if msg.is_empty() {
        return;
    }
    let width = usize::from(area.width.saturating_sub(4)).max(1);
    for (i, line) in wrap_words(msg, width).into_iter().enumerate() {
        let y = area.y.saturating_add(area.height / 3).saturating_add(cells(i));
        if y < area.bottom() {
            put(f, area.x, y, area.width, Line::styled(line, dim()).centered());
        }
    }
}

// ----- the frame -----

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = theme();
    let all = f.area();
    // A saved copy is read offline: its images only come from disk.
    app.images.offline = app.tab.view == View::Thread && app.tab.saved().is_some();
    f.buffer_mut().set_style(all, Style::new().fg(t.text).bg(t.background));
    // Only chips drawn this frame can be clicked: none under the viewer.
    app.tab_chips.clear();
    if app.tab.viewer().is_some() {
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
        match app.tab.popup {
            Some(TabPopup::Preview(_)) => draw_preview(f, app),
            Some(TabPopup::Links(_)) => draw_links(f, app),
            _ => {}
        }
        match app.popup {
            Some(Popup::Settings(_)) => draw_settings_popup(f, app),
            Some(Popup::AddFilter(_)) => draw_add_filter(f, app),
            Some(Popup::Help(_)) => draw_help(f, app),
            _ => {}
        }
    }
    match app.popup {
        Some(Popup::Hints(_)) => draw_hints(f, app),
        Some(Popup::Menu(_)) => draw_menu(f, app),
        Some(Popup::ImageSearch(_)) => draw_image_search(f, app),
        Some(Popup::Adding(_)) => draw_adding(f, app),
        Some(Popup::Confirm(_)) => draw_confirm(f, app),
        _ => {}
    }
    app.images.end_frame();
    // Text in the terminal's own color on a filled color, in one that reads on it; and every
    // 24-bit color to the nearest of 256, for terminals without 24-bit color.
    for cell in f.buffer_mut().content.iter_mut() {
        cell.fg = theme::ink(cell.fg, cell.bg);
        if !app.truecolor {
            cell.fg = theme::to_256(cell.fg);
            cell.bg = theme::to_256(cell.bg);
        }
    }
}

/// With more than one tab, their chips in the row under the bar (the current one stands
/// out); clicking one switches to it.
fn draw_tab_row(f: &mut Frame, app: &mut App, area: Rect) {
    let n = app.tabs.len();
    if n < 2 {
        return;
    }
    let t = theme();
    let each = (area.width as usize / n).clamp(8, 28);
    let mut x = area.x;
    for i in 0..n {
        let label = app.tab_label(i);
        // A watched thread's new posts, kept whatever the label is cut to.
        let unread = app.tab_unread(i).map_or(String::new(), |n| format!(" ({n})"));
        let text = format!(" {} {}{unread} ", i + 1, truncate(&label, each.saturating_sub(5 + unread.width())));
        let w = (text.width() as u16).min(area.right().saturating_sub(x));
        if w == 0 {
            break;
        }
        let style = if i == app.active {
            bold(t.on_primary_container).bg(t.primary_container).add_modifier(Modifier::UNDERLINED)
        } else {
            Style::new().fg(t.text_dim).bg(t.surface_high)
        };
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
    let (crumbs, mut meta) = location(app);
    let mut spans = vec![Span::styled(" ck ", bold(t.on_primary).bg(t.primary)), Span::raw(" ")];
    let n = crumbs.len();
    let before: usize = 5 + crumbs.iter().take(n.saturating_sub(1)).map(|c| c.width() + 5).sum::<usize>();
    // The facts on the right give way (the counts first, then the chips) before the last
    // crumb is cut to less than a few letters: where you are matters more.
    let wanted = before + crumbs.last().map_or(0, |c| c.width().min(12));
    let meta_width = |meta: &[Span]| meta.iter().map(|s| s.width()).sum::<usize>();
    while !meta.is_empty() && usize::from(area.width).saturating_sub(meta_width(&meta) + 2) < wanted {
        // The counts are the last span but the trailing space.
        let counts = meta.len().saturating_sub(2);
        if meta.get(counts).is_some_and(|s| !s.content.trim().is_empty()) {
            meta.remove(counts);
        } else {
            meta.remove(0);
        }
    }
    let meta_w = cells(meta_width(&meta));
    let left_w = area.width.saturating_sub(meta_w + 2);
    // The last crumb (the most specific) gives way, with an ellipsis, when space is short.
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
            if let Some(board) = &app.tab.catalog_board
                && !app.images_on(app.tab.catalog_site, board)
            {
                meta.push("images off".into());
            }
            vec![site(), board().unwrap_or_default()]
        }
        View::Thread => {
            let mut return_crumbs = None;
            let th = app.tab.thread.as_ref();
            let subject = th.and_then(|th| th.posts.first()?.subject.clone()).unwrap_or_else(|| {
                th.map_or_else(|| "Thread".into(), |th| format!("Thread {}", th.no))
            });
            if let Some(th) = th {
                let deleted = th.deleted.len();
                meta.push(plural(th.posts.len().saturating_sub(deleted), "post"));
                if deleted > 0 {
                    meta.push(format!("{deleted} deleted"));
                }
                let new = (0..th.posts.len()).filter(|&i| th.is_new(i)).count();
                match th.new_below().0 {
                    0 if new > 0 => meta.push(format!("{new} new")),
                    0 => {}
                    below => meta.push(format!("{new} new, {below} below ↓")),
                }
                if th.reveal_all {
                    meta.push("spoilers shown".into());
                }
                // Its page in the board's index (the last one is shown with the chips).
                if let Some((p, of)) = app.thread_page(&app.key(&th.board, th.no)).filter(|(p, of)| p < of) {
                    meta.push(format!("p{p}/{of}"));
                }
                // Which screenful of a post taller than the screen.
                if let (Some(((at, n), ..)), Some(p)) = (th.tall(), th.current()) {
                    meta.insert(0, format!("No.{} ({at}/{n})", p.no));
                }
            }
            if th.is_some_and(|th| !app.images_on(app.tab.site, &th.board)) {
                meta.push("images off".into());
            }
            let uri = app.tab.board.as_ref().map(|b| format!("/{}/", b.uri)).unwrap_or_default();
            if let Some(c) = th.and_then(|th| th.conversation.as_ref()).filter(|_| app.tab.gallery.is_none()) {
                // The conversation's posts instead of the thread's.
                meta.retain(|m| !m.ends_with("posts") && !m.ends_with("post"));
                meta.insert(0, if c.capped { format!("first {}", plural(c.depth.len(), "post")) } else { plural(c.depth.len(), "post") });
                let what = match &c.poster {
                    Some(id) => format!("Posts by ID:{}", truncate(id, 16)),
                    None => format!("Conversation of No.{}", c.anchor),
                };
                return_crumbs = Some(vec![site(), uri.clone(), truncate(&subject, 24), what]);
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
            let unread = app.store.watched.iter().fold(0usize, |n, w| n.saturating_add(w.unread));
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
            meta.push(human_size(app.store.saved.iter().fold(0u64, |n, m| n.saturating_add(m.bytes))));
            vec!["Saved".into()]
        }
        View::Settings => vec!["Settings".into()],
        View::Search => {
            let Some(s) = &app.tab.search else { return (vec!["Search".into()], Vec::new()) };
            match s.total {
                Some(t) => meta.push(format!("{} of {}", s.hits.len(), plural(t as usize, "result"))),
                None => meta.push(plural(s.hits.len(), "result")),
            }
            let hidden = s.hidden.iter().filter(|&&h| h).count();
            if hidden > 0 {
                meta.push(format!("{hidden} hidden"));
            }
            if s.saved.is_some() {
                vec!["Saved".into(), format!("Search: {}", truncate(&s.query, 40))]
            } else {
                vec![site(), format!("/{}/", s.board), format!("Search: {}", truncate(&s.query, 40))]
            }
        }
    };
    let mut spans: Vec<Span> = Vec::new();
    // An active filter or search, unless it's being typed (the footer shows that).
    let query = match app.tab.view {
        View::Thread => app.tab.thread.as_ref().filter(|th| !th.search.is_empty() && app.typing != Some(Typing::ThreadSearch)).map(|th| {
            let k = th.matches.len();
            format!("/{}  {}", th.search, count(k, "match", "matches"))
        }),
        _ => Some(current_filter(app)).filter(|q| !q.is_empty() && app.typing != Some(Typing::ListFilter)).map(|q| format!("/{q}")),
    };
    if let Some(q) = query {
        spans.extend([chip(q, t.on_primary_container, t.primary_container), Span::raw("  ")]);
    }
    // A thread on its board's last page is next to fall off.
    let last_page = app.tab.thread.as_ref().filter(|_| app.tab.view == View::Thread).and_then(|th| app.thread_page(&app.key(&th.board, th.no)));
    if let Some((p, of)) = last_page.filter(|(p, of)| p >= of) {
        spans.extend([chip(format!("last page {p}/{of}"), t.background, t.warning), Span::raw("  ")]);
    }
    // Only the posts with files, or images hidden (`M`).
    if let Some(label) = app.tab.thread.as_ref().filter(|_| app.tab.view == View::Thread).and_then(|th| th.media.label()) {
        spans.extend([chip(label, t.on_primary_container, t.primary_container), Span::raw("  ")]);
    }
    // A saved copy, read offline; or the last copy kept, shown while it loads.
    let copy = match app.tab.view {
        View::Thread => app.tab.copy.map(|c| match c {
            ThreadCopy::Saved(o) => ("saved", o),
            ThreadCopy::Cached(o) => ("cached", o),
        }),
        View::Catalog => app.tab.catalog_cached.map(|c| ("cached", c)),
        _ => None,
    };
    if let Some((what, off)) = copy {
        spans.extend([chip(format!("{what} {}", ago(off.saved, app.clock)), t.on_primary_container, t.primary_container), Span::raw(" ")]);
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
    let typing = app.typing.as_ref().map(|typing| match typing {
        Typing::Goto(g) => ("go to", g.as_str()),
        Typing::ArchiveQuery(q) => ("search the archive", q.as_str()),
        Typing::ThreadSearch => ("search", app.tab.thread.as_ref().map_or("", |th| th.search.as_str())),
        Typing::ListFilter => ("filter", current_filter(app)),
    });
    let line = if let Some((what, q)) = typing {
        Line::from(vec![
            Span::raw(" "),
            chip(what, t.on_primary, t.primary),
            Span::styled(format!(" {q}"), Style::new().fg(t.on_bar)),
            Span::styled("▏", Style::new().fg(t.primary)),
            Span::styled(
                match (app.goto_text(), &app.status) {
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
            Span::styled(format!(" {} ", spinner(app.tick)), bold(t.primary)),
            Span::styled(format!("{label}…"), Style::new().fg(t.on_bar)),
        ])
    } else if let Some(s) = &app.status {
        Line::from([vec![Span::raw(" ")], status_spans(s, t)].concat())
    } else {
        Line::default()
    };
    let d = &app.downloads;
    let mut right = Vec::new();
    if d.running > 0 {
        right.extend([chip(format!("⇣ {}/{}", d.done + d.skipped + d.failed, d.total), t.text, t.surface_high), Span::raw(" ")]);
    }
    if !app.refreshing.is_empty() {
        right.extend([chip(format!("↻ {}", app.refreshing.len()), t.text, t.surface_high), Span::raw(" ")]);
    }
    let right_w = cells(right.iter().map(|s| s.width()).sum::<usize>());
    let line = if line.spans.is_empty() { fit_hints(footer_hints(app), usize::from(area.width.saturating_sub(right_w))) } else { line };
    put(f, area.x, area.y, area.width.saturating_sub(right_w), line);
    put(f, area.right().saturating_sub(right_w), area.y, right_w, Line::from(right));
}

/// Key hints in `width` columns: the last one (help) always, and before it as many of the
/// others, in order, as fit whole.
fn fit_hints(mut hints: Vec<(String, &'static str)>, width: usize) -> Line<'static> {
    let t = theme();
    let wide = |(key, label): &(String, &str)| key.width() + 1 + label.width() + 3;
    let last = hints.pop();
    let mut room = width.saturating_sub(1).saturating_sub(last.as_ref().map_or(0, wide));
    let fitting = hints.into_iter().take_while(|h| {
        let fits = wide(h) <= room;
        room = room.saturating_sub(wide(h));
        fits
    });
    let mut spans = vec![Span::raw(" ")];
    for (key, label) in fitting.collect::<Vec<_>>().into_iter().chain(last) {
        spans.extend([Span::styled(key, bold(t.primary)), Span::styled(format!(" {label}   "), dim())]);
    }
    Line::from(spans)
}

/// Key hints for the footer, with the configured keys, most useful first; help is last.
fn footer_hints(app: &App) -> Vec<(String, &'static str)> {
    let k = |a| app.keys.key(a);
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
            Some(Part::Poster) => vec![
                ("enter".into(), "this poster's posts"),
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
                let c = app.tab.thread.as_ref().and_then(|t| t.conversation.as_ref());
                let capped = c.is_some_and(|c| c.capped);
                let back = if c.is_some_and(|c| c.poster.is_some()) { Action::Poster } else { Action::Conversation };
                let mut hints = vec![
                    ("j/k".into(), "post"),
                    (k(Action::NextPart), "images & links"),
                    (k(Action::Menu), "more"),
                    ("enter".into(), "quote"),
                    (format!("esc/{}", k(back)), "whole thread"),
                ];
                if capped {
                    hints.push(("".into(), "the nearest 500 posts"));
                }
                hints
            }
            // What the selected post has comes first: its images, the quote enter follows,
            // its conversation; then what's always there.
            None => {
                let Some(t) = app.tab.thread.as_ref() else { return vec![(k(Action::Help), "help")] };
                let post = t.current();
                let files = post.is_some_and(|p| !p.files.is_empty());
                let quotes = post.is_some_and(|p| !p.links.is_empty() || !p.quotes.is_empty());
                let parts = post.is_some_and(|p| !p.files.is_empty() || !p.links.is_empty() || !p.urls.is_empty());
                let talks = post.is_some_and(|p| t.backlinks.get(t.selected).is_some_and(|b| !b.is_empty()) || p.quotes.iter().any(|q| t.index.contains_key(q)));
                let mut hints = vec![("j/k".into(), "post")];
                if files {
                    hints.push((k(Action::View), "view image"));
                }
                if quotes {
                    hints.push(("enter".into(), "quote"));
                }
                // The menu lists everything else, with its keys.
                hints.push((k(Action::Menu), "more"));
                if parts {
                    hints.push((k(Action::NextPart), "images & links"));
                }
                if talks {
                    hints.push((k(Action::Conversation), "conversation"));
                }
                if quotes {
                    hints.push((k(Action::Preview), "preview quote"));
                }
                if app.can_jump_back() {
                    hints.push((k(Action::JumpBack), "back"));
                }
                hints.push((k(Action::Hints), "hints"));
                hints.push((k(Action::Search), "search"));
                hints.push((k(Action::Watch), if app.menu_watching(t.no) { "unwatch" } else { "watch" }));
                if t.posts.iter().any(|p| !p.files.is_empty()) {
                    hints.push((k(Action::Gallery), "gallery"));
                }
                hints
            }
        },
        View::Catalog => {
            let op = app.visible_catalog().get(app.tab.catalog_list.state.selected().unwrap_or(0)).and_then(|&i| app.tab.catalog.get(i));
            let mut hints = vec![("enter".into(), "open")];
            if op.is_some_and(|p| !p.files.is_empty()) {
                hints.push((k(Action::View), "view image"));
            }
            hints.push((k(Action::Watch), if op.is_some_and(|p| app.menu_watching(p.no)) { "unwatch" } else { "watch" }));
            hints.extend([
                (k(Action::Search), "filter"),
                (k(Action::Menu), "more"),
                (k(Action::Hints), "hints"),
                (k(Action::Sort), "sort"),
                (k(Action::Compact), "layout"),
                (k(Action::Reload), "reload"),
            ]);
            hints
        }
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
        View::Search if app.tab.search.as_ref().is_some_and(|s| s.saved.is_some()) => {
            vec![("enter".into(), "read the saved copy"), (k(Action::Reload), "search again"), ("esc".into(), "back")]
        }
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
    if app.tab.view == View::Thread && app.tab.gallery.is_none() && app.focused().is_none() && app.tab.saved().is_some_and(|o| !o.dead) {
        hints.insert(0, (k(Action::Reload), "live thread"));
    }
    if app.tab.view != View::Settings {
        hints.push((k(Action::Settings), "settings"));
    }
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
    // A bar at the start, drawn as a glyph too: a selection shown by color alone is lost in
    // a monochrome terminal (and in a copy of the screen).
    if (selected || marked)
        && row.width > 0
        && let Some(cell) = f.buffer_mut().cell_mut((row.x, row.y))
    {
        cell.set_symbol(if selected { "▌" } else { "▏" }).set_fg(t.primary);
    }
}

/// The rows of a list in `area`, from `first`, a line each; the selected one is painted
/// (out into the panel's padding).
pub(super) fn list_rows(f: &mut Frame, area: Rect, first: usize, n: usize, sel: Option<usize>, mut row: impl FnMut(usize) -> Line<'static>) {
    for k in (first..n).take(area.height as usize) {
        let y = area.y + (k - first) as u16;
        paint_row(f, Rect::new(area.x - 2, y, area.width + 4, 1), None, Some(k) == sel, false);
        put(f, area.x, y, area.width, row(k));
    }
}

/// A list in a panel, as Settings' lists are: `n` rows (made by `row`, given the width)
/// scrolled to the selected one (painted unless not `painted`), `empty` when there are none,
/// and `note` on the last line.
/// The panel is `extra` rows taller than the list needs; `input` of them, above the note,
/// are left for a field. Returns the panel's inner area.
#[allow(clippy::too_many_arguments)]
pub(super) fn list_panel(
    f: &mut Frame,
    (width, title, hint): (u16, &str, &str),
    n: usize,
    (sel, painted): (usize, bool),
    (empty, note): (&str, &str),
    (extra, input): (u16, u16),
    mut row: impl FnMut(usize, u16) -> Line<'static>,
) -> Rect {
    let h = (n.max(1) as u16 + 5 + extra).min(f.area().height.saturating_sub(4));
    let inner = panel(f, width, h, title, hint);
    let view = inner.height.saturating_sub(2 + input);
    let first = (sel + 1).saturating_sub(view as usize);
    if n == 0 {
        put(f, inner.x, inner.y, inner.width, Line::styled(empty.to_string(), dim()));
    }
    list_rows(f, Rect { height: view, ..inner }, first, n, painted.then_some(sel), |k| row(k, inner.width));
    put(f, inner.x, inner.bottom().saturating_sub(1), inner.width, Line::styled(note.to_string(), dim()));
    inner
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
