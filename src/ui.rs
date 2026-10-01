use chrono::{Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, ThreadLayout, ThreadView, View};
use crate::markup;
use crate::model::Post;

const ACCENT: Color = Color::Yellow;
const DIM: Style = Style::new().fg(Color::DarkGray);
const SELECTED: Style = Style::new().bg(Color::Rgb(45, 45, 60)).add_modifier(Modifier::BOLD);
const SPINNER: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(f.area());

    draw_header(f, app, header);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(DIM)
        .title(view_title(app))
        .title_style(Style::new().fg(ACCENT).bold());
    let inner = block.inner(body);
    f.render_widget(block, body);

    match app.view {
        View::Sites => draw_sites(f, app, inner),
        View::Boards => draw_boards(f, app, inner),
        View::Catalog => draw_catalog(f, app, inner),
        View::Thread => draw_thread(f, app, inner),
    }
    draw_footer(f, app, footer);

    if app.show_help {
        draw_help(f);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let sep = Span::styled(" › ", DIM);
    let mut spans = vec![Span::styled(" ck ", Style::new().fg(Color::Black).bg(ACCENT).bold())];
    if app.view != View::Sites {
        spans.push(sep.clone());
        spans.push(Span::raw(app.current_site().cfg.name.clone()));
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
            (format!("{} ({} threads)", b.unwrap_or_default(), app.catalog.len()), &app.catalog_list.filter)
        }
        View::Thread => {
            let t = app.thread.as_ref();
            let subject = t.and_then(|t| t.posts.first()?.subject.clone());
            let n = t.map_or(0, |t| t.posts.len());
            (format!("{} ({n} post{})", subject.unwrap_or_else(|| "Thread".into()), if n == 1 { "" } else { "s" }), &String::new())
        }
    };
    let mut spans = vec![Span::raw(format!(" {title} "))];
    if !filter.is_empty() || app.filtering {
        spans.push(Span::styled(format!("[/{filter}] "), Style::new().fg(Color::Cyan)));
    }
    Line::from(spans)
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = if app.filtering {
        Line::from(vec![
            Span::styled(" filter: ", Style::new().fg(Color::Cyan)),
            Span::raw(current_filter(app).to_string()),
            Span::styled("█", Style::new().fg(Color::Cyan)),
            Span::styled("  enter accept · esc clear", DIM),
        ])
    } else if let Some(label) = &app.loading {
        Line::from(vec![
            Span::styled(format!(" {} ", SPINNER[app.tick % SPINNER.len()]), Style::new().fg(ACCENT)),
            Span::raw(format!("{label}…")),
        ])
    } else if let Some((msg, is_err)) = &app.status {
        let style = if *is_err { Style::new().fg(Color::Red) } else { Style::new().fg(Color::Green) };
        Line::styled(format!(" {msg}"), style)
    } else {
        let keys = match app.view {
            View::Thread => "j/k post · J/K line · enter follow quote · b replies · u back · i image · o browser · r reload · ? help",
            _ => "j/k move · enter open · esc back · / filter · o browser · r reload · ? help · q quit",
        };
        Line::styled(format!(" {keys}"), DIM)
    };
    f.render_widget(line, area);
}

fn current_filter(app: &App) -> &str {
    match app.view {
        View::Sites => &app.site_list.filter,
        View::Boards => &app.board_list.filter,
        View::Catalog => &app.catalog_list.filter,
        View::Thread => "",
    }
}

fn draw_sites(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .visible_sites()
        .into_iter()
        .map(|i| {
            let s = &app.sites[i];
            let url = s.cfg.url.clone().unwrap_or_else(|| "https://4chan.org".into());
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<16}", s.cfg.name), Style::new().bold()),
                Span::styled(format!("{:<10}", format!("{:?}", s.cfg.kind).to_lowercase()), Style::new().fg(Color::Blue)),
                Span::styled(url, DIM),
            ]))
        })
        .collect();
    render_list(f, area, items, &mut app.site_list.state, "No sites match");
}

fn draw_boards(f: &mut Frame, app: &mut App, area: Rect) {
    let width = app.boards().iter().map(|b| b.uri.width()).max().unwrap_or(1) + 3;
    let items: Vec<ListItem> = app
        .visible_boards()
        .into_iter()
        .map(|i| {
            let b = &app.boards()[i];
            let mut spans = vec![
                Span::styled(format!("{:<width$}", format!("/{}/", b.uri)), Style::new().fg(ACCENT).bold()),
                Span::raw(b.title.clone()),
            ];
            if b.nsfw == Some(true) {
                spans.push(Span::styled(" nsfw", Style::new().fg(Color::Red)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let empty = if app.loading.is_some() { "" } else { "No boards" };
    render_list(f, area, items, &mut app.board_list.state, empty);
}

fn draw_catalog(f: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.saturating_sub(3) as usize;
    let items: Vec<ListItem> = app
        .visible_catalog()
        .into_iter()
        .map(|i| {
            let p = &app.catalog[i];
            let mut head = Vec::new();
            if p.sticky {
                head.push(Span::styled("📌 ", Style::new().fg(Color::Green)));
            }
            if p.locked {
                head.push(Span::styled("🔒 ", Style::new().fg(Color::Red)));
            }
            head.push(Span::styled(format!("{}", p.no), DIM));
            head.push(Span::raw("  "));
            if let Some(s) = &p.subject {
                head.push(Span::styled(s.clone(), Style::new().fg(ACCENT).bold()));
                head.push(Span::raw("  "));
            }
            head.push(Span::styled(
                format!("R:{} I:{}", p.replies.unwrap_or(0), p.images.unwrap_or(0)),
                Style::new().fg(Color::Blue),
            ));
            head.push(Span::styled(format!("  {}", ago(p.time)), DIM));
            let preview = truncate(&p.plain_text(), width);
            ListItem::new(Text::from(vec![Line::from(head), Line::styled(preview, Style::new().fg(Color::Gray)), Line::raw("")]))
        })
        .collect();
    let empty = if app.loading.is_some() { "" } else { "No threads" };
    render_list(f, area, items, &mut app.catalog_list.state, empty);
}

fn render_list(f: &mut Frame, area: Rect, items: Vec<ListItem>, state: &mut ratatui::widgets::ListState, empty: &str) {
    if items.is_empty() {
        f.render_widget(Paragraph::new(Span::styled(format!(" {empty}"), DIM)), area);
        return;
    }
    let list = List::new(items).highlight_style(SELECTED).highlight_symbol("▌ ");
    f.render_stateful_widget(list, area, state);
}

fn draw_thread(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(t) = &mut app.thread else {
        let msg = if app.loading.is_some() { "" } else { "Thread not loaded" };
        f.render_widget(Paragraph::new(Span::styled(format!(" {msg}"), DIM)), area);
        return;
    };
    t.viewport = area.height as usize;
    if t.layout.as_ref().is_none_or(|l| l.width != area.width) {
        t.layout = Some(layout_thread(t, area.width));
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
                Span::styled("▌ ", Style::new().fg(ACCENT))
            } else {
                Span::raw("  ")
            };
            let mut spans = vec![gutter];
            spans.extend(line.spans.iter().cloned());
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

/// Lay out every post of a thread as wrapped lines, recording where each post starts.
fn layout_thread(t: &ThreadView, width: u16) -> ThreadLayout {
    let text_width = width.saturating_sub(2).max(10) as usize;
    let mut lines = Vec::new();
    let mut starts = Vec::with_capacity(t.posts.len() + 1);
    for (i, p) in t.posts.iter().enumerate() {
        starts.push(lines.len());
        lines.extend(post_lines(p, i == 0, &t.backlinks[i], t.no, text_width));
        lines.push(Line::raw(""));
    }
    starts.push(lines.len());
    ThreadLayout { width, lines, starts }
}

fn post_lines(p: &Post, is_op: bool, backlinks: &[u64], op_no: u64, width: usize) -> Vec<Line<'static>> {
    let mut head = vec![
        Span::styled(p.name.clone(), Style::new().fg(Color::Green).bold()),
        Span::raw("  "),
        Span::styled(fmt_time(p.time), DIM),
        Span::raw("  "),
        Span::styled(format!("No.{}", p.no), Style::new().fg(Color::Blue)),
    ];
    if is_op {
        head.push(Span::styled(" OP", Style::new().fg(ACCENT).bold()));
    }
    let mut out = markup::wrap(&Line::from(head), width);
    if let Some(s) = &p.subject {
        out.extend(markup::wrap(&Line::styled(s.clone(), Style::new().fg(ACCENT).bold()), width));
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
                Span::styled("File: ", DIM),
                Span::styled(file.filename.clone(), Style::new().fg(Color::Cyan)),
                Span::styled(meta, DIM),
            ]),
            width,
        ));
    }
    for line in &p.body {
        // Mark quotes of the OP like 4chan does.
        let line = if line.spans.iter().any(|s| s.content.contains(&format!(">>{op_no}"))) {
            let mut l = line.clone();
            for s in &mut l.spans {
                if s.content == format!(">>{op_no}").as_str() {
                    s.content.to_mut().push_str(" (OP)");
                }
            }
            l
        } else {
            line.clone()
        };
        out.extend(markup::wrap(&line, width));
    }
    if !backlinks.is_empty() {
        let mut spans = vec![Span::styled("Replies: ", DIM)];
        for no in backlinks {
            spans.push(Span::styled(format!(">>{no}"), markup::QUOTELINK.fg(Color::DarkGray)));
            spans.push(Span::raw(" "));
        }
        out.extend(markup::wrap(&Line::from(spans), width));
    }
    out
}

fn draw_help(f: &mut Frame) {
    let rows = [
        ("Everywhere", ""),
        ("j / k, ↓ / ↑", "move"),
        ("g / G", "top / bottom"),
        ("ctrl-d / ctrl-u", "half page down / up"),
        ("enter, l", "open"),
        ("esc, h, backspace", "back"),
        ("/", "filter list"),
        ("r", "reload"),
        ("o", "open in browser"),
        ("q, ctrl-c", "quit"),
        ("", ""),
        ("Thread", ""),
        ("j / k", "next / previous post"),
        ("J / K, space", "scroll by line / page"),
        ("enter, l", "jump to quoted post"),
        ("b", "jump to first reply"),
        ("u", "jump back"),
        ("i", "open post's file"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                Line::styled(*k, Style::new().fg(ACCENT).bold())
            } else {
                Line::from(vec![Span::styled(format!("  {k:<20}"), Style::new().fg(Color::Cyan)), Span::raw(*v)])
            }
        })
        .collect();
    let area = f.area();
    let w = 46.min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::bordered().border_type(BorderType::Rounded).title(" Help ").border_style(Style::new().fg(ACCENT)),
        ),
        popup,
    );
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

fn fmt_time(ts: i64) -> String {
    match Local.timestamp_opt(ts, 0).single() {
        Some(t) => format!("{} ({})", t.format("%Y-%m-%d %H:%M"), ago(ts)),
        None => String::new(),
    }
}

fn ago(ts: i64) -> String {
    if ts == 0 {
        return String::new();
    }
    let secs = (Local::now().timestamp() - ts).max(0);
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
