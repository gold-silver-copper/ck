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
    let w = lines.iter().map(|l| l.width()).max().unwrap_or(0).max(title.width() + hint.width() + 8).max(60) + 6;
    let inner = panel(f, w.min(100) as u16, lines.len() as u16 + 3, &title, hint);
    for (i, line) in lines.into_iter().enumerate() {
        put(f, inner.x, inner.y + i as u16, inner.width, line);
    }
}

/// A big save asking first: what it will write, and where (a long folder wraps at its
/// slashes).
pub(super) fn draw_confirm(f: &mut Frame, app: &App) {
    let Some(Popup::Confirm(c)) = &app.popup else { return };
    let t = theme();
    let w = (c.lines.iter().map(|l| l.width()).max().unwrap_or(0).max(c.title.width() + 24) + 6).min(100) as u16;
    let width = (w.min(f.area().width.saturating_sub(4)).saturating_sub(4) as usize).max(10);
    let lines: Vec<String> = c.lines.iter().flat_map(|l| wrap_path(l, width)).collect();
    let inner = panel(f, w, lines.len() as u16 + 3, c.title, "enter save · esc cancel");
    for (i, line) in lines.into_iter().enumerate() {
        put(f, inner.x, inner.y + i as u16, inner.width, Line::styled(line, Style::new().fg(t.text)));
    }
}

/// `text` in lines of at most `width` columns, broken after a `/` where it can be.
pub(crate) fn wrap_path(text: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        // As many characters as fit (at least one), then back to just after the last slash.
        let (mut end, mut w) = (start, 0);
        while end < chars.len() {
            let cw = unicode_width::UnicodeWidthChar::width(chars[end]).unwrap_or(0);
            if w + cw > width && end > start {
                break;
            }
            w += cw;
            end += 1;
        }
        if end < chars.len()
            && let Some(slash) = (start..end).rev().find(|&i| chars[i] == '/').filter(|&i| i + 1 - start > (end - start) / 2)
        {
            end = slash + 1;
        }
        out.push(chars[start..end].iter().collect());
        start = end;
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
    list_rows(f, Rect { height: rows as u16, ..inner }, off, m.items.len(), Some(sel), |k| {
        let key = keys.get(k).cloned().unwrap_or_default();
        Line::from(vec![Span::styled(format!("{key:<key_w$}  "), bold(t.primary)), Span::styled(label(&m.items[k]), Style::new().fg(t.text))])
    });
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

pub(super) fn draw_preview(f: &mut Frame, app: &App) {
    let (Some(TabPopup::Preview(p)), Some(t)) = (&app.tab.popup, &app.tab.thread) else { return };
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
pub(super) fn draw_links(f: &mut Frame, app: &mut App) {
    let t = theme();
    let Some(TabPopup::Links(p)) = &mut app.tab.popup else { return };
    let w = f.area().width.saturating_sub(8).clamp(20, 110);
    let inner = panel(f, w, p.items.len() as u16 + 3, "Links", "enter open · y copy · esc close");
    let rows = inner.height as usize;
    let sel = p.list.selected().unwrap_or(0);
    let off = scroll_to(p.list.offset(), sel, rows);
    *p.list.offset_mut() = off;
    p.area = inner;
    list_rows(f, Rect { height: rows as u16, ..inner }, off, p.items.len(), Some(sel), |k| {
        let (kind, text, extra) = match &p.items[k] {
            LinkItem::Quote(_, label) => ("quote", label.clone(), String::new()),
            LinkItem::Url(u) => ("web", u.clone(), String::new()),
            LinkItem::File(file) => ("file", file.filename.clone(), format!("  {}", file.url)),
        };
        let room = (inner.width as usize).saturating_sub(9);
        let text = truncate(&text, room);
        let extra = truncate(&extra, room.saturating_sub(text.width()));
        Line::from(vec![
            chip(format!("{kind:<5}"), t.text_dim, t.surface_high),
            Span::raw("  "),
            Span::styled(text, Style::new().fg(if kind == "file" { t.text } else { t.quotelink })),
            Span::styled(extra, dim()),
        ])
    });
}

/// `R`: the reverse image search engines, per file.
pub(super) fn draw_image_search(f: &mut Frame, app: &mut App) {
    let t = theme();
    let names: Vec<String> = app.image_search.iter().map(|e| e.name.clone()).collect();
    let Some(Popup::ImageSearch(p)) = &mut app.popup else { return };
    let inner = panel(f, 64, p.rows.len() as u16 + 3, "Search for this image", "enter open · y copy · esc close");
    let rows = inner.height as usize;
    let sel = p.list.selected().unwrap_or(0);
    let off = scroll_to(p.list.offset(), sel, rows);
    *p.list.offset_mut() = off;
    p.area = inner;
    // File headers are never selected, so never painted.
    list_rows(f, Rect { height: rows as u16, ..inner }, off, p.rows.len(), Some(sel), |k| match &p.rows[k] {
        Err(file) => Line::styled(truncate(file, inner.width as usize), bold(t.primary)),
        Ok((_, e)) => Line::styled(format!("  {}", names[*e]), Style::new().fg(t.text)),
    });
}

/// Key help, by section, with the configured keys. Keep in sync with the README.
fn help_sections(keys: &KeyMap) -> Vec<(&'static str, Vec<(String, &'static str)>)> {
    let k = |a| keys.label(a);
    let pair = |a, b| format!("{} / {}", keys.label(a), keys.label(b));
    // Saving more than a file is in the menu, unless given keys.
    let bulk = [Action::DownloadPost, Action::DownloadThread, Action::Export];
    let saves = if bulk.iter().all(|&a| keys.keys(a).is_empty()) {
        format!("{} menu", keys.label(Action::Menu))
    } else {
        bulk.map(|a| if keys.keys(a).is_empty() { "-".to_string() } else { keys.label(a) }).join(" / ")
    };
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
                (format!("{} link", k(Action::Goto)), "add a site (any page of it)"),
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
                ("+ / - / 0".into(), "zoom in / out / fit"),
                ("i".into(), "open externally"),
                (pair(Action::Copy, Action::CopyLink), "copy file URL / post link"),
                (k(Action::Download), "save the file"),
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
                (k(Action::Download), "save the focused file (tab)"),
                (saves, "save: post / all files / page"),
                (format!("{} / {} / {}", k(Action::Watch), k(Action::NewTab), k(Action::Follow)), "watch / quote tab / general"),
                (format!("{} / {}", pair(Action::Hide, Action::ShowHidden), k(Action::Filter)), "hide / show hidden / filter"),
                (k(Action::Mine), "mark as yours (replies)"),
                (k(Action::Archive), "open a 404'd thread archived"),
                (pair(Action::Copy, Action::CopyLink), "copy text / link"),
            ],
        ),
    ]
}

pub(super) fn draw_help(f: &mut Frame, app: &App) {
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
    let scrolled = if let Some(Popup::Help(s)) = app.popup { s } else { 0 };
    let scroll = scrolled.min(rows.saturating_sub(inner.height)) as usize;
    for (c, lines) in cols.into_iter().enumerate() {
        let x = inner.x + c as u16 * (COL + 2);
        for (row, line) in lines.into_iter().skip(scroll).take(inner.height as usize).enumerate() {
            put(f, x, inner.y + row as u16, COL, line);
        }
    }
}

// ----- settings -----

/// `X`: a filter like the selected post.
pub(super) fn draw_add_filter(f: &mut Frame, app: &App) {
    use crate::filter::FilterAction;
    let t = theme();
    let Some(Popup::AddFilter(a)) = &app.popup else { return };
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
    let note = match &a.word {
        // `w`: a word to hide everywhere instead.
        Some(word) => {
            row(f, 3, "Word", vec![Span::styled(word.clone(), bold(t.text)), Span::styled("▏", Style::new().fg(t.primary))], "enter: hide it everywhere");
            "Any post with it is hidden, on every board (Settings › Filters › Hidden words)."
        }
        None => {
            row(f, 3, "", Vec::new(), "w: a word from it…");
            "Saved in the config as a [[filter]]; u right after takes it back."
        }
    };
    put(f, inner.x, y + 4, inner.width, Line::styled(note, dim()));
}
