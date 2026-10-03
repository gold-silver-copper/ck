//! The lists: home, watched, history, saved, boards, search results, and the catalog (cards,
//! compact rows and the grid).

use super::*;

pub(super) fn draw_sites(f: &mut Frame, app: &mut App, area: Rect) {
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

pub(super) fn draw_watched(f: &mut Frame, app: &mut App, area: Rect) {
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

pub(super) fn draw_history(f: &mut Frame, app: &mut App, area: Rect) {
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

pub(super) fn draw_saved(f: &mut Frame, app: &mut App, area: Rect) {
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
            "No saved threads. Watched threads are saved as they refresh ({} watches one), and so is one you save as a page ({}).",
            app.keys.how(Action::Watch),
            app.keys.how(Action::Export)
        );
        empty(f, area, &msg);
    }
}

pub(super) fn draw_boards(f: &mut Frame, app: &mut App, area: Rect) {
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

pub(super) fn draw_catalog(f: &mut Frame, app: &mut App, area: Rect) {
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
pub(super) fn hidden_label(filter: &str) -> String {
    if filter.is_empty() { "hidden".into() } else { format!("hidden: {filter}") }
}

/// Archive search results: each post with its thread, as cards.
pub(super) fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
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
pub(super) fn beside_tile(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let blank = Span::raw(" ".repeat(width as usize));
    lines.into_iter().map(|l| Line::from([vec![blank.clone()], l.spans].concat()).style(l.style)).collect()
}

/// A thumbnail tile: a flat square with the file's type, and the image over it once it's
/// loaded and the tile is fully on screen (`clip`).
pub(super) fn draw_tile(f: &mut Frame, images: &mut Images, file: &Attachment, count: usize, tile: Rect, clip: Rect) {
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
