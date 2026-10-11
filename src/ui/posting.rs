//! The reply box (`P`): the post's fields, then the site's captcha, or the browser view when
//! the site wants a person.

use super::*;
use crate::app::{Art, Compose, Field, Stage, ViewAt, grid_cols};
use std::time::Instant;
use crate::captcha::{Cell, Solving, Task};
use crate::editor::Editor;
use image::DynamicImage;
use std::borrow::Cow;

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
        Stage::Offer => "y download · esc back",
        Stage::Installing { .. } => "esc keep for later",
        Stage::Working => "esc stop",
        Stage::Person(_) => "arrows move · enter click · esc stop",
        Stage::Waiting { .. } => "enter ask again · esc back",
        Stage::Solving(s) if s.expired(Instant::now()) => "enter new captcha · esc back",
        Stage::Solving(s) => match s.challenge.task {
            Task::Slider(_) => "←/→ slide · enter it's this one · ctrl-r new · esc back",
            Task::Grid { .. } => "arrows move · space pick · enter post · esc back",
            Task::Text { .. } | Task::None => "enter post · ctrl-r new · esc back",
        },
    };
    let area = f.area();
    // A captcha or the browser view gets the whole screen, to be read.
    let (w, h) = match c.stage {
        Stage::Solving(_) | Stage::Person(_) => (area.width, area.height),
        _ => (100, area.height.saturating_sub(2).min(32)),
    };
    let inner = panel(f, w, h, &where_to, hint);
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
        Stage::Offer => {
            say(f, "Posting needs ck-web, the browser ck posts through (for the site's captcha and checks).".into());
            let mb = crate::web::install::SIZE_MB;
            let lines = [
                format!("It's Chromium with no window: ck downloads it once (about {mb} MB), from ck's"),
                "releases on GitHub, checks it, and keeps it in ck's data folder.".into(),
                String::new(),
                "Download it now? y / n".into(),
            ];
            for (i, l) in lines.into_iter().enumerate() {
                put(f, body.x, body.y + 2 + cells(i), body.width, Line::styled(l, Style::new().fg(t.text)));
            }
        }
        Stage::Installing { got, size } => {
            let mb = |b: u64| b / (1 << 20);
            let of = size.map_or(String::new(), |s| format!(" of {} MB", mb(s)));
            say(f, format!("{spin} Downloading ck-web: {} MB{of}…", mb(*got)));
            if let Some(s) = size.filter(|&s| s > 0) {
                let w = u64::from(body.width.saturating_sub(2));
                let done = cells(usize::try_from(got.saturating_mul(w) / s).unwrap_or(0));
                fill(f, Rect::new(body.x, body.y + 2, body.width.saturating_sub(2), 1), t.surface_high);
                fill(f, Rect::new(body.x, body.y + 2, done.min(body.width.saturating_sub(2)), 1), t.primary);
            }
        }
        Stage::Working => say(f, format!("{spin} Posting to {}…", c.to.site)),
        Stage::Waiting { until, message } => {
            let left = until.saturating_duration_since(Instant::now()).as_secs();
            say(f, message.clone());
            let when = if left == 0 { "You can ask for a captcha again (enter).".into() } else { format!("You can ask for a captcha again in {left}s.") };
            put(f, body.x, body.y + 2, body.width, Line::styled(when, dim()));
        }
        Stage::Person(img) => {
            let how = format!("{} wants to know you're a person: move the red pointer onto what it asks (arrows, shift for bigger steps) and press enter, or click it.", c.to.site);
            put(f, body.x, body.y, body.width, Line::styled(truncate(&how, body.width as usize), Style::new().fg(t.primary)));
            let view = Rect { y: body.y + 2, height: body.height.saturating_sub(2), ..body };
            match img {
                Some((img, left, top)) => {
                    let cell = c.view_at.map_or((8, 16), |v| (v.width / u32::from(v.area.width.max(1)), v.height / u32::from(v.area.height.max(1))));
                    let at = c.pointer.map(|(x, y)| (x.saturating_sub(*left), y.saturating_sub(*top)));
                    let key = c.frames << 32 | at.map_or(0, |(x, y)| u64::from(x) << 16 | u64::from(y));
                    if let Some(area) = art(f, images, &mut c.art, key, || Cow::Owned(with_pointer(img, at, cell)), view) {
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
        // A field's name is its label.
        put(f, area.x, y, LABEL, Line::styled(format!("{field:?}"), label_style));
        match field {
            Field::Comment => {
                let n = c.comment.text().chars().count();
                let count = c.limit.map_or_else(|| n.to_string(), |max| format!("{n}/{max}"));
                let style = if c.limit.is_some_and(|max| n > max) { Style::new().fg(t.error) } else { dim() };
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
                let (editor, placeholder) = match field {
                    Field::Name => (&c.name, "Anonymous"),
                    Field::Options => (&c.options, "sage, …"),
                    Field::Subject => (&c.subject, ""),
                    _ => (&c.file, "a path: ~/pictures/cat.png"),
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
            let title = format!("Step {}/{} · {expiry}", s.step + 1, steps.len());
            put(f, area.x, area.y, area.width, Line::styled(title, dim()));
            put(f, area.x, area.y + 1, area.width, Line::styled(truncate(&step.text, area.width as usize), Style::new().fg(t.text)));
            // The shape to find on the left, beside the strip the slider's on, to compare.
            let height = area.height.saturating_sub(5);
            let side = (area.width / 4).min(24);
            let (find, strip) = (Rect::new(area.x, area.y + 4, side, height), Rect::new(area.x + side + 2, area.y + 4, area.width.saturating_sub(side + 2), height));
            put(f, find.x, find.y - 1, find.width, Line::styled("Find this", bold(t.primary)));
            let at = format!("← {} of {} →  in this one? enter", s.slide + 1, step.items.len());
            put(f, strip.x, strip.y - 1, strip.width, Line::styled(at, bold(t.primary)));
            if let Some(img) = &step.reference {
                art(f, images, &mut c.side_art, s.step as u64, || Cow::Borrowed(img), find);
            }
            if let Some(img) = step.items.get(s.slide) {
                let key = (s.step as u64) << 32 | s.slide as u64;
                art(f, images, &mut c.art, key, || Cow::Borrowed(img), strip);
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
        Task::Grid { prompt, image, cells: grid, single } => {
            let how = if *single { "pick one" } else { "pick all that fit" };
            put(f, area.x, area.y, area.width, Line::from(vec![Span::styled(prompt.clone(), Style::new().fg(t.text)), Span::styled(format!("  {how} · {expiry}"), dim())]));
            let cols = grid_cols(grid.len()) as u16;
            let rows = grid.len().div_ceil(cols.max(1) as usize) as u16;
            let mut space = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
            // The picture the cells are about, above them.
            if let Some(url) = image {
                let pic = Rect { height: space.height * 2 / 5, ..space };
                url_image(f, images, url, pic);
                space = Rect { y: pic.bottom() + 1, height: space.height.saturating_sub(pic.height + 1), ..space };
            }
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

/// The browser view with the keys' pointer drawn on it at `at` (its pixels): a red cross a
/// few cells wide, open in the middle so what's under it shows; `cell` is a cell's size in
/// its pixels, so it reads at any size.
fn with_pointer(img: &DynamicImage, at: Option<(u32, u32)>, cell: (u32, u32)) -> DynamicImage {
    let Some((px, py)) = at else { return img.clone() };
    let mut out = img.to_rgb8();
    let (w, h) = out.dimensions();
    let (cw, ch) = (cell.0.max(2), cell.1.max(2));
    let red = image::Rgb([230, 20, 40]);
    let mut put = |x: i64, y: i64| {
        if let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y))
            && x < w
            && y < h
        {
            out.put_pixel(x, y, red);
        }
    };
    let (px, py) = (i64::from(px), i64::from(py));
    // Arms two cells long, a third of a cell thick, starting half a cell out.
    let (tw, th) = (i64::from(cw / 3).max(1), i64::from(ch / 6).max(1));
    let (gap_x, gap_y) = (i64::from(cw / 2), i64::from(ch / 2));
    let (arm_x, arm_y) = (i64::from(cw) * 2, i64::from(ch));
    for d in gap_x..gap_x + arm_x {
        for t in -th..=th {
            put(px - d, py + t);
            put(px + d, py + t);
        }
    }
    for d in gap_y..gap_y + arm_y {
        for t in -tw..=tw {
            put(px + t, py - d);
            put(px + t, py + d);
        }
    }
    DynamicImage::ImageRgb8(out)
}

/// A captcha's picture by its URL (fetched like any other) or the key it came with, as big as
/// fits.
fn url_image(f: &mut Frame, images: &mut Images, url: &str, area: Rect) {
    match images.get(url, Size::new(area.width, area.height), Kind::Captcha) {
        State::Ready(p) => {
            let s = p.size();
            f.render_widget(Image::new(p), Rect::new(area.x, area.y, s.width, s.height).intersection(area));
        }
        State::Failed | State::Unavailable => put(f, area.x, area.y, area.width, Line::styled("(couldn't load the picture)", Style::new().fg(theme().error))),
        State::Loading | State::Rendering => put(f, area.x, area.y, area.width, Line::styled("Loading…", dim())),
    }
}

/// Draw `img` (made only when it's to be encoded) in `area`, encoded once for `key` and the
/// area, as big as fits and centered; where it was drawn.
fn art<'a>(f: &mut Frame, images: &Images, cache: &mut Option<Art>, key: u64, img: impl FnOnce() -> Cow<'a, DynamicImage>, area: Rect) -> Option<Rect> {
    if area.is_empty() {
        return None;
    }
    if cache.as_ref().is_none_or(|a| a.key != key || a.area != area) {
        *cache = images.encode_now(&img(), Size::new(area.width, area.height)).map(|proto| Art { key, area, proto });
    }
    let Some(a) = cache.as_ref() else {
        put(f, area.x, area.y, area.width, Line::styled("Images are off: turn them on in Settings to see this", Style::new().fg(theme().error)));
        return None;
    };
    let s = a.proto.size();
    let x = area.x + area.width.saturating_sub(s.width) / 2;
    let r = Rect::new(x, area.y, s.width, s.height).intersection(area);
    f.render_widget(Image::new(&a.proto), r);
    Some(r)
}
