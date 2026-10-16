//! Popups over the screen: the menu, hints, quote preview, links, image search, help, the
//! save confirmation, adding a site, and adding a filter.

use super::*;

/// Adding a site: typing its link, asking it what it runs, then naming it.
pub(super) fn draw_adding(f: &mut Frame, app: &App) {
    use crate::app::Adding;
    let Some(Popup::Adding(a)) = &app.popup else { return };
    let t = theme();
    let field = |label: &str, text: &str| {
        Line::from(vec![
            Span::styled(format!("{label}  "), dim()),
            Span::styled(text.to_string(), Style::new().fg(t.text)),
            Span::styled("▏", Style::new().fg(t.primary)),
        ])
    };
    let text = |s: String| Line::styled(s, Style::new().fg(t.text));
    let (title, hint, lines): (String, &str, Vec<Line>) = match a {
        Adding::Typing(link) => (
            "Add a site".into(),
            "enter look · esc cancel",
            vec![
                field("Link", link),
                Line::default(),
                Line::styled("A link to any page of it: https://somechan.org/b/ or just somechan.org.", dim()),
                Line::styled("ck asks the site what it runs (jschan, LynxChan, vichan, …).", dim()),
            ],
        ),
        Adding::Looking { host, .. } => ("Add a site".into(), "esc cancel", vec![text(format!("Asking {host} what it runs…"))]),
        Adding::Site { site, name, .. } => {
            let host = crate::http::host(site.url.as_deref().unwrap_or_default()).to_string();
            let mut lines = vec![text(format!("It runs {}.", site.kind.label()))];
            // Where its files are, when that isn't the usual.
            let files: Vec<String> = [site.thumb_ext.as_ref().map(|e| format!("thumbnails are .{e}")), site.media_url.as_ref().map(|m| format!("files on {}", crate::http::host(m)))]
                .into_iter()
                .flatten()
                .collect();
            if !files.is_empty() {
                let mut what = files.join(", ");
                if let Some(c) = what.get(..1).map(str::to_uppercase) {
                    what.replace_range(..1, &c);
                }
                lines.push(text(format!("{what}.")));
            }
            match site.boards.as_deref() {
                Some([b]) => {
                    lines.push(text(format!("Its boards: /{}/ so far. vichan has no board list: Settings › Sites", b.uri())));
                    lines.push(text("adds more from links to them.".into()));
                }
                // Read from the board bar on its pages.
                Some(boards) => {
                    let names: Vec<&str> = boards.iter().map(|b| b.uri()).collect();
                    lines.push(text(format!("{} boards, from the list on its pages:", boards.len())));
                    lines.push(Line::styled(truncate(&names.join(" "), 80), dim()));
                }
                None => {}
            }
            lines.extend([
                Line::default(),
                field("Name", name),
                Line::styled(format!("For : and favorites, like {}/board. Saved in your config.", name.trim()), dim()),
            ]);
            (format!("Add {host}?"), "enter add · esc cancel", lines)
        }
        Adding::Boards { site, update, drop, builtin } => {
            let s = app.sites.get(*site);
            let name = s.map_or("", |s| s.cfg.name.as_str());
            let uris = |b: &[crate::config::BoardConfig]| b.iter().map(|b| format!("/{}/", b.uri())).collect::<Vec<_>>().join(" ");
            let mut lines = Vec::new();
            if !update.added.is_empty() {
                lines.push(text(format!("New: {}", truncate(&uris(&update.added), 80))));
            }
            if !update.renamed.is_empty() {
                let r: Vec<String> = update.renamed.iter().map(|(u, _, new)| format!("/{u}/ is now \"{new}\"")).collect();
                lines.push(text(truncate(&r.join(", "), 90)));
            }
            if !update.missing.is_empty() {
                let what = if *drop { "dropped" } else { "kept (d drops them)" };
                lines.push(text(format!("Not in its list now: {}, {what}", truncate(&uris(&update.missing), 60))));
            }
            if *builtin {
                lines.push(Line::default());
                lines.push(Line::styled(format!("{name} is built in: this saves it as one of your sites, with this list."), dim()));
            }
            (format!("Update {name}'s boards?"), "enter update · esc cancel", lines)
        }
        Adding::Board { site, board, .. } => {
            let name = app.sites.get(*site).map_or("", |s| s.cfg.name.as_str());
            (format!("Add /{board}/ to {name}?"), "enter add · esc cancel", vec![text(format!("{name} doesn't list /{board}/ yet; the site has it."))])
        }
    };
    let w = lines.iter().map(|l| markup::spans_columns(&l.spans)).max().unwrap_or(0).max(markup::columns(&title) + markup::columns(hint) + 8).max(60) + 6;
    text_panel(f, (w.min(100) as u16, &title, hint), &lines);
}

/// A big save asking first: what it will write, and where (a long folder wraps at its
/// slashes).
pub(super) fn draw_confirm(f: &mut Frame, app: &App) {
    let Some(Popup::Confirm(c)) = &app.popup else { return };
    let t = theme();
    let w = (c.lines.iter().map(|l| markup::columns(l)).max().unwrap_or(0).max(markup::columns(c.title) + 24) + 6).min(100) as u16;
    let width = (w.min(f.area().width.saturating_sub(4)).saturating_sub(4) as usize).max(10);
    let lines: Vec<_> = c.lines.iter().flat_map(|l| wrap_path(l, width)).map(|l| Line::styled(l, Style::new().fg(t.text))).collect();
    let keys = if c.files { "enter save · f with files · esc cancel" } else { "enter save · esc cancel" };
    text_panel(f, (w, c.title, keys), &lines);
}

/// `text` in lines of at most `width` columns, broken after a `/` where it can be.
pub(crate) fn wrap_path(text: &str, width: usize) -> Vec<String> {
    use unicode_segmentation::UnicodeSegmentation;
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        // As many graphemes as fit (at least one that's drawn), then back to just after the last slash.
        let (mut line, mut after) = markup::split_at_columns(rest, width);
        if markup::columns(line) == 0 {
            let end = rest.grapheme_indices(true).find(|(_, g)| markup::columns(g) > 0).map_or(rest.len(), |(i, g)| i + g.len());
            (line, after) = rest.split_at_checked(end).unwrap_or((rest, ""));
        }
        if !after.is_empty()
            && let Some((head, tail)) = line.grapheme_indices(true).rfind(|&(_, g)| g == "/").and_then(|(i, _)| rest.split_at_checked(i + 1)).filter(|(head, _)| markup::columns(head) > markup::columns(line) / 2)
        {
            (line, after) = (head, tail);
        }
        out.push(line.to_string());
        rest = after;
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// The menu for what's selected: each thing that can be done, with its key.
pub(super) fn draw_menu(f: &mut Frame, app: &mut App) {
    let t = theme();
    let keys: Vec<String> = match &app.popup {
        Some(Popup::Menu(m)) => m.items.iter().map(|i| app.menu_key(i)).collect(),
        _ => Vec::new(),
    };
    let Some(Popup::Menu(m)) = &mut app.popup else { return };
    let key_w = keys.iter().map(|k| markup::columns(k)).max().unwrap_or(1).max(5);
    let label = |i: &crate::app::MenuItem| match i {
        crate::app::MenuItem::Enter(l) | crate::app::MenuItem::Act(_, l) => l.clone(),
    };
    let w = (m.items.iter().map(|i| markup::columns(&label(i))).max().unwrap_or(10) + key_w + 8).max(markup::columns(&m.title) + 24) as u16;
    let title = if m.title.is_empty() { "Actions".to_string() } else { m.title.clone() };
    let hit = list_panel(f, (w.min(90), &title, "enter run · esc close"), &mut m.list, &m.items, ("", true), |_| Vec::new(), |k, item, _| {
        let key = keys.get(k).cloned().unwrap_or_default();
        Line::from(vec![Span::styled(format!("{}  ", pad(&key, key_w)), bold(t.primary)), Span::styled(label(item), Style::new().fg(t.text))])
    });
    app.drawn.popup = Some(hit);
}

/// Link hints: each label where its target is; typed letters dim, the rest bright.
pub(super) fn draw_hints(f: &mut Frame, app: &App) {
    let t = theme();
    let Some(Popup::Hints(h)) = &app.popup else { return };
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

pub(super) fn draw_preview(f: &mut Frame, app: &mut App) {
    let off = !app.thread_images_on();
    let (Some(TabPopup::Preview(p)), Some(t)) = (&mut app.tab.popup, &app.tab.thread) else { return };
    let w = f.area().width.saturating_sub(8).clamp(20, 110);
    let width = w.saturating_sub(4) as usize;
    let mut lines = Vec::new();
    let mut tiles = Vec::new();
    // Quoted posts a refresh has since dropped are left out.
    for &i in p.posts.iter().filter_map(|no| t.index.get(no)) {
        let at = lines.len();
        lines.extend(post_card(t, i, app.clock, width, app.images.enabled(), &mut tiles, at));
        lines.push(Line::raw(""));
    }
    for (link, quoted) in &p.elsewhere {
        let no = link.post.unwrap_or_default();
        let thread = link.thread.map_or_else(String::new, |th| format!(" in thread {th}"));
        let here = app.tab.board.as_ref().map(|b| b.uri.as_str());
        let board = link.board.as_ref().filter(|b| Some(b.as_str()) != here).map_or_else(String::new, |b| format!(" on /{b}/"));
        match quoted {
            Quoted::Found(post) => {
                lines.push(Line::from(vec![
                    Span::styled(post.poster.name().to_string(), bold(theme().name)),
                    Span::styled(format!("  {}  No.{no}{thread}{board}", ago(post.time, app.clock)), dim()),
                ]));
                for line in &post.body {
                    let painted = Line::from(line.spans.iter().map(|s| Span::styled(s.content.clone(), theme::paint(s.style))).collect::<Vec<_>>());
                    lines.extend(markup::wrap(&painted, width));
                }
            }
            Quoted::Loading => lines.push(Line::styled(format!(">>{no}{thread}{board}: loading…"), dim())),
            Quoted::Missing(why) => lines.push(Line::styled(format!(">>{no}{thread}{board}: {why}"), dim())),
        }
        lines.push(Line::raw(""));
    }
    while lines.last().is_some_and(|l| markup::spans_columns(&l.spans) == 0) {
        lines.pop();
    }
    // Not shorter than the last tile.
    let tiled = tiles.iter().map(|&(at, _)| at + THUMB.height as usize).max().unwrap_or(0);
    lines.resize(lines.len().max(tiled), Line::raw(""));
    let hit = list_panel(f, (w, "Quoted posts", "j/k scroll · enter jump · esc close"), &mut p.scroll, &lines, ("", false), |_| Vec::new(), |_, line, _| line.clone());
    if let Hit::List { area, offset, .. } = hit {
        draw_tiles(f, &mut app.images, &tiles, off, area, offset);
    }
    cover_tabs(&mut app.drawn.tabs, hit);
}

/// The selected post's links: quotes leading elsewhere, web links, files.
pub(super) fn draw_links(f: &mut Frame, app: &mut App) {
    let t = theme();
    let Some(TabPopup::Links(p)) = &mut app.tab.popup else { return };
    let w = f.area().width.saturating_sub(8).clamp(20, 110);
    let hit = list_panel(f, (w, "Links", "enter open · y copy · esc close"), &mut p.list, &p.items, ("", true), |_| Vec::new(), |_, item, width| {
        let (kind, text, extra) = match item {
            LinkItem::Quote(_, label) => ("quote", label.clone(), String::new()),
            LinkItem::Url(u) => ("web", u.clone(), String::new()),
            LinkItem::File(f) => (if f.url.is_some() { "file" } else { "thumb" }, f.filename.clone(), f.link().map(|(_, u)| format!("  {u}")).unwrap_or_default()),
        };
        let room = (width as usize).saturating_sub(9);
        let text = truncate(&text, room);
        let extra = truncate(&extra, room.saturating_sub(markup::columns(&text)));
        Line::from(vec![
            chip(pad(kind, 5), t.text_dim, t.surface_high),
            Span::raw("  "),
            Span::styled(text, Style::new().fg(if matches!(kind, "file" | "thumb") { t.text } else { t.quotelink })),
            Span::styled(extra, dim()),
        ])
    });
    app.drawn.popup = Some(hit);
    cover_tabs(&mut app.drawn.tabs, hit);
}

/// `R`: the reverse image search engines, per file.
pub(super) fn draw_image_search(f: &mut Frame, app: &mut App) {
    let t = theme();
    let names: Vec<String> = app.image_search.iter().map(|e| e.name.clone()).collect();
    let Some(Popup::ImageSearch(p)) = &mut app.popup else { return };
    // File headers are never selected, so never painted.
    let hit = list_panel(f, (64, "Search for this image", "enter open · y copy · esc close"), &mut p.list, &p.rows, ("", true), |_| Vec::new(), |_, row, width| match row {
        Err(file) => Line::styled(truncate(file, width as usize), bold(t.primary)),
        Ok((_, e)) => Line::styled(format!("  {}", names.get(*e).map_or("", String::as_str)), Style::new().fg(t.text)),
    });
    app.drawn.popup = Some(hit);
}

/// Every key, with yours, from the tables they work by: all of where you are, then
/// everywhere, then the rest, each where it works first.
fn help_sections(app: &App) -> Vec<(&'static str, Vec<(String, &'static str)>)> {
    let here = app.scope();
    let mut scopes = keys::SCOPES.to_vec();
    scopes.sort_by_key(|&s| (s != here, s != Scope::Global));
    scopes.into_iter().map(|s| (s.label(), app.keys.help(s, s == here))).filter(|(_, rows)| !rows.is_empty()).collect()
}

pub(super) fn draw_help(f: &mut Frame, app: &mut App) {
    // A column is this wide at least (when there's room for two or more), and at most.
    const NARROWEST: u16 = 50;
    const WIDEST: u16 = 72;
    // Keys take this many columns; what they do wraps in the rest.
    const KEYS: usize = 21;
    let t = theme();
    let sections = help_sections(app);
    // The keys' lines in columns `col` wide.
    let lay_out = |col: usize| -> Vec<Vec<Line<'static>>> {
        sections
            .iter()
            .map(|(title, rows)| {
                let mut lines = vec![Line::from(Span::styled(*title, bold(t.primary)))];
                for (k, v) in rows {
                    // A word too long for the column is cut, so the next column starts where it should.
                    for (i, part) in wrap_words(v, col.saturating_sub(KEYS)).into_iter().enumerate() {
                        let keys = if i == 0 { format!("  {} ", pad(&truncate(k, 18), 18)) } else { " ".repeat(KEYS) };
                        let part = truncate(&part, col.saturating_sub(KEYS));
                        lines.push(Line::from(vec![Span::styled(keys, bold(t.text)), Span::styled(part, Style::new().fg(t.text_dim))]));
                    }
                }
                lines.push(Line::raw(""));
                lines
            })
            .collect()
    };
    // As many columns as the help needs to fit the screen's height, as many as fit its width
    // if it doesn't, each as wide as there's room for (up to WIDEST).
    let area = f.area();
    let inside = area.width.saturating_sub(8);
    let width_of = |n: u16| inside.saturating_sub(2 * (n - 1)) / n;
    let most = (2..=4u16).rev().find(|&n| width_of(n) >= NARROWEST).unwrap_or(1);
    let (mut n, mut sections) = (1, Vec::new());
    for cols in 1..=most {
        n = cols;
        sections = lay_out(usize::from(width_of(cols).min(WIDEST)));
        let lines: usize = sections.iter().map(Vec::len).sum();
        if cells(lines.div_ceil(usize::from(cols))).saturating_add(4) <= area.height {
            break;
        }
    }
    let col = width_of(n).min(WIDEST);
    // Split at the section boundaries nearest an even share each.
    let total: usize = sections.iter().map(Vec::len).sum();
    let mut columns: Vec<Vec<Line>> = vec![Vec::new(); usize::from(n)];
    let (mut at, mut placed) = (0, 0);
    for s in sections {
        if at + 1 < columns.len() && placed > 0 && placed + s.len() / 2 >= total * (at + 1) / columns.len() {
            at += 1;
        }
        placed += s.len();
        if let Some(c) = columns.get_mut(at) {
            c.extend(s);
        }
    }
    for c in &mut columns {
        while c.last().is_some_and(|l| markup::spans_columns(&l.spans) == 0) {
            c.pop();
        }
    }
    // A row is each column's line, side by side.
    let rows = columns.iter().map(Vec::len).max().unwrap_or(0);
    let mut columns = columns.into_iter();
    let mut left = columns.next().unwrap_or_default();
    left.resize(rows, Line::default());
    for (k, column) in columns.enumerate() {
        // Where this column starts: after k + 1 columns and their gaps.
        let start = (k + 1) * (usize::from(col) + 2);
        for (l, r) in left.iter_mut().zip(column) {
            l.spans.push(Span::raw(" ".repeat(start.saturating_sub(markup::spans_columns(&l.spans)))));
            l.spans.extend(r.spans);
        }
    }
    let w = n * col + 2 * (n - 1) + 4;
    let scrolls = cells(left.len()).saturating_add(3) > room(f, 0, cells(left.len()).saturating_add(3)).1;
    let hint = format!("{}images: {} · esc close", if scrolls { "j/k scroll · " } else { "" }, app.images.protocol_name());
    // In small terminals the help scrolls (j/k).
    let Some(Popup::Help(scroll)) = &mut app.popup else { return };
    let hit = list_panel(f, (w, "Keys", &hint), scroll, &left, ("", false), |_| Vec::new(), |_, line, _| line.clone());
    if let Hit::List { area, offset, .. } = hit
        && left.len() > offset + area.height as usize
        && area.height > 0
    {
        // Over the whole row, so no word of it runs into the marker.
        let more = format!("{}↓ more (j)", " ".repeat(usize::from(area.width).saturating_sub(10)));
        put(f, area.x, area.bottom() - 1, area.width, Line::styled(more, bold(t.primary)));
    }
}

// ----- settings -----

/// `X`: a filter like the selected post.
pub(super) fn draw_add_filter(f: &mut Frame, app: &mut App) {
    use crate::filter::FilterAction;
    let t = theme();
    let Some(Popup::AddFilter(a)) = &mut app.popup else { return };
    let (verb, other) = match a.action {
        FilterAction::Hide => ("hide them", "a: highlight instead"),
        FilterAction::Highlight => ("highlight them", "a: hide instead"),
    };
    let (reach, label) = (a.reach_text(), a.label());
    // Under the candidates: what it does, where, its label, and a word instead.
    let foot = |width: u16| {
        let row = |name: &str, mut value: Vec<Span<'static>>, key: &str| {
            value.insert(0, Span::styled(pad(name, 8), dim()));
            spread(value, vec![Span::styled(key.to_string(), dim())], width as usize)
        };
        let typed = |text: &String| vec![Span::styled(text.clone(), bold(t.text)), Span::styled("▏", Style::new().fg(t.primary))];
        // A word is hidden everywhere, with no label: what, where and the label are a filter's.
        if let Some(word) = &a.word {
            return vec![
                row("Word", typed(word), "enter: hide it everywhere"),
                Line::styled("Any post with it is hidden, on every board (Settings › Filters › Hidden words).", dim()),
            ];
        }
        vec![
            row("Do", vec![Span::styled(verb, bold(t.text))], other),
            row("Where", vec![Span::styled(reach, bold(t.text))], "s: change"),
            match &a.typing {
                Some(text) => row("Label", typed(text), "enter: keep"),
                None => row("Label", vec![Span::styled(label, bold(t.text))], "e: edit"),
            },
            // `w`: a word to hide everywhere instead.
            row("", Vec::new(), "w: a word from it…"),
            Line::styled("Saved in the config as a [[filter]]; u right after takes it back.", dim()),
        ]
    };
    list_panel(f, (72, &format!("Filter like No.{}", a.post), "enter add · esc cancel"), &mut a.list, &a.candidates, ("Nothing in it to filter on but its words: type one", true), foot, |_, c, width| {
        let room = (width as usize).saturating_sub(c.field.as_str().len() + 2);
        let left = vec![Span::styled(truncate(&c.what, room), Style::new().fg(t.text))];
        spread(left, vec![Span::styled(c.field.as_str(), dim())], width as usize)
    });
}
