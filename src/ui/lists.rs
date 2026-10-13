//! The lists: home, watched, history, saved, boards, search results, and the catalog (cards,
//! compact rows and the grid).

use std::fmt::Write as _;

use super::*;

pub(super) fn draw_sites(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let width = area.width.saturating_sub(PAD + 1) as usize;
    let rows = app.visible_sites();
    let mut state = app.list_state(View::Sites);
    app.drawn.body = draw_rows(f, area, rows.len(), &mut state, (1, 0), None, &|_| false, &mut |k| {
        let Some(&row) = rows.get(k) else { return Vec::new() };
        match row {
            SiteRow::Watched => {
                let n = app.store.all_watched().len();
                let (unread, _) = app.store.watched_new();
                let mut spans = vec![
                    Span::styled("◉  ", Style::new().fg(t.primary)),
                    Span::styled(pad("Watched", 16), bold(t.text)),
                    Span::styled(plural(n, "thread"), dim()),
                ];
                if unread > 0 {
                    spans.extend([Span::raw("  "), chip(format!("{unread} new"), t.background, t.new)]);
                }
                vec![Line::from(spans)]
            }
            SiteRow::History => vec![Line::from(vec![
                Span::styled("◷  ", Style::new().fg(t.primary)),
                Span::styled(pad("History", 16), bold(t.text)),
                Span::styled(plural(app.store.history.len(), "recent thread"), dim()),
            ])],
            SiteRow::Saved => {
                let dead = app.store.saved.iter().filter(|m| m.dead).count();
                let mut spans = vec![
                    Span::styled("▤  ", Style::new().fg(t.primary)),
                    Span::styled(pad("Saved", 16), bold(t.text)),
                    Span::styled(plural(app.store.saved.len(), "thread"), dim()),
                ];
                if dead > 0 {
                    spans.push(Span::styled(format!(", {dead} gone from the site"), dim()));
                }
                vec![Line::from(spans)]
            }
            SiteRow::Favorite(i) => {
                let Some(b) = app.favorites.get(i) else { return Vec::new() };
                let left = vec![
                    Span::styled("★  ", Style::new().fg(t.primary)),
                    Span::styled(col(&format!("{} /{}/", b.site, b.board), 16), bold(t.text)),
                    Span::styled(app.board_title(b).to_string(), dim()),
                ];
                // Its threads that are new since it was last opened; 1-9 open the first nine.
                let mut right = Vec::new();
                if let Some(&n) = app.favorites_new.get(&b.key()).filter(|&&n| n > 0) {
                    right.extend([chip(plural(n, "new thread"), t.background, t.new), Span::raw("  ")]);
                }
                if i < 9 {
                    right.push(chip(format!("{}", i + 1), t.text_dim, t.surface_high));
                }
                vec![spread(left, right, width)]
            }
            SiteRow::Recent(i) => {
                let r = app.recent_board(i);
                let (name, title) = r.as_ref().map_or((String::new(), ""), |r| (format!("{} /{}/", r.site, r.board), app.board_title(r)));
                vec![Line::from(vec![
                    Span::styled("↺  ", Style::new().fg(t.text_dim)),
                    Span::styled(col(&name, 16), Style::new().fg(t.text)),
                    Span::styled(title.to_string(), dim()),
                ])]
            }
            SiteRow::Site(i) => {
                let Some(s) = app.sites.get(i) else { return Vec::new() };
                let url = s.cfg.url.clone().unwrap_or_else(|| "https://4chan.org".into());
                let kind = s.cfg.kind.engine().name;
                let hidden = app.is_site_hidden(i);
                let spans = vec![
                    Span::raw("   "),
                    Span::styled(col(&s.cfg.name, 16), if hidden { dim() } else { bold(t.text) }),
                    chip(pad(kind, 9), t.text_dim, t.surface_high),
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
    app.list_scrolled(View::Sites, state.offset());
    if app.drawn.body.is_none() {
        empty(f, area, "No sites match");
    }
}

/// The site column of Watched, History and Saved: as wide as the longest site name (up to
/// 20), and a space.
fn site_width(app: &App) -> usize {
    app.sites.iter().map(|s| markup::columns(&s.cfg.name)).max().unwrap_or(0).clamp(9, 20) + 2
}

/// The board column of a list of `keys`: as wide as its longest `/board/` (from 8, up to
/// 16), and a space.
fn board_width<'a>(keys: impl Iterator<Item = &'a crate::store::ThreadKey>) -> usize {
    keys.map(|k| markup::columns(&k.board) + 2).max().unwrap_or(0).clamp(8, 16) + 2
}

/// `site  /board/  subject`, the shared start of Watched, History and Saved rows, in a row
/// `width` wide: the subject gets what's left (`spread` cuts it). On a narrow screen the
/// site goes, and the board takes only its own room.
fn thread_row(key: &crate::store::ThreadKey, subject: &str, width: usize, (site_w, board_w): (usize, usize)) -> Vec<Span<'static>> {
    let t = theme();
    let board = format!("/{}/", key.board);
    let mut row = Vec::new();
    if width >= 70 {
        row.push(Span::styled(col(&key.site, site_w), dim()));
    }
    row.push(Span::styled(if width >= 50 { col(&board, board_w) } else { format!("{board} ") }, bold(t.primary)));
    row.push(Span::styled(subject.to_string(), Style::new().fg(t.text)));
    row
}

pub(super) fn draw_watched(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let width = area.width.saturating_sub(PAD + 1) as usize;
    let columns = (site_width(app), board_width(app.store.all_watched().iter().map(|w| &w.key)));
    let rows = app.visible_watched();
    let mut state = app.list_state(View::Watched);
    app.drawn.body = draw_rows(f, area, rows.len(), &mut state, (1, 0), None, &|_| false, &mut |k| {
        let Some(w) = rows.get(k).and_then(|&i| app.store.all_watched().get(i)) else { return Vec::new() };
        let mut right = Vec::new();
        if app.refreshing.contains(&w.key) {
            right.push(Span::styled("↻  ", Style::new().fg(t.primary)));
        }
        if let Some(g) = &w.general {
            right.extend([chip(format!("follows {g}"), t.on_primary_container, t.primary_container), Span::raw(" ")]);
        }
        let (dead, (unread, replies)) = (w.status.is_dead(), w.status.counts());
        if w.at_limit && !dead {
            right.extend([chip("bump limit", t.text_dim, t.surface_high), Span::raw(" ")]);
        }
        if replies > 0 {
            right.extend([chip(format!("{replies} repl{} to you", if replies == 1 { "y" } else { "ies" }), t.on_primary, t.primary), Span::raw(" ")]);
        }
        if dead {
            right.extend([chip("archived/deleted", t.background, t.error), Span::raw("  ")]);
        } else if unread > 0 {
            right.extend([chip(format!("{unread} new"), t.background, t.new), Span::raw("  ")]);
        }
        // Its page in the board's index; on the last, it's next to fall off.
        match app.thread_page(&w.key).filter(|_| !dead) {
            Some((p, of)) if p >= of => right.extend([chip(format!("last page {p}/{of}"), t.background, t.warning), Span::raw("  ")]),
            Some((p, of)) => right.push(Span::styled(format!("p{p}/{of}  ·  "), dim())),
            None => {}
        }
        right.push(Span::styled(plural(w.posts, "post"), dim()));
        vec![spread(thread_row(&w.key, &w.subject, width, columns), right, width)]
    });
    app.list_scrolled(View::Watched, state.offset());
    if app.drawn.body.is_none() {
        let msg = format!("No watched threads. Press {} in a catalog or thread to watch one.", app.keys.key(Action::Watch));
        empty(f, area, &msg);
    }
}

pub(super) fn draw_history(f: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.saturating_sub(PAD + 1) as usize;
    let columns = (site_width(app), board_width(app.store.history.iter().map(|v| &v.key)));
    let rows = app.visible_history();
    let mut state = app.list_state(View::History);
    app.drawn.body = draw_rows(f, area, rows.len(), &mut state, (1, 0), None, &|_| false, &mut |k| {
        let Some(v) = rows.get(k).and_then(|&i| app.store.history.get(i)) else { return Vec::new() };
        vec![spread(thread_row(&v.key, &v.subject, width, columns), vec![Span::styled(ago(v.opened, app.clock), dim())], width)]
    });
    app.list_scrolled(View::History, state.offset());
    if app.drawn.body.is_none() {
        empty(f, area, "No history yet");
    }
}

pub(super) fn draw_saved(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let width = area.width.saturating_sub(PAD + 1) as usize;
    let columns = (site_width(app), board_width(app.store.saved.iter().map(|m| &m.key)));
    let rows = app.visible_saved();
    let mut state = app.list_state(View::Saved);
    app.drawn.body = draw_rows(f, area, rows.len(), &mut state, (1, 0), None, &|_| false, &mut |k| {
        let Some(m) = rows.get(k).and_then(|&i| app.store.saved.get(i)) else { return Vec::new() };
        let mut right = Vec::new();
        if m.dead {
            right.extend([chip("dead", t.background, t.error), Span::raw(" ")]);
        } else if app.store.watched(&m.key).is_some() {
            right.extend([chip("watching", t.on_primary_container, t.primary_container), Span::raw(" ")]);
        }
        right.push(Span::styled(format!(" No.{}  ·  {}  ·  {}", m.key.no, plural(m.posts, "post"), ago(m.saved, app.clock)), dim()));
        vec![spread(thread_row(&m.key, &m.subject, width, columns), right, width)]
    });
    app.list_scrolled(View::Saved, state.offset());
    if app.drawn.body.is_none() {
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
    // Names padded as wide as nearly all of them: a rare long one (8kun's) pushes its own
    // title along rather than every row's.
    let mut names: Vec<usize> = app.boards().iter().map(|b| markup::columns(&b.uri)).collect();
    names.sort_unstable();
    let width = names.get(names.len() * 9 / 10).copied().unwrap_or(1) + 4;
    let rows = app.visible_boards();
    let mut state = app.list_state(View::Boards);
    // At most `fit` columns: a title too long is cut short, keeping the name and the tag.
    let line = |b: &crate::model::Board, fit: usize| {
        let name = pad(&format!("/{}/ ", b.uri), width);
        let nsfw = if b.nsfw == Some(true) { "  nsfw" } else { "" };
        let room = fit.saturating_sub(markup::columns(&name) + markup::columns(nsfw));
        Line::from(vec![
            Span::styled(name, bold(t.primary)),
            Span::styled(truncate(&b.title, room), Style::new().fg(t.text)),
            Span::styled(nsfw, Style::new().fg(t.error)),
        ])
    };
    // On a wide screen, in columns as wide as two boards in three need (a longer title is
    // cut short), so a site's boards fit on it.
    let mut needs: Vec<usize> = app.boards().iter().map(|b| markup::spans_columns(&line(b, usize::MAX).spans)).collect();
    needs.sort_unstable();
    let cell = needs.get(needs.len() * 2 / 3).map_or(0, |&w| w as u16 + PAD + 3);
    let cols = area.width.checked_div(cell).map_or(1, usize::from);
    app.drawn.body = if cols > 1 && rows.len() > usize::from(area.height) {
        draw_cells(f, area, rows.len(), &mut state, (cols, area.width / cols as u16), &mut |k, fit| rows.get(k).and_then(|&i| app.boards().get(i)).map(|b| line(b, fit)).unwrap_or_default())
    } else {
        draw_rows(f, area, rows.len(), &mut state, (1, 0), None, &|_| false, &mut |k| rows.get(k).and_then(|&i| app.boards().get(i)).map(|b| line(b, usize::MAX)).into_iter().collect())
    };
    app.list_scrolled(View::Boards, state.offset());
    if app.drawn.body.is_none() && app.tab.loading().is_none() {
        empty(f, area, app.tab.failed.as_deref().unwrap_or("No boards"));
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
    let compact = layout == CatalogLayout::Compact;
    let thumbs = images && !compact;
    // Lines of text a card shows, and its thumbnail (as tall as the card, its shape kept).
    let text_rows = app.catalog_lines.map_or(if thumbs { CAT_THUMB.height - 1 } else { 1 }, |n| n.max(1));
    let thumb = Size::new((text_rows + 1).max(CAT_THUMB.height) * CAT_THUMB.width / CAT_THUMB.height, (text_rows + 1).max(CAT_THUMB.height));
    let width = area.width.saturating_sub(PAD + 2) as usize;
    let visible = app.visible_catalog();
    let mut state = app.list_state(View::Catalog);
    let mut build = |k: usize| -> Vec<Line<'static>> {
        let Some(&i) = visible.get(k) else { return Vec::new() };
        let Some(p) = app.tab.catalog.posts().get(i) else { return Vec::new() };
        let (hidden, highlight) = (app.tab.catalog.marks.why_hidden(i).is_some(), app.tab.catalog.marks.highlight(i));
        let mut head = Vec::new();
        if app.catalog_watching(p) {
            head.push(Span::styled(WATCHING, bold(t.primary)));
        }
        if hidden {
            head.extend([chip("hidden", t.text_dim, t.surface_high), Span::raw(" ")]);
        }
        if let Some(label) = highlight {
            head.extend([chip(label.to_string(), t.on_primary_container, t.primary_container), Span::raw(" ")]);
        }
        if app.tab.catalog.new.contains(&p.no) {
            head.extend([chip("new", t.background, t.new), Span::raw(" ")]);
        }
        if p.sticky {
            head.extend([chip("pinned", t.text_dim, t.surface_high), Span::raw(" ")]);
        }
        if p.locked {
            head.extend([chip("locked", t.text_dim, t.surface_high), Span::raw(" ")]);
        }
        // Overboards show where each thread lives.
        if let Some(b) = p.board.as_ref().filter(|b| app.tab.catalog.board().is_some_and(|cur| cur.uri != **b)) {
            head.extend([chip(format!("/{b}/"), t.on_primary_container, t.primary_container), Span::raw(" ")]);
        }
        // A thread without a subject has its text as its title, under (or, compact, beside)
        // the chips.
        let subject_style = if hidden { dim() } else { bold(t.text) };
        if let Some(s) = &p.subject {
            head.push(Span::styled(s.clone(), subject_style));
        }
        // Some overboards don't give counts; show nothing rather than zeros.
        let mut meta = Vec::new();
        if let Some(n) = app.new_replies(p) {
            meta.push(Span::styled(format!("+{n} "), bold(t.new)));
        }
        let text_w = if thumbs { width.saturating_sub(thumb.width as usize + 2) } else { width };
        // The counts in words with the thread's age and last reply; without its age; then
        // as the grid writes them, "R312 I58 · 2h": they give way to a subject that needs the
        // room, and shrink rather than vanish on a narrow screen.
        let facts = |level: u8| {
            let mut facts = Vec::new();
            if let Some(r) = p.replies {
                let images = p.images.unwrap_or(0) as usize;
                if level == 2 {
                    facts.push(format!("R{r} I{images}"));
                } else {
                    facts.push(count(r as usize, "reply", "replies"));
                    facts.push(plural(images, "image"));
                }
            }
            // How old it is, and (when it has replies) how long since the last: an old thread
            // can be the liveliest.
            let old = format!("{} old", ago(p.time, app.clock).trim_end_matches(" ago"));
            match last_reply(p) {
                Some(at) if level == 0 => facts.extend([old, format!("last reply {}", ago(at, app.clock))]),
                Some(at) if level == 1 => facts.push(format!("last reply {}", ago(at, app.clock))),
                Some(at) => facts.push(ago(at, app.clock)),
                None => facts.push(old),
            }
            facts.join(" · ")
        };
        let head_w = markup::spans_columns(&head);
        let fits = |f: &String| head_w.min(48) + markup::columns(f) + 4 <= text_w;
        let chosen = [facts(0), facts(1)].into_iter().find(fits).unwrap_or_else(|| facts(2));
        meta.push(Span::styled(chosen, dim()));
        // The subject gives way to the counts.
        let meta_w = markup::spans_columns(&meta);
        let head_room = text_w.saturating_sub(meta_w + 2);
        if head_w > head_room
            && p.subject.is_some()
            && let Some(last) = head.last_mut()
        {
            let others = head_w.saturating_sub(markup::columns(&last.content));
            last.content = truncate(&last.content, head_room.saturating_sub(others)).into();
        }
        if compact {
            let used = markup::spans_columns(&head) + markup::spans_columns(&meta);
            let room = text_w.saturating_sub(used + 4);
            let text = if p.subject.is_some() || hidden { dim() } else { Style::new().fg(t.text) };
            let gap = if head.is_empty() { "" } else { "  " };
            if room > 8 {
                head.push(Span::styled(format!("{gap}{}", truncate(p.plain_text(), room)), text));
            }
            return vec![spread(head, meta, text_w)];
        }
        // Without a subject, the text starts beside the chips.
        let mut text = p.plain_text();
        if p.subject.is_none() {
            let (first, rest) = first_words(text, head_room.saturating_sub(head_w));
            if !first.is_empty() {
                head.push(Span::styled(first.to_string(), if hidden { dim() } else { Style::new().fg(t.text) }));
                text = rest.trim_start();
            }
        }
        let mut lines = vec![spread(head, meta, text_w)];
        lines.extend(excerpt(text, Style::new().fg(t.text), text_w, usize::from(text_rows)));
        if thumbs {
            lines.resize(thumb.height as usize, Line::raw(""));
            lines = beside_tile(lines, thumb.width + 1);
        }
        lines
    };
    let (height, gap, card) = if compact {
        (1, 0, None)
    } else if thumbs {
        (thumb.height, 1, Some(t.surface))
    } else {
        (text_rows + 1, 1, Some(t.surface))
    };
    let highlighted = |k: usize| visible.get(k).is_some_and(|&i| app.tab.catalog.marks.highlight(i).is_some());
    app.drawn.body = draw_rows(f, area, visible.len(), &mut state, (height, gap), card, &highlighted, &mut build);
    app.list_scrolled(View::Catalog, state.offset());
    if app.drawn.body.is_none() {
        if app.tab.loading().is_none() {
            empty(f, area, app.tab.failed.as_deref().unwrap_or("No threads"));
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
    let offset = state.offset();
    for (k, &i) in visible.iter().enumerate().skip(offset).take(on_screen * 2) {
        let Some(p) = app.tab.catalog.posts().get(i) else { continue };
        let off = !app.catalog_images_on(p);
        let Some(file) = p.files.first() else { continue };
        let row = (k - offset) as u16 * per;
        if row < area.height {
            let tile = Rect::new(area.x + PAD, area.y + row, thumb.width, thumb.height);
            draw_tile(f, &mut app.images, file, p.files.len(), off, tile, area);
        } else if let Some(url) = file.thumb.as_ref().filter(|u| !off && http::is_media_host(u)) {
            app.images.want(url, Kind::Thumb);
        }
    }
}

/// When a catalog thread last had a reply, if it has any and the site says.
fn last_reply(p: &Post) -> Option<i64> {
    p.bumped.filter(|&b| p.replies.unwrap_or(0) > 0 && b > p.time)
}

/// Before a catalog thread you watch.
const WATCHING: &str = "◉ ";


/// Archive search results: each post with its thread, as cards.
pub(super) fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let mut state = app.list_state(View::Search);
    let Some(s) = &app.tab.search else { return };
    // Saved threads: how far the search has got, above the results.
    let area = match &s.saved {
        Some(saved) => {
            let line = if saved.finished {
                let skipped = if saved.skipped > 0 { format!(" ({} couldn't be read)", saved.skipped) } else { String::new() };
                format!("{} in {}{skipped}", plural(s.hits.len(), "post"), plural(saved.of, "saved thread"))
            } else {
                format!("searching… {} of {}", saved.done, plural(saved.of, "saved thread"))
            };
            put(f, area.x + PAD, area.y, area.width.saturating_sub(PAD), Line::styled(line, dim()));
            Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area }
        }
        None => area,
    };
    let width = area.width.saturating_sub(PAD + 2) as usize;
    let more = app.more_results();
    // The words searched for, to show and highlight in each result.
    let words: Vec<String> = s.query.to_lowercase().split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string()).filter(|w| w.chars().count() > 1).collect();
    let visible = app.visible_hits();
    let mut build = |row: usize| -> Vec<Line<'static>> {
        let Some(&k) = visible.get(row) else { return Vec::new() };
        let Some((thread, p)) = s.hits.get(k) else { return Vec::new() };
        let hidden = s.marks.why_hidden(k).is_some();
        let mut head = Vec::new();
        if hidden {
            head.extend([chip("hidden", t.text_dim, t.surface_high), Span::raw(" ")]);
        }
        head.extend([Span::styled(p.poster.name().to_string(), if hidden { dim() } else { bold(t.name) }), Span::raw("  ")]);
        let key = s.saved.as_ref().and_then(|x| x.keys.get(k));
        if let Some(key) = key {
            head.push(Span::styled(format!("{}/{}/{}  ", key.site, key.board, key.no), dim()));
        }
        if p.no == *thread {
            head.extend([chip("OP", t.on_primary_container, t.primary_container), Span::raw(" ")]);
        } else if key.is_none() {
            head.push(Span::styled(format!("in thread {thread}  "), dim()));
        }
        if let Some(subject) = &p.subject {
            head.push(Span::styled(subject.clone(), if hidden { dim() } else { bold(t.text) }));
        }
        let right = vec![Span::styled(format!("No.{}  ·  {}", p.no, ago(p.time, app.clock)), dim())];
        let mut lines = vec![spread(head, right, width)];
        // The text around what matched, highlighted.
        lines.extend(around_match(p.plain_text(), &words, width, 2));
        lines
    };
    let hit = draw_rows(f, area, visible.len(), &mut state, (3, 1), Some(t.surface), &|_| false, &mut build);
    if hit.is_none() && app.tab.loading().is_none() {
        empty(f, area, if s.hits.is_empty() { "No results".to_string() } else { format!("All hidden ({} shows them)", app.keys.how(Action::ShowHidden)) }.as_str());
    }
    // Below the last card: more to load.
    if more && let Some(Hit::List { offset, item_height, .. }) = hit {
        let shown = cells(visible.len().saturating_sub(offset)).saturating_mul(item_height);
        if shown < area.height {
            let hint = format!("{} more: {} or go down to load them", (s.total.unwrap_or(0) as usize).saturating_sub(s.hits.len()), app.keys.key(Action::NextMatch));
            put(f, area.x, area.y + shown, area.width, Line::styled(hint, dim()).centered());
        }
    }
    app.drawn.body = hit;
    app.list_scrolled(View::Search, state.offset());
}

/// `rows` lines of `text` from a little before the first of `words` (lowercase) in it, every
/// place one is highlighted; without one, from its start.
fn around_match(text: &str, words: &[String], width: usize, rows: usize) -> Vec<Line<'static>> {
    let t = theme();
    let (plain, hl) = (Style::new().fg(t.text), search_hl());
    let lower = text.to_lowercase();
    // Lowercasing can change lengths (rarely): then nothing is found.
    let mut found: Vec<(usize, usize)> = if lower.len() == text.len() { words.iter().flat_map(|w| lower.match_indices(w.as_str()).map(|(at, m)| (at, at + m.len()))).collect() } else { Vec::new() };
    found.sort_unstable();
    let Some(&(first, _)) = found.first() else { return excerpt(text, plain, width, rows) };
    // Some context before it, from the start of a word.
    let mut start = first.saturating_sub(width / 3);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    if start > 0 {
        start = text.get(start..first).and_then(|b| b.find(' ')).map_or(start, |sp| start + sp + 1);
    }
    let mut spans = vec![Span::styled(if start > 0 { "…" } else { "" }, plain)];
    let mut at = start;
    for (s, e) in found.into_iter().filter(|&(s, _)| s >= start) {
        // Overlapping matches (one word inside another) join the first.
        if s < at {
            continue;
        }
        spans.push(Span::styled(text.get(at..s).unwrap_or_default().to_string(), plain));
        spans.push(Span::styled(text.get(s..e).unwrap_or_default().to_string(), hl));
        at = e;
    }
    spans.push(Span::styled(text.get(at..).unwrap_or_default().to_string(), plain));
    let mut lines = markup::wrap(&Line::from(spans), width);
    if lines.len() > rows {
        lines.truncate(rows);
        // The last line shown ends in "…" to say there's more.
        if let Some(last) = lines.last_mut() {
            let mut cut = super::cut(std::mem::take(&mut last.spans), width.saturating_sub(1));
            if !cut.last().is_some_and(|s| s.content.ends_with('…')) {
                cut.push(Span::styled("…", plain));
            }
            last.spans = cut;
        }
    }
    lines
}

/// Grid cards: a thumbnail over three lines of text.
const GRID_CARD: Size = Size::new(20, 11);
/// A grid card's width on a wide screen.
const GRID_WIDE: u16 = 26;

/// The catalog as a grid of thumbnails, as many columns as fit.
fn draw_grid(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let visible = app.visible_catalog();
    if visible.is_empty() {
        if app.tab.loading().is_none() {
            empty(f, area, app.tab.failed.as_deref().unwrap_or("No threads"));
        }
        return;
    }
    // As many columns as fit; past a few, wider cards (fewer of them) so titles have room.
    // The columns share the width, leaving none of it empty on the right.
    let fit = |card: u16| (area.width + 2) / (card + 2);
    let cols = match fit(GRID_CARD.width) {
        n if n > 4 => fit(GRID_WIDE).max(4),
        n => n.max(1),
    };
    let (cell_w, cell_h) = (((area.width + 2) / cols).max(GRID_CARD.width + 2), GRID_CARD.height + 1);
    let card_w = cell_w - 2;
    let cols = usize::from(cols);
    let rows = ((area.height + 1) / cell_h).max(1) as usize;
    let mut state = app.list_state(View::Catalog);
    let sel = state.selected().unwrap_or(0).min(visible.len() - 1);
    let top = window(&mut state, visible.len(), rows, cols) / cols;
    app.list_scrolled(View::Catalog, top * cols);
    for (k, &i) in visible.iter().enumerate().skip(top * cols).take((rows + 1) * cols) {
        let (r, c) = (k / cols - top, k % cols);
        let (x, y) = (area.x + c as u16 * cell_w, area.y + r as u16 * cell_h);
        let card = Rect::new(x, y, card_w, GRID_CARD.height).intersection(area);
        let off = app.tab.catalog.posts().get(i).is_some_and(|p| !app.catalog_images_on(p));
        if card.is_empty() {
            // Below the screen: prefetch from media hosts.
            if let Some(url) = app.tab.catalog.posts().get(i).and_then(|p| p.files.first()).and_then(|f| f.thumb.as_ref()).filter(|u| !off && http::is_media_host(u)) {
                app.images.want(url, Kind::Thumb);
            }
            continue;
        }
        let Some(p) = app.tab.catalog.posts().get(i) else { continue };
        let hidden = app.tab.catalog.marks.why_hidden(i).is_some();
        paint_row(f, card, Some(t.surface), k == sel, app.tab.catalog.marks.highlight(i).is_some());
        let tile = Rect::new(x + PAD, y, THUMB.width, THUMB.height);
        match p.files.first() {
            Some(file) => draw_tile(f, &mut app.images, file, p.files.len(), off, tile, area),
            None => {
                fill(f, tile.intersection(area), t.surface_high);
                put(f, tile.x, tile.y + tile.height / 2, tile.width, Line::styled("no file", dim()).centered());
            }
        }
        let w = card_w - PAD - 1;
        let mut head = Vec::new();
        if app.catalog_watching(p) {
            head.push(Span::styled(WATCHING, bold(t.primary)));
        }
        if app.tab.catalog.new.contains(&p.no) {
            head.extend([chip("new", t.background, t.new), Span::raw(" ")]);
        }
        if hidden {
            head.extend([chip("hidden", t.text_dim, t.surface_high), Span::raw(" ")]);
        }
        let (title, rest) = match &p.subject {
            Some(s) => (s.clone(), p.plain_text().to_string()),
            None => (p.plain_text().to_string(), String::new()),
        };
        let used = markup::spans_columns(&head);
        head.push(Span::styled(truncate(&title, (w as usize).saturating_sub(used)), if hidden { dim() } else { bold(t.text) }));
        let mut facts = Vec::new();
        if let Some(n) = app.new_replies(p) {
            facts.push(Span::styled(format!("+{n} "), bold(t.new)));
        }
        // The counts in words as far as the card has room: "94 replies · 61 images · 2d",
        // then without the images, then as "R94 I61 · 2d".
        // Since the last reply (the thread's age without one).
        let age = ago(last_reply(p).unwrap_or(p.time), app.clock).trim_end_matches(" ago").to_string();
        let room = usize::from(w).saturating_sub(markup::spans_columns(&facts));
        let counts = match p.replies {
            Some(r) => {
                let (replies, images) = (count(r as usize, "reply", "replies"), plural(p.images.unwrap_or(0) as usize, "image"));
                let short = format!("R{r} I{} · {age}", p.images.unwrap_or(0));
                [format!("{replies} · {images} · {age}"), format!("{replies} · {age}")].into_iter().find(|c| markup::columns(c) <= room).unwrap_or(short)
            }
            None => age,
        };
        facts.push(Span::styled(counts, dim()));
        let text_y = y + THUMB.height;
        for (row, line) in [Line::from(head), Line::styled(truncate(&rest, w as usize), dim()), Line::from(facts)].into_iter().enumerate() {
            if text_y + (row as u16) < area.bottom() {
                put(f, x + PAD, text_y + row as u16, w, line);
            }
        }
    }
    app.drawn.body = Some(Hit::Grid { area, offset: top * cols, cols, cell: (cell_w, cell_h) });
}

/// The words at the start of `text` that fit in `width` columns, and the rest.
fn first_words(text: &str, width: usize) -> (&str, &str) {
    let mut end = 0;
    for (i, _) in text.match_indices(' ').chain(std::iter::once((text.len(), ""))) {
        if markup::columns(text.get(..i).unwrap_or_default()) > width {
            break;
        }
        end = i;
    }
    text.split_at_checked(end).unwrap_or((text, ""))
}

/// `text` wrapped to `width`, cut to `rows` lines with "…" if there's more.
fn excerpt(text: &str, style: Style, width: usize, rows: usize) -> Vec<Line<'static>> {
    // The style goes on the text, not the line: wrapping keeps only the text's.
    let mut lines = markup::wrap(&Line::from(Span::styled(text.to_string(), style)), width);
    if lines.len() > rows {
        lines.truncate(rows);
        // The last line shown ends in "…" to say there's more, styled as the lines above it.
        if let Some(last) = lines.last_mut() {
            let text: String = last.spans.iter().map(|s| s.content.as_ref()).collect::<String>() + "…";
            *last = Line::from(Span::styled(truncate(&text, width), style));
        }
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
/// A file's thumbnail in `tile` (`off`: images are off on its board, so a placeholder and
/// nothing asked for).
#[allow(clippy::too_many_arguments)]
/// The biggest file a lost thumbnail is made from, in bytes.
const THUMB_FROM_FILE_MAX: u64 = 4_000_000;

pub(super) fn draw_tile(f: &mut Frame, images: &mut Images, file: &Attachment, count: usize, off: bool, tile: Rect, clip: Rect) {
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
    if off {
        return label(f, "image off".into(), dim());
    }
    let mut kind = if file.spoiler {
        "spoiler".to_string()
    } else {
        Some(file.ext().to_uppercase()).filter(|e| !e.is_empty()).unwrap_or_else(|| "FILE".into())
    };
    if count > 1 {
        let _ = write!(kind, " +{}", count - 1);
    }
    let Some(thumb) = file.thumb.as_ref().filter(|_| whole) else {
        label(f, kind, dim());
        return;
    };
    let size = Size::new(tile.width, tile.height);
    // A thumbnail the site has lost (old posts on some sites): made from the file instead,
    // when that's an image small enough to fetch for it.
    let lost = matches!(images.get(thumb, size, Kind::Thumb), State::Failed);
    let small = file.is_image() && file.size.is_some_and(|s| s <= THUMB_FROM_FILE_MAX);
    let url = file.url.as_ref().filter(|_| lost && small).unwrap_or(thumb);
    match images.get(url, size, Kind::Thumb) {
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
