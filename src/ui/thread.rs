//! The thread: its layout (laid out as it comes on screen), each post's lines, and the peek
//! at a focused quote.

use super::*;

pub(super) fn draw_thread(f: &mut Frame, app: &mut App, area: Rect) {
    let off = app.tab.thread.as_ref().is_some_and(|t| !app.images_on(app.tab.site, &t.board));
    let Some(t) = &mut app.tab.thread else {
        if app.tab.loading.is_some() {
            return;
        }
        match app.tab.saved_offer.as_ref().and_then(|k| app.store.saved(k)) {
            Some(m) => empty(f, area, &format!("The thread is gone. enter opens its saved copy from {}.", ago(m.saved, app.clock))),
            None => empty(f, area, app.tab.failed.as_deref().unwrap_or("Thread not loaded")),
        }
        return;
    };
    let th = theme();
    // A taller or shorter screen: the selection stays in view.
    if t.viewport != area.height as usize {
        t.viewport = area.height as usize;
        t.reveal.get_or_insert(Reveal::Visible);
    }
    app.hit = Some(Hit::Thread { area });
    let thumbs = app.images.enabled() && area.width >= MIN_THUMB_WIDTH;
    let clock = app.clock;
    if t.layout.as_ref().is_none_or(|l| l.width != area.width || l.thumbs_on != thumbs) {
        // Lines cached at another width won't be used again.
        if t.cache_width != 0 && t.cache_width != area.width {
            t.cache.clear();
            t.estimates.clear();
            t.cache_width = area.width;
        }
        t.layout = Some(layout_thread(t, area.width, thumbs));
        // After a refresh, keep the same post at the top even if lines above it changed.
        if let Some((i, off)) = t.anchor.take() {
            lay_out_entries(t, [i], clock);
            if let Some(l) = &t.layout {
                t.scroll = (l.starts.get(i).copied().unwrap_or(0) + off).min(l.len().saturating_sub(t.viewport));
            }
        }
        t.reveal.get_or_insert(Reveal::Visible);
    }
    // Time moved on: what's laid out is laid out again (the line cache keeps what didn't
    // change), in place.
    if let Some(l) = t.layout.as_mut().filter(|l| l.at != clock.now()) {
        l.at = clock.now();
        l.exact.iter_mut().for_each(|x| *x = false);
    }
    settle(t, clock);
    // The selection moved (or everything was laid out again): into view, placed exactly.
    // Laying out what's around it can move it (estimates above it turning exact), so again,
    // until it stays.
    if let Some(how) = t.reveal.take() {
        for _ in 0..4 {
            lay_out_entries(t, [t.entry()], clock);
            let before = t.scroll;
            t.scroll_to(how);
            settle(t, clock);
            if t.scroll == before {
                break;
            }
        }
    }
    let cursor = t.entry();
    // A part just focused is scrolled into view (the post itself: its top).
    if std::mem::take(&mut t.follow_focus)
        && let Some(l) = t.layout.as_ref()
    {
        let at = l.spots.get(cursor).and_then(|s| s.iter().find(|s| Some(&s.part) == t.focus.as_ref()));
        let line = l.starts[cursor] + at.map_or(0, |s| s.line);
        if line < t.scroll {
            t.scroll = line;
        } else if line >= t.scroll + t.viewport {
            t.scroll = (line + 2).saturating_sub(t.viewport).min(l.len().saturating_sub(t.viewport));
        }
        settle(t, clock);
    }
    let Some(l) = t.layout.as_ref() else { return };
    // Where the text on the first and last rows ends, for the markers of a tall post.
    let mut ends = (area.x, area.x);
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
        let end = x.saturating_add(PAD).saturating_add(cells(line.width()));
        if row == 0 {
            ends.0 = end;
        }
        if row + 1 == area.height {
            ends.1 = end;
        }
    }

    // In a post taller than the screen: whether it goes on below, or started above.
    // Over the end of the row when the text leaves room; else just the arrow, in the margin
    // the text never reaches, so no text is covered.
    if let Some((_, above, below)) = t.tall() {
        let mark = |f: &mut Frame, y: u16, end: u16, text: &str, arrow: &str| {
            let w = cells(text.width()).saturating_add(2);
            let x = area.right().saturating_sub(w + 1);
            let (x, w, text) = if x > end { (x, w, format!(" {text} ")) } else { (area.right().saturating_sub(1), 1, arrow.to_string()) };
            put(f, x, y, w, Line::from(Span::styled(text, Style::new().fg(th.text_dim).bg(th.surface_high))));
        };
        if above {
            mark(f, area.y, ends.0, "↑", "↑");
        }
        if below {
            mark(f, area.bottom().saturating_sub(1), ends.1, "↓ more", "↓");
        }
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
                draw_tile(f, &mut app.images, &p.files[0], p.files.len(), off, tile, area);
            } else {
                fill(f, tile.intersection(area), th.surface_high);
            }
        }
    }
    for &(line, e) in &l.thumbs {
        let near = !(line >= top && line + h <= top + view) && line + h + view > top && line < top + 2 * view;
        if let Some(url) = t.posts[t.entries[e].post].files[0].thumb.as_ref().filter(|u| near && !off && http::is_media_host(u)) {
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

/// The width of an entry's text: what's left of `width` after its indent and padding.
fn text_width_of(width: u16, depth: u8) -> usize {
    width.saturating_sub(PAD + 2 + INDENT * depth as u16).max(10) as usize
}

/// Whether a post is drawn with a thumbnail tile beside its text.
fn with_thumb(thumbs: bool, p: &Post, text_width: usize) -> bool {
    thumbs && !p.files.is_empty() && text_width > THUMB.width as usize + 12
}

/// A thread's layout with every entry estimated (none laid out yet): see `lay_out_entries`.
fn layout_thread(t: &mut ThreadView, width: u16, thumbs: bool) -> ThreadLayout {
    let n = t.entries.len();
    let mut estimates = std::mem::take(&mut t.estimates);
    let blocks = (0..n).map(|e| blank(estimate(t, &mut estimates, e, width, thumbs))).collect();
    t.estimates = estimates;
    let mut l = ThreadLayout {
        width,
        blocks,
        starts: Vec::with_capacity(n + 1),
        thumbs: Vec::new(),
        spots: vec![Rc::from([]); n],
        exact: vec![false; n],
        has_thumb: vec![false; n],
        thumbs_on: thumbs,
        at: 0,
    };
    l.restart();
    l
}

thread_local! {
    /// Blank blocks by height, shared: what an entry not laid out yet holds.
    static BLANKS: std::cell::RefCell<HashMap<usize, Rc<[Line<'static>]>>> = Default::default();
}

pub(super) fn blank(height: usize) -> Rc<[Line<'static>]> {
    BLANKS.with(|b| b.borrow_mut().entry(height).or_insert_with(|| vec![Line::raw(""); height].into()).clone())
}

/// How tall an entry will be: exactly, if it was laid out at this width before (the line
/// cache has it), else from the length of its text (remembered in `estimates`).
fn estimate(t: &ThreadView, estimates: &mut HashMap<(u64, u16, bool), usize>, e: usize, width: u16, thumbs: bool) -> usize {
    let Some(entry) = t.entries.get(e) else { return 0 };
    let p = &t.posts[entry.post];
    if t.is_collapsed(entry.post) {
        return 2;
    }
    let text_width = text_width_of(width, entry.depth);
    let thumb = with_thumb(thumbs, p, text_width);
    let key = (p.no, text_width as u16, thumb);
    if let Some((_, block, _)) = t.cache.get(&key) {
        return block.len();
    }
    if let Some(&n) = estimates.get(&key) {
        return n;
    }
    let w = if thumb { text_width - THUMB.width as usize - 2 } else { text_width }.max(1);
    let rows = |width: usize| width.div_ceil(w).max(1);
    let mut n = 2 + p.files.len();
    if let Some(s) = &p.subject {
        n += rows(s.width());
    }
    if !p.body.is_empty() {
        n += 1 + p.body.iter().map(|l| rows(l.spans.iter().map(|s| s.content.width()).sum())).sum::<usize>();
    }
    if !t.backlinks[entry.post].is_empty() {
        n += 2;
    }
    n += 2;
    let n = if thumb { n.max(THUMB.height as usize + 3) } else { n };
    estimates.insert(key, n);
    n
}

/// Lay out entry `e` (a post, or a reply shown inline) as a card of wrapped lines: a padding
/// line above and below the content, then a gap line. Posts with files get a thumbnail tile
/// on the left. Unchanged posts come from the line cache. Returns its lines, where its parts
/// are, and whether it has a thumbnail.
fn lay_out(t: &ThreadView, cache: &mut LineCache, e: usize, width: u16, thumbs: bool, clock: Clock) -> (Rc<[Line<'static>]>, Rc<[Spot]>, bool) {
    let entry = &t.entries[e];
    let (i, p) = (entry.post, &t.posts[entry.post]);
    if t.is_collapsed(i) {
        // A hidden post is one line, so replies to it still make sense.
        let why = t.marks.get(i).and_then(|m| m.hidden.as_ref()?.filter());
        let why = why.map_or("hidden".to_string(), |l| format!("hidden by the filter \"{l}\""));
        let block = vec![Line::styled(format!("No.{}  {why}", p.no), Style::new().fg(theme().text_dim)), Line::raw("")];
        return (block.into(), Rc::from([]), false);
    }
    let text_width = text_width_of(width, entry.depth);
    let thumb = with_thumb(thumbs, p, text_width);
    let mut ctx = post_ctx(t, i, clock);
    ctx.focus = t.focus.as_ref().filter(|_| e == t.entry());
    let key = (p.no, text_width as u16, thumb);
    let shows = shown_with(t, i, p, &ctx);
    if let Some((_, block, at)) = cache.get(&key).filter(|(h, ..)| *h == shows) {
        return (block.clone(), at.clone(), thumb);
    }
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
    let (block, at): (Rc<[Line<'static>]>, Rc<[Spot]>) = (Rc::from(lines), Rc::from(at));
    cache.insert(key, (shows, block.clone(), at.clone()));
    (block, at, thumb)
}

/// Lay out these entries (those not laid out yet). Returns whether any was.
fn lay_out_entries(t: &mut ThreadView, entries: impl IntoIterator<Item = usize>, clock: Clock) -> bool {
    let Some(mut l) = t.layout.take() else { return false };
    let mut cache = std::mem::take(&mut t.cache);
    let mut any = false;
    for e in entries {
        if e >= l.blocks.len() || l.exact[e] {
            continue;
        }
        let (block, at, thumb) = lay_out(t, &mut cache, e, l.width, l.thumbs_on, clock);
        (l.blocks[e], l.spots[e], l.has_thumb[e], l.exact[e]) = (block, at, thumb, true);
        any = true;
    }
    if any {
        l.restart();
    }
    t.cache = cache;
    t.layout = Some(l);
    any
}

/// Lay out what's on screen, and a screen above and below (and the selected entry), keeping
/// the entry at the top where it is as estimates turn exact.
fn settle(t: &mut ThreadView, clock: Clock) {
    for _ in 0..8 {
        let Some(l) = &t.layout else { return };
        if l.blocks.is_empty() {
            return;
        }
        let view = t.viewport.max(1);
        t.scroll = t.scroll.min(l.len().saturating_sub(view));
        let top = l.entry_at(t.scroll);
        let off = t.scroll - l.starts[top];
        let (lo, hi) = (l.entry_at(t.scroll.saturating_sub(view)), l.entry_at(t.scroll + 2 * view));
        let cursor = t.entry();
        if !lay_out_entries(t, (lo..=hi).chain([cursor]), clock) {
            return;
        }
        let Some(l) = &t.layout else { return };
        t.scroll = (l.starts[top] + off).min(l.len().saturating_sub(view));
    }
}

/// Every entry laid out, as a full layout would (checks and benchmarks).
#[cfg(test)]
pub fn layout_all(t: &mut ThreadView, width: u16, thumbs: bool, clock: Clock) -> ThreadLayout {
    let mut cache = LineCache::new();
    let mut l = layout_thread(t, width, thumbs);
    for e in 0..t.entries.len() {
        let (block, at, thumb) = lay_out(t, &mut cache, e, width, thumbs, clock);
        (l.blocks[e], l.spots[e], l.has_thumb[e], l.exact[e]) = (block, at, thumb, true);
    }
    l.restart();
    l
}

/// A hash of what a post's lines show besides the post itself, for the line cache: its
/// "3h ago", OP/new/yours chips, revealed spoilers, filter marks, backlinks, and the search
/// highlight where it can appear.
fn shown_with(t: &ThreadView, i: usize, p: &Post, ctx: &PostCtx) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // The quotes drawn with " (OP)" or " (You)", found as drawing finds them (by the quote's
    // text, which may name a post the parser didn't count as a quote here).
    let marked: Vec<(u64, bool, bool)> = p
        .body
        .iter()
        .flat_map(|l| &l.spans)
        .filter(|s| markup::is_quote_link(s.style))
        .filter_map(|s| markup::quote_target(&s.content))
        .map(|n| (n, n == ctx.op_no, ctx.mine.contains(&n)))
        .filter(|&(_, op, mine)| op || mine)
        .collect();
    let quotes_marked = !marked.is_empty();
    // Matches are found in the post's own text; the " (OP)" / " (You)" added to quotes can
    // only be highlighted by a search for (part of) them.
    let s = ctx.search.as_str();
    let in_added = !s.is_empty() && (s.contains(['(', ')']) || " (op)".contains(s) || " (you)".contains(s));
    let highlighted = t.matches.binary_search(&i).is_ok() || (quotes_marked && in_added);
    (ago(p.time, ctx.clock), ctx.is_op, ctx.is_new, ctx.reveal, ctx.mark, ctx.mine.contains(&p.no), &marked, ctx.backlinks).hash(&mut h);
    // The post itself, which a refresh may bring changed (a file deleted, say).
    (&p.name, &p.subject, p.plain_text(), p.body.len()).hash(&mut h);
    for f in &p.files {
        (&f.url, &f.filename, f.width, f.height, f.size).hash(&mut h);
    }
    (ctx.focus, ctx.anchor).hash(&mut h);
    if highlighted {
        ctx.search.hash(&mut h);
    }
    h.finish()
}

/// How to render one post of a thread.
pub(super) struct PostCtx<'a> {
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

pub(super) fn post_ctx(t: &ThreadView, i: usize, clock: Clock) -> PostCtx<'_> {
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

pub(super) fn search_hl() -> Style {
    let t = theme();
    Style::new().fg(t.on_search).bg(t.search).add_modifier(Modifier::UNDERLINED)
}

/// A post as wrapped lines, and where its parts landed in them. Parts are drawn with a
/// tag (an underline color nothing else uses) that's found after wrapping, then cleared.
pub(super) fn post_lines(p: &Post, ctx: &PostCtx, width: usize) -> (Vec<Line<'static>>, Vec<Spot>) {
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
        // A hidden spoiler's text isn't drawn at all (hidden by color alone, it would show in
        // a monochrome terminal, to a screen reader, or in a copy of the screen).
        for s in &mut line.spans {
            if markup::is_spoiler(s.style) {
                s.content = markup::masked(&s.content).into();
            }
        }
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
                        s.style = s.style.fg(t.on_primary).bg(t.primary).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
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

/// A focused quote of a post in this thread shows that post, without taking the keys: at
/// the bottom of the thread, or the top when the quote is down there.
pub(super) fn draw_peek(f: &mut Frame, app: &App, area: Rect) {
    use crate::model::Target;
    let Some(Part::Link(Target::Quote(l))) = app.focused() else { return };
    let Some(t) = &app.tab.thread else { return };
    let Some(&i) = l.post.filter(|_| l.board.is_none() && l.thread.is_none_or(|n| n == t.no)).and_then(|n| t.index.get(&n)) else { return };
    let th = theme();
    let width = area.width.saturating_sub(6);
    let mut lines = post_lines(&t.posts[i], &post_ctx(t, i, app.clock), width as usize).0;
    let h = (cells(lines.len()).saturating_add(2)).min(area.height / 2).max(3);
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
