//! The settings screen and its popups: themes, colors, keys, filters, your sites.

use super::*;

pub(super) fn draw_settings(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let selected = app.settings_list.state.selected().unwrap_or(0);
    let rows = setting_rows();
    // Scroll so the selected setting (and the note under the list, at the end) shows.
    let at = rows.iter().position(|r| *r == Ok(selected)).unwrap_or(0);
    let total = rows.len() + 2;
    let offset = if at + 1 == rows.len() { total } else { at + 2 }.saturating_sub(area.height as usize);
    app.hit = Some(Hit::Settings { area, offset });
    let items: Vec<_> = SETTING_SECTIONS.iter().flat_map(|(_, items)| items.iter().copied()).collect();
    for (row, r) in rows.into_iter().enumerate().skip(offset) {
        let y = area.y + (row - offset) as u16;
        if y >= area.bottom() {
            break;
        }
        match r {
            Err(title) => put(f, area.x, y, area.width, Line::styled(title.to_string(), bold(t.primary))),
            Ok(i) => {
                let (item, label, hint) = items[i];
                paint_row(f, Rect::new(area.x, y, area.width, 1), None, i == selected, false);
                let value = app.setting_value(item);
                let hint_w = (area.width as usize).saturating_sub(PAD as usize + 1 + 18 + 34);
                let line = Line::from(vec![
                    Span::styled(format!("{label:<18}"), Style::new().fg(t.text)),
                    Span::styled(format!("{:<34}", truncate(&value, 32)), bold(t.text)),
                    Span::styled(if hint_w >= 16 { truncate(hint, hint_w) } else { String::new() }, dim()),
                ]);
                put(f, area.x + PAD, y, area.width.saturating_sub(PAD + 1), line);
            }
        }
    }
    let y = (area.y as usize + setting_rows().len() + 1).checked_sub(offset).map_or(area.bottom(), |y| y as u16);
    if y < area.bottom() {
        let note = match &app.config_path {
            Some(p) => format!(
                "Saved to {}, comments kept. Themes, colors and keys can be edited there too.",
                crate::app::tilde(&p.display().to_string())
            ),
            None => "There's no config file to save to (no home directory); changes last until ck quits.".into(),
        };
        put(f, area.x, y, area.width, Line::styled(note, dim()));
    }
}

pub(super) fn draw_settings_popup(f: &mut Frame, app: &App) {
    let t = theme();
    let popup = match &app.popup {
        Some(Popup::Settings(p)) => Some(p),
        _ => None,
    };
    match popup {
        Some(SettingsPopup::Themes { list, names, .. }) => {
            let inner = panel(f, 52, names.len() as u16 + 3, "Theme", "enter keep · esc cancel");
            list_rows(f, inner, 0, names.len(), list.selected().or(Some(0)), |k| {
                let name = &names[k];
                let mut spans = vec![Span::styled(format!("{name:<22}"), Style::new().fg(t.text))];
                // A row of colored cells: the theme at a glance.
                if let Ok(th) = theme::resolve(name, &app.themes) {
                    let colors = [th.background, th.surface, th.selection, th.primary, th.primary_container, th.greentext, th.quotelink, th.heading, th.new];
                    spans.extend(colors.map(|c| Span::styled("  ", Style::new().bg(c))));
                }
                Line::from(spans)
            });
        }
        Some(SettingsPopup::Colors { list, editing }) => {
            let title = format!("Colors · {}", app.theme_name);
            let hint = if editing.is_some() { "enter save · esc cancel" } else { "enter edit · x reset · esc close" };
            let h = (ROLES.len() as u16 + 5).min(f.area().height.saturating_sub(4));
            let inner = panel(f, 96, h, &title, hint);
            let rows = inner.height.saturating_sub(2) as usize;
            let sel = list.selected().unwrap_or(0);
            let first = sel.saturating_sub(rows.saturating_sub(1));
            list_rows(f, Rect { height: rows as u16, ..inner }, first, ROLES.len(), Some(sel), |k| {
                let (role, desc) = ROLES[k];
                let c = t.get(role).unwrap_or(Color::Reset);
                Line::from(vec![
                    Span::styled("    ", Style::new().bg(c)),
                    Span::styled(format!("  {role:<22}"), Style::new().fg(t.text)),
                    Span::styled(format!("{:<10}", theme::color_string(c)), bold(t.text)),
                    Span::styled(desc, dim()),
                ])
            });
            let y = inner.bottom().saturating_sub(1);
            let line = match editing {
                Some(text) => Line::from(vec![
                    Span::styled(format!("New {} color: ", ROLES.get(sel).map_or("", |r| r.0)), Style::new().fg(t.text)),
                    Span::styled(text.clone(), bold(t.text)),
                    Span::styled("▏", Style::new().fg(t.primary)),
                    Span::styled("   #rrggbb, a name, 0-255, or default", dim()),
                ]),
                None => Line::styled("Changing a color of a built-in theme saves it as a copy: NAME-custom.", dim()),
            };
            put(f, inner.x, y, inner.width, line);
        }
        Some(SettingsPopup::Keys { list, capture }) => {
            let rows = key_rows();
            let hint = if capture.is_some() { "press a key · esc cancel" } else { "enter rebind · a add · u unbind · x reset · esc close" };
            let h = (rows.len() as u16 + 5).min(f.area().height.saturating_sub(4));
            let inner = panel(f, 96, h, "Keys", hint);
            let view = inner.height.saturating_sub(2) as usize;
            let sel = list.selected().unwrap_or(0);
            let first = (sel + 2).saturating_sub(view).min(rows.len().saturating_sub(view));
            // Group titles are never selected, so never painted.
            list_rows(f, Rect { height: view as u16, ..inner }, first, rows.len(), Some(sel), |k| {
                let i = match rows[k] {
                    Err(title) => return Line::styled(title.to_string(), bold(t.primary)),
                    Ok(i) => i,
                };
                let (action, name, _, scopes, desc) = keys::ACTIONS[i];
                let changed = !app.keys.is_default(action);
                let key_style = if changed { bold(t.primary) } else { bold(t.text) };
                let scopes = scopes.iter().map(|s| s.label()).collect::<Vec<_>>().join(", ");
                let label = app.keys.label(action);
                let mut spans = match label.as_str() {
                    "" => vec![Span::styled(format!("  {:<16}", "menu"), dim())],
                    _ => vec![Span::styled(format!("  {:<16}", truncate(&label, 15)), key_style)],
                };
                // Narrow: just the key and what it does.
                if inner.width >= 80 {
                    spans.push(Span::styled(format!("{name:<17}"), dim()));
                    spans.push(Span::styled(format!("{:<37}", truncate(desc, 36)), Style::new().fg(t.text)));
                    spans.push(Span::styled(truncate(&scopes, (inner.width as usize).saturating_sub(72)), dim()));
                } else {
                    spans.push(Span::styled(truncate(desc, (inner.width as usize).saturating_sub(18)), Style::new().fg(t.text)));
                }
                Line::from(spans)
            });
            let y = inner.bottom().saturating_sub(1);
            let line = match (capture, rows.get(sel)) {
                (Some(add), Some(Ok(i))) => {
                    let verb = if *add { "Press a key to add to" } else { "Press the new key for" };
                    Line::from(vec![
                        Span::styled(format!("{verb} "), Style::new().fg(t.text)),
                        Span::styled(keys::ACTIONS[*i].1, bold(t.primary)),
                        Span::styled("   esc cancels", dim()),
                    ])
                }
                _ => Line::styled("Changed keys are saved in [keys]; navigation keys are fixed. Without a key: in the . menu.", dim()),
            };
            put(f, inner.x, y, inner.width, line);
        }
        Some(SettingsPopup::Filters { list, counts }) => draw_filter_list(f, app, list, counts),
        Some(SettingsPopup::Sites(m)) => draw_my_sites(f, m),
        Some(SettingsPopup::BoardImages { list }) => draw_board_images(f, app, list),
        Some(SettingsPopup::HiddenWords { list, typing }) => draw_hidden_words(f, app, list, typing.as_deref()),
        Some(SettingsPopup::FilterEdit { index, draft, row, typing }) => draw_filter_edit(f, app, *index, draft, *row, typing.as_deref()),
        Some(SettingsPopup::Folder { value }) => {
            let inner = panel(f, 90, 7, "Download folder", "enter save · esc cancel");
            let field = Rect::new(inner.x, inner.y, inner.width, 1);
            fill(f, field, t.surface);
            let line = Line::from(vec![Span::styled(value.clone(), Style::new().fg(t.text)), Span::styled("▏", Style::new().fg(t.primary))]);
            put(f, inner.x + 1, inner.y, inner.width.saturating_sub(2), line);
            let help = "{site}, {board}, {thread} and {downloads} are filled in. Empty for the default.";
            put(f, inner.x, inner.y + 2, inner.width, Line::styled(help, dim()));
        }
        None => {}
    }
}

// ----- filters -----

/// Where a filter applies, in words.
fn filter_scope(c: &crate::filter::FilterConfig) -> String {
    let boards = c.boards.iter().map(|b| format!("/{b}/")).collect::<Vec<_>>().join(" ");
    match (c.sites.is_empty(), c.boards.is_empty()) {
        (true, true) => "everywhere".into(),
        (false, true) => c.sites.join(", "),
        (true, false) => format!("{boards} on any site"),
        (false, false) => format!("{boards} on {}", c.sites.join(", ")),
    }
}

fn counts_text((posts, threads): (usize, usize)) -> String {
    match (posts, threads) {
        (0, 0) => "nothing here".into(),
        (p, 0) => plural(p, "post"),
        (0, t) => plural(t, "thread"),
        (p, t) => format!("{}, {}", plural(p, "post"), plural(t, "thread")),
    }
}

/// Settings › Catalog › Hidden words.
fn draw_hidden_words(f: &mut Frame, app: &App, list: &ListState, typing: Option<&str>) {
    let t = theme();
    let words = &app.hidden_words;
    let hint = if typing.is_some() { "enter add · esc cancel" } else { "a add · x remove · esc close" };
    let empty = if typing.is_some() { "" } else { "None yet. a adds one; so does w in a post's X." };
    let sel = list.selected().unwrap_or(0);
    let inner = list_panel(f, (70, "Hidden words", hint), words.len(), (sel, typing.is_none()), (empty, "Whole words, any case. Kept in the config as hidden_words."), (1, 1), |k, _| {
        Line::styled(words[k].clone(), Style::new().fg(t.text))
    });
    if let Some(text) = typing {
        let line = Line::from(vec![Span::styled("New  ", dim()), Span::styled(text.to_string(), bold(t.text)), Span::styled("▏", Style::new().fg(t.primary))]);
        put(f, inner.x, inner.bottom().saturating_sub(2), inner.width, line);
    }
}

/// Settings › Catalog › Board images: boards with their own image setting.
fn draw_board_images(f: &mut Frame, app: &App, list: &ListState) {
    let t = theme();
    let boards = app.boards_with_images_set();
    let empty = "None: every board follows the default. A board's . menu changes it.";
    let note = "Boards the site marks NSFW follow the NSFW boards setting; the rest show images.";
    list_panel(f, (70, "Board images", "x back to the default · esc close"), boards.len(), (list.selected().unwrap_or(0), true), (empty, note), (0, 0), |k, width| {
        let (key, on) = &boards[k];
        spread(vec![Span::styled(key.clone(), Style::new().fg(t.text))], vec![Span::styled(if *on { "images on" } else { "images off" }, dim())], width as usize)
    });
}

/// Settings › Sites › Your sites: the config's `[[site]]` tables.
fn draw_my_sites(f: &mut Frame, m: &crate::app::MySites) {
    let t = theme();
    let empty = "None yet: only the built-in sites. a adds one from a link to it.";
    let note = "Kept in the config as [[site]] tables; the built-in sites are always there too.";
    let hint = "a add · r update a vichan site's boards · x remove · esc close";
    list_panel(f, (100, "Your sites", hint), m.sites.len(), (m.list.selected().unwrap_or(0), true), (empty, note), (1, 0), |k, width| {
        let s = &m.sites[k];
        let left = vec![
            Span::styled(format!("{:<18}", truncate(&s.name, 17)), bold(t.text)),
            Span::styled(format!("{:<11}", s.kind.as_str()), Style::new().fg(t.text)),
            Span::styled(truncate(s.url.as_deref().unwrap_or(""), (width as usize).saturating_sub(52)), dim()),
        ];
        let tag = if m.armed == Some(k) { "x again removes it".to_string() } else { crate::app::site_origin(s).to_string() };
        spread(left, vec![Span::styled(tag, dim())], width as usize)
    });
}

fn draw_filter_list(f: &mut Frame, app: &App, list: &ratatui::widgets::ListState, counts: &[(usize, usize)]) {
    let t = theme();
    let cfgs = &app.filter_cfgs;
    let empty = format!("No filters yet. a adds one; {} on a post makes one like it.", app.keys.key(Action::Filter));
    let note = "Counts are for the open catalog and thread. Kept in the config as [[filter]] tables.";
    let hint = "enter edit · space on/off · a add · x remove · esc close";
    list_panel(f, (110, "Filters", hint), cfgs.len(), (list.selected().unwrap_or(0), true), (&empty, note), (1, 0), |k, width| {
        let c = &cfgs[k];
        let wide = width >= 90;
        let style = if c.enabled { Style::new().fg(t.text) } else { dim() };
        let fields = c.fields().iter().map(|f| f.as_str()).collect::<Vec<_>>().join("+");
        let count = counts.get(k).map_or(String::new(), |&n| counts_text(n));
        // Narrow: the label and fields share what the count leaves.
        let (label_w, fields_w) = if wide { (22, 18) } else { ((width as usize).saturating_sub(count.width() + 15) * 3 / 5, (width as usize).saturating_sub(count.width() + 15) * 2 / 5) };
        let mut left = vec![
            if c.enabled { chip(format!("{:<9}", c.action.as_str()), t.on_primary_container, t.primary_container) } else { chip(format!("{:<9}", "off"), t.text_dim, t.surface_high) },
            Span::raw("  "),
            Span::styled(format!("{:<label_w$}", truncate(c.label(), label_w.saturating_sub(1))), if c.enabled { bold(t.text) } else { dim() }),
            Span::styled(format!("{:<fields_w$}", truncate(&fields, fields_w.saturating_sub(1))), style),
        ];
        if wide {
            left.push(Span::styled(format!("{:<20}", truncate(&filter_scope(c), 19)), style));
            // The pattern gets what's left, beside the count.
            let used: usize = left.iter().map(|s| s.width()).sum::<usize>() + count.width() + 2;
            left.push(Span::styled(truncate(&c.pattern, (width as usize).saturating_sub(used)), dim()));
        }
        let right = vec![Span::styled(count, dim())];
        spread(left, right, width as usize)
    });
}

/// One filter in the editor.
fn draw_filter_edit(f: &mut Frame, app: &App, index: Option<usize>, draft: &crate::filter::FilterConfig, row: usize, typing: Option<&str>) {
    use crate::app::{EDIT_ROWS, EditRow};
    let t = theme();
    let title = match index {
        Some(_) => format!("Filter: {}", draft.label()),
        None => "New filter".into(),
    };
    let hint = if typing.is_some() { "enter keep · esc cancel" } else { "enter change · esc back" };
    let inner = panel(f, 84, EDIT_ROWS.len() as u16 + 6, &title, hint);
    let fields = draft.fields();
    for (k, r) in EDIT_ROWS.iter().enumerate().take(inner.height.saturating_sub(2) as usize) {
        let y = inner.y + k as u16;
        paint_row(f, Rect::new(inner.x - 2, y, inner.width + 4, 1), None, k == row, false);
        let (name, value): (String, String) = match r {
            EditRow::Pattern => ("Pattern".into(), draft.pattern.clone()),
            EditRow::Label => ("Label".into(), draft.label.clone().unwrap_or_else(|| "(the pattern)".into())),
            EditRow::Action => ("Action".into(), draft.action.as_str().into()),
            EditRow::Field(field) => (format!("  {}", field.as_str()), if fields.contains(field) { "✓ looked at".into() } else { "·".into() }),
            EditRow::Sites => ("Sites".into(), if draft.sites.is_empty() { "(any)".into() } else { draft.sites.join(", ") }),
            EditRow::Boards => ("Boards".into(), if draft.boards.is_empty() { "(any)".into() } else { draft.boards.join(", ") }),
            EditRow::Enabled => ("On".into(), if draft.enabled { "yes".into() } else { "no (kept, not applied)".into() }),
        };
        let mut spans = vec![Span::styled(format!("{name:<12}"), dim())];
        match typing.filter(|_| k == row) {
            Some(text) => spans.extend([Span::styled(text.to_string(), bold(t.text)), Span::styled("▏", Style::new().fg(t.primary))]),
            None => spans.push(Span::styled(truncate(&value, (inner.width as usize).saturating_sub(13)), Style::new().fg(t.text))),
        }
        put(f, inner.x, y, inner.width, Line::from(spans));
    }
    // What's wrong with what's being typed, as it's typed; else what it catches.
    let r = EDIT_ROWS.get(row).copied().unwrap_or(EditRow::Pattern);
    let line = match typing.map(|text| crate::app::filters_problem(&crate::app::filters_with_text(draft, r, text))) {
        Some(Some(e)) => Line::styled(e, Style::new().fg(t.error)),
        _ if draft.pattern.is_empty() => Line::styled("Type a pattern (a regex; an MD5 for the md5 field).", dim()),
        _ => Line::styled(format!("Catches {} now. Each change is saved in the config.", counts_text(app.filter_counts(draft))), dim()),
    };
    put(f, inner.x, inner.bottom().saturating_sub(1), inner.width, line);
}

// ----- the image viewer -----
