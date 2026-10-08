//! The image viewer and the gallery.

use super::*;

/// The thread's files (`V`): thumbnails with their post number and type.
pub(super) fn draw_gallery(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let off = !app.thread_images_on();
    let Some(g) = &mut app.tab.gallery else { return };
    let (card_w, card_h) = (THUMB.width + 4, THUMB.height + 1);
    let (cell_w, cell_h) = (card_w + 2, card_h + 1);
    let cols = ((area.width + 2) / cell_w).max(1) as usize;
    let rows = ((area.height + 1) / cell_h).max(1) as usize;
    let n = g.files.len();
    let sel = g.state.selected().unwrap_or(0).min(n - 1);
    let top = window(&mut g.state, n, rows, cols) / cols;
    for (k, (no, file)) in g.files.iter().enumerate().skip(top * cols).take((rows + 1) * cols) {
        let (r, c) = (k / cols - top, k % cols);
        let (x, y) = (area.x + c as u16 * cell_w, area.y + r as u16 * cell_h);
        let card = Rect::new(x, y, card_w, card_h).intersection(area);
        if card.is_empty() {
            if let Some(url) = file.thumb.as_ref().filter(|u| !off && http::is_media_host(u)) {
                app.images.want(url, Kind::Thumb);
            }
            continue;
        }
        paint_row(f, card, Some(t.surface), k == sel, false);
        draw_tile(f, &mut app.images, file, 1, off, Rect::new(x + PAD, y, THUMB.width, THUMB.height), area);
        let kind = file.ext().to_uppercase();
        let label = Line::from(vec![Span::styled(format!("No.{no}"), Style::new().fg(t.text)), Span::styled(format!("  {kind}"), dim())]);
        if y + THUMB.height < area.bottom() {
            put(f, x + PAD, y + THUMB.height, card_w - PAD - 1, label);
        }
    }
    app.drawn.body = Some(Hit::Grid { area, offset: top * cols, cols, cell: (cell_w, cell_h) });
}

pub(super) fn draw_viewer(f: &mut Frame, app: &mut App) {
    let t = theme();
    let Some(v) = app.tab.viewer() else { return };
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
    if let Some(no) = v.posts.get(v.index) {
        meta.push(format!("No.{no}"));
    }
    meta.push(format!("{} of {}", v.index + 1, v.files.len()));
    if !v.crop.is_fit() {
        meta.push(format!("{}%", v.crop.zoom));
    }
    let crop = v.crop;
    fill(f, top, t.bar);
    let line = Line::from(vec![
        Span::styled(" ck ", bold(t.on_primary).bg(t.primary)),
        Span::styled(format!("  {}", file.filename), bold(t.on_bar)),
        Span::styled(format!("    {}", meta.join("  ·  ")), dim()),
    ]);
    put(f, top.x, top.y, top.width, line);
    fill(f, bottom, t.bar);
    let mut hints = vec![Span::raw(" ")];
    if let Some(s) = app.status() {
        hints.extend(status_spans(s, t));
    } else {
        let save = app.keys.key(Action::Download);
        let mut keys: Vec<(&str, &str)> = if crop.is_fit() {
            vec![("h/l", "previous / next"), ("+/-", "zoom"), ("i", "open externally"), (&save, "save"), ("esc", "close")]
        } else {
            vec![("h/j/k/l", "move"), ("+/-", "zoom"), ("pgup/pgdn", "previous / next"), ("0, esc", "fit")]
        };
        keys.retain(|(k, _)| !k.is_empty());
        for (k, label) in keys {
            hints.extend([Span::styled(k.to_string(), bold(t.primary)), Span::styled(format!(" {label}   "), dim())]);
        }
    }
    put(f, bottom.x, bottom.y, bottom.width, Line::from(hints));
    let area = middle.inner(Margin::new(MARGIN, 0));
    // Non-images (videos, pdfs, ...) show their thumbnail, if any, with a hint; so do images
    // of a saved copy that weren't downloaded.
    let source = app.viewer_source(file);
    let mut inner = area;
    let thumb_only = source.as_ref().is_some_and(|(_, k)| *k == Kind::Thumb);
    if file.image().is_none() || thumb_only {
        let hint = match file.ext().to_uppercase() {
            _ if file.url.is_none() && file.thumb.is_none() => "Neither the file nor its thumbnail is available here.".to_string(),
            _ if file.url.is_none() => "Only the thumbnail is available here. Press i to open it externally.".to_string(),
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
    let Some((url, kind)) = source.filter(|_| !matches!(app.popup, Some(Popup::ImageSearch(_)))) else { return };
    let spinner = spinner(app.tick);
    // How much of the image this zoom shows here, for moving around (and so what's shown stays
    // inside the image).
    let mut crop = crop;
    if let (Some((w, h)), Some((cw, ch)), Some(v)) = (app.images.dims(&url), app.images.cell_size(), app.tab.viewer_mut()) {
        let shown = crop.shown(w, h, (u32::from(inner.width) * cw, u32::from(inner.height) * ch));
        crop = crop.within(shown);
        (v.shown, v.crop) = (Some(shown), crop);
    }
    match app.images.get_crop(&url, Size::new(inner.width, inner.height), kind, crop) {
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
