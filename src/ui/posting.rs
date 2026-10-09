//! The reply box (`P`): the post's fields, then 4chan's captcha, or the browser view when the
//! site wants a person.

use super::*;
use crate::app::{Art, Compose, Field, Stage, ViewAt, grid_cols};
use std::time::Instant;
use crate::captcha::{Cell, Prompt, Solving, Task};
use crate::editor::Editor;
use image::DynamicImage;

/// 4chan's longest comment.
const MAX_COMMENT: usize = 2000;
/// The label column's width.
const LABEL: u16 = 9;

pub(super) fn draw_reply(f: &mut Frame, app: &mut App) {
    let App { popup, images, tick, .. } = app;
    let Some(Popup::Reply(c)) = popup else { return };
    let where_to = match c.to.thread {
        0 => format!("New thread on /{}/", c.to.board),
        no => format!("Reply to /{}/ No.{no}", c.to.board),
    };
    let hint = match &c.stage {
        Stage::Writing => "ctrl-s post · tab next · esc keep for later",
        Stage::Asking | Stage::Sending => "esc stop",
        Stage::Person(_) => "click it · esc stop",
        Stage::Waiting { .. } => "enter ask again · esc back",
        Stage::Solving(s) if s.expired(Instant::now()) => "enter new captcha · esc back",
        Stage::Solving(s) => match s.challenge.task {
            Task::Slider(_) => "←/→ pick · enter next · ctrl-r new · esc back",
            Task::Grid { .. } => "arrows move · space pick · enter post · esc back",
            Task::Text { .. } | Task::None => "enter post · ctrl-r new · esc back",
        },
    };
    let area = f.area();
    let inner = panel(f, 100, area.height.saturating_sub(2).min(32), &where_to, hint);
    if inner.height < 3 {
        return;
    }
    let t = theme();
    let (body, foot) = (Rect { height: inner.height - 1, ..inner }, Rect { y: inner.bottom() - 1, height: 1, ..inner });
    if let Some(p) = &c.problem {
        put(f, foot.x, foot.y, foot.width, Line::styled(truncate(p, foot.width as usize), Style::new().fg(t.error)));
    }
    c.view_at = None;
    let spin = spinner(*tick);
    let say = |f: &mut Frame, text: String| put(f, body.x, body.y, body.width, Line::styled(text, Style::new().fg(t.primary)));
    match &c.stage {
        Stage::Writing => draw_fields(f, c, body),
        Stage::Asking => say(f, format!("{spin} Getting a captcha from 4chan…")),
        Stage::Sending => say(f, format!("{spin} Posting…")),
        Stage::Waiting { until, message } => {
            let left = until.saturating_duration_since(Instant::now()).as_secs();
            say(f, message.clone());
            let when = if left == 0 { "You can ask for a captcha again (enter).".into() } else { format!("You can ask for a captcha again in {left}s.") };
            put(f, body.x, body.y + 2, body.width, Line::styled(when, dim()));
        }
        Stage::Person(img) => {
            put(f, body.x, body.y, body.width, Line::styled("4chan wants to know you're a person: click what it asks below.", Style::new().fg(t.primary)));
            let view = Rect { y: body.y + 2, height: body.height.saturating_sub(2), ..body };
            match img {
                Some((img, left, top)) => {
                    if let Some(area) = art(f, images, &mut c.art, c.frames, img, view, true) {
                        c.view_at = Some(ViewAt { area, left: *left, top: *top, width: img.width(), height: img.height() });
                    }
                }
                None => put(f, view.x, view.y, view.width, Line::styled(format!("{spin} Loading…"), dim())),
            }
        }
        Stage::Solving(_) => draw_captcha(f, images, c, body),
    }
}

/// The post's fields: a row each, and the comment's box.
fn draw_fields(f: &mut Frame, c: &Compose, area: Rect) {
    let t = theme();
    let fields: Vec<Field> = c.fields().collect();
    // One row each, but the comment's, which takes what's left.
    let fixed = cells(fields.len().saturating_sub(1));
    let comment_rows = area.height.saturating_sub(fixed + 1).max(1);
    let mut y = area.y;
    let value_x = area.x + LABEL;
    let value_w = area.width.saturating_sub(LABEL);
    for field in fields {
        let focused = c.field == field;
        let label_style = if focused { bold(t.primary) } else { dim() };
        let label = match field {
            Field::Name => "Name",
            Field::Options => "Options",
            Field::Subject => "Subject",
            Field::Comment => "Comment",
            Field::File => "File",
            Field::Spoiler => "Spoiler",
        };
        put(f, area.x, y, LABEL, Line::styled(label, label_style));
        match field {
            Field::Comment => {
                let n = c.comment.text().chars().count();
                let count = format!("{n}/{MAX_COMMENT}");
                let style = if n > MAX_COMMENT { Style::new().fg(t.error) } else { dim() };
                let rows = Rect::new(value_x, y, value_w.saturating_sub(cells(count.len()) + 1), comment_rows);
                fill(f, Rect::new(value_x, y, value_w, comment_rows), t.surface);
                draw_editor(f, &c.comment, rows, focused, Some(">>123 quotes, >text is green"));
                put(f, area.right().saturating_sub(cells(count.len())), y + comment_rows - 1, cells(count.len()), Line::styled(count, style));
                y += comment_rows;
            }
            Field::Spoiler => {
                let mark = if c.spoiler { "[x]" } else { "[ ]" };
                let style = if focused { Style::new().fg(t.text).bg(t.selection) } else { Style::new().fg(t.text) };
                put(f, value_x, y, value_w, Line::from(vec![Span::styled(mark, style), Span::styled(" spoiler the file (space)", dim())]));
                y += 1;
            }
            _ => {
                let placeholder = match field {
                    Field::Name => "Anonymous",
                    Field::Options => "sage, …",
                    Field::File => "a path: ~/pictures/cat.png",
                    _ => "",
                };
                let editor = match field {
                    Field::Name => &c.name,
                    Field::Options => &c.options,
                    Field::Subject => &c.subject,
                    _ => &c.file,
                };
                draw_editor(f, editor, Rect::new(value_x, y, value_w, 1), focused, Some(placeholder));
                y += 1;
            }
        }
    }
}

/// A field's text in `area` (wrapped, kept scrolled to the cursor), with the cursor when
/// it's focused, or what goes there when it's empty.
fn draw_editor(f: &mut Frame, e: &Editor, area: Rect, focused: bool, placeholder: Option<&str>) {
    let t = theme();
    if area.is_empty() {
        return;
    }
    if e.is_empty() && !focused {
        put(f, area.x, area.y, area.width, Line::styled(placeholder.unwrap_or_default().to_string(), dim()));
        return;
    }
    let width = area.width.saturating_sub(1).max(1) as usize;
    let (rows, (row, col)) = e.wrapped(width);
    let height = area.height as usize;
    let first = (row + 1).saturating_sub(height);
    for (i, text) in rows.iter().skip(first).take(height).enumerate() {
        let style = if text.starts_with('>') && !text.starts_with(">>") { Style::new().fg(t.greentext) } else { Style::new().fg(t.text) };
        put(f, area.x, area.y + cells(i), area.width, Line::styled(text.clone(), style));
    }
    if focused {
        let (x, y) = (area.x + cells(col), area.y + cells(row - first));
        if let Some(cell) = f.buffer_mut().cell_mut((x, y)) {
            cell.set_style(Style::new().fg(t.on_primary).bg(t.primary));
        }
        if e.is_empty() && let Some(p) = placeholder {
            put(f, x + 1, y, area.width.saturating_sub(1), Line::styled(p.to_string(), dim()));
        }
    }
}

fn draw_captcha(f: &mut Frame, images: &mut Images, c: &mut Compose, area: Rect) {
    let t = theme();
    let Stage::Solving(s) = &c.stage else { return };
    let left = s.challenge.expires.saturating_duration_since(Instant::now()).as_secs();
    let expiry = if left == 0 { "Expired: enter for a new one".to_string() } else { format!("expires in {left}s") };
    let s: &Solving = s;
    match &s.challenge.task {
        Task::None => put(f, area.x, area.y, area.width, Line::styled("No captcha needed: posting…", Style::new().fg(t.primary))),
        Task::Slider(steps) => {
            let Some(step) = s.current() else { return };
            let title = format!("Step {}/{} · {}/{} · {expiry}", s.step + 1, steps.len(), s.slide, step.items.len());
            put(f, area.x, area.y, area.width, Line::styled(title, dim()));
            let pic = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
            let shown = match (s.slide.checked_sub(1).and_then(|i| step.items.get(i)), &step.prompt) {
                (Some(img), _) | (None, Prompt::Image(img)) => Some(img),
                (None, Prompt::Text(text)) => {
                    put(f, pic.x, pic.y, pic.width, Line::styled(format!("{text}  (→ to start)"), Style::new().fg(t.text)));
                    None
                }
            };
            if let Some(img) = shown {
                let key = (s.step as u64) << 32 | s.slide as u64;
                art(f, images, &mut c.art, key, img, pic, false);
            }
        }
        Task::Text { prompt, image } => {
            put(f, area.x, area.y, area.width, Line::from(vec![Span::styled(prompt.clone(), Style::new().fg(t.text)), Span::styled(format!("  {expiry}"), dim())]));
            let answer = Line::from(vec![Span::styled("Answer   ", bold(t.primary)), Span::styled(s.typed.clone(), Style::new().fg(t.text)), Span::styled("▏", Style::new().fg(t.primary))]);
            put(f, area.x, area.y + 2, area.width, answer);
            if let Some(url) = image {
                let pic = Rect { y: area.y + 4, height: area.height.saturating_sub(4), ..area };
                url_image(f, images, url, pic);
            }
        }
        Task::Grid { prompt, cells: grid, single } => {
            let how = if *single { "pick one" } else { "pick all that fit" };
            put(f, area.x, area.y, area.width, Line::from(vec![Span::styled(prompt.clone(), Style::new().fg(t.text)), Span::styled(format!("  {how} · {expiry}"), dim())]));
            let cols = grid_cols(grid.len()) as u16;
            let rows = grid.len().div_ceil(cols.max(1) as usize) as u16;
            let space = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
            let (cw, ch) = (space.width / cols.max(1), space.height / rows.max(1));
            for (i, cell) in grid.iter().enumerate() {
                let (col, row) = (i as u16 % cols.max(1), i as u16 / cols.max(1));
                let r = Rect::new(space.x + col * cw, space.y + row * ch, cw.saturating_sub(1), ch.saturating_sub(1));
                if r.height < 2 {
                    continue;
                }
                let chosen = s.chosen.get(i).copied().unwrap_or(false);
                let bg = if chosen { t.primary_container } else { t.surface };
                fill(f, r, bg);
                let mark = format!("{} {}{}", i + 1, if chosen { "✓" } else { " " }, if s.at == i { " ◀" } else { "" });
                let style = if s.at == i { bold(t.primary) } else { Style::new().fg(t.text) };
                put(f, r.x, r.y, r.width, Line::styled(mark, style));
                let inside = Rect { y: r.y + 1, height: r.height - 1, ..r };
                match cell {
                    Cell::Label(l) => put(f, inside.x, inside.y, inside.width, Line::styled(l.clone(), Style::new().fg(t.text))),
                    Cell::Image(url) => url_image(f, images, url, inside),
                }
            }
        }
    }
}

/// A picture of 4chan's (a captcha's) by its URL, fetched like any other.
fn url_image(f: &mut Frame, images: &mut Images, url: &str, area: Rect) {
    match images.get(url, Size::new(area.width, area.height), Kind::Full) {
        State::Ready(p) => {
            let s = p.size();
            f.render_widget(Image::new(p), Rect::new(area.x, area.y, s.width, s.height).intersection(area));
        }
        State::Failed | State::Unavailable => put(f, area.x, area.y, area.width, Line::styled("(couldn't load the picture)", Style::new().fg(theme().error))),
        State::Loading | State::Rendering => put(f, area.x, area.y, area.width, Line::styled("Loading…", dim())),
    }
}

/// Draw `img` in `area`, encoded once for `key` and the area; where it was drawn. The
/// browser view (`grow`) is made as big as fits, centered; a captcha's pictures are drawn as
/// they are, if they fit.
fn art(f: &mut Frame, images: &Images, cache: &mut Option<Art>, key: u64, img: &DynamicImage, area: Rect, grow: bool) -> Option<Rect> {
    if area.is_empty() {
        return None;
    }
    if cache.as_ref().is_none_or(|a| a.key != key || a.area != area) {
        *cache = images.encode_now(img, Size::new(area.width, area.height), grow).map(|proto| Art { key, area, proto });
    }
    let Some(a) = cache.as_ref() else {
        put(f, area.x, area.y, area.width, Line::styled("Images are off: turn them on in Settings to see this", Style::new().fg(theme().error)));
        return None;
    };
    let s = a.proto.size();
    let x = if grow { area.x + area.width.saturating_sub(s.width) / 2 } else { area.x };
    let r = Rect::new(x, area.y, s.width, s.height).intersection(area);
    f.render_widget(Image::new(&a.proto), r);
    Some(r)
}
