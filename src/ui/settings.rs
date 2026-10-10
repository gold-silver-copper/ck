//! The settings screen and its popups: themes, colors, keys, filters, your sites.

use super::*;

pub(super) fn draw_settings(f: &mut Frame, app: &mut App, area: Rect) {
    let t = theme();
    let selected = app.list_state(View::Settings).selected().unwrap_or(0);
    let rows = setting_rows();
    // Scroll so the selected setting (and the note under the list, at the end) shows.
    let at = rows.iter().position(|r| *r == Ok(selected)).unwrap_or(0);
    let total = rows.len() + 2;
    let offset = if at + 1 == rows.len() { total } else { at + 2 }.saturating_sub(area.height as usize);
    app.drawn.body = Some(Hit::Settings { area, offset });
    let items: Vec<_> = settings().collect();
    for (row, r) in rows.into_iter().enumerate().skip(offset) {
        let y = area.y + (row - offset) as u16;
        if y >= area.bottom() {
            break;
        }
        match r {
            Err(title) => put(f, area.x, y, area.width, Line::styled(title.to_string(), bold(t.primary))),
            Ok(i) => {
                let Some(item) = items.get(i) else { continue };
                let (label, hint) = (item.label, item.hint);
                paint_row(f, Rect::new(area.x, y, area.width, 1), None, i == selected, false);
                let value = (item.value)(app);
                let hint_w = (area.width as usize).saturating_sub(PAD as usize + 1 + 18 + 34);
                let line = Line::from(vec![
                    Span::styled(pad(label, 18), Style::new().fg(t.text)),
                    Span::styled(pad(&truncate(&value, 32), 34), bold(t.text)),
                    Span::styled(if hint_w >= 16 { truncate(hint, hint_w) } else { String::new() }, dim()),
                ]);
                put(f, area.x + PAD, y, area.width.saturating_sub(PAD + 1), line);
            }
        }
    }
    let y = (area.y as usize + setting_rows().len() + 1).checked_sub(offset).map_or_else(|| area.bottom(), |y| y as u16);
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

pub(super) fn draw_settings_popup(f: &mut Frame, app: &mut App) {
    // Out while it's drawn, so its list can scroll beside the rest of the app.
    let Some(Popup::Settings(mut popup)) = app.popup.take_if(|p| matches!(p, Popup::Settings(_))) else { return };
    match &mut popup {
        SettingsPopup::Themes { list, names, .. } => draw_themes(f, app, list, names),
        SettingsPopup::Colors { list, editing } => draw_colors(f, app, list, editing.as_deref()),
        SettingsPopup::Keys { list, capture } => draw_keys(f, app, list, *capture),
        SettingsPopup::Filters { list, counts } => draw_filter_list(f, app, list, counts),
        SettingsPopup::Sites(m) => draw_my_sites(f, m),
        SettingsPopup::BoardImages { list } => draw_board_images(f, app, list),
        SettingsPopup::HiddenWords { list, typing } => draw_hidden_words(f, app, list, typing.as_deref()),
        SettingsPopup::FilterEdit { index, draft, row, typing } => draw_filter_edit(f, app, *index, draft, *row, typing.as_deref()),
        SettingsPopup::Folder { value } => draw_folder(f, value),
    }
    app.popup = Some(Popup::Settings(popup));
}

fn draw_themes(f: &mut Frame, app: &App, list: &mut ListState, names: &[String]) {
    let t = theme();
    list_panel(f, (52, "Theme", "enter keep · esc cancel"), list, names, ("", true), |_| Vec::new(), |_, name, _| {
        let mut spans = vec![Span::styled(pad(name, 22), Style::new().fg(t.text))];
        // A row of colored cells: the theme at a glance.
        if let Ok(th) = theme::resolve(name, &app.themes) {
            let colors = [th.background, th.surface, th.selection, th.primary, th.primary_container, th.greentext, th.quotelink, th.heading, th.new];
            spans.extend(colors.map(|c| Span::styled("  ", Style::new().bg(c))));
        }
        Line::from(spans)
    });
}

fn draw_colors(f: &mut Frame, app: &App, list: &mut ListState, editing: Option<&str>) {
    let t = theme();
    let title = format!("Colors · {}", app.theme_name);
    let hint = if editing.is_some() { "enter save · esc cancel" } else { "enter edit · x reset · esc close" };
    let sel = list.selected().unwrap_or(0);
    let foot = |_| {
        vec![match editing {
            Some(text) => Line::from(vec![
                Span::styled(format!("New {} color: ", ROLES.get(sel).map_or("", |r| r.0)), Style::new().fg(t.text)),
                Span::styled(text.to_string(), bold(t.text)),
                Span::styled("▏", Style::new().fg(t.primary)),
                Span::styled("   #rrggbb, a name, 0-255, or default", dim()),
            ]),
            None => Line::styled("Changing a color of a built-in theme saves it as a copy: NAME-custom.", dim()),
        }]
    };
    list_panel(f, (96, &title, hint), list, ROLES, ("", true), foot, |_, &(role, desc), _| {
        let c = t.get(role).unwrap_or(Color::Reset);
        Line::from(vec![
            Span::styled("    ", Style::new().bg(c)),
            Span::styled(format!("  {}", pad(role, 22)), Style::new().fg(t.text)),
            Span::styled(pad(&theme::color_string(c), 10), bold(t.text)),
            Span::styled(desc, dim()),
        ])
    });
}

fn draw_keys(f: &mut Frame, app: &App, list: &mut ListState, capture: Option<bool>) {
    let t = theme();
    let rows = key_rows();
    let hint = if capture.is_some() { "press a key · esc cancel" } else { "enter rebind · a add · u unbind · x reset · esc close" };
    let sel = list.selected().unwrap_or(0);
    let foot = |_| {
        vec![match (capture, rows.get(sel).and_then(|r| r.ok())) {
            (Some(add), Some(action)) => {
                let verb = if add { "Press a key to add to" } else { "Press the new key for" };
                Line::from(vec![
                    Span::styled(format!("{verb} "), Style::new().fg(t.text)),
                    Span::styled(action.name(), bold(t.primary)),
                    Span::styled("   esc cancels", dim()),
                ])
            }
            _ => Line::styled("Changed keys are saved in [keys]; navigation keys are fixed. Without a key: in the . menu.", dim()),
        }]
    };
    // Group titles are never selected, so never painted.
    list_panel(f, (96, "Keys", hint), list, &rows, ("", true), foot, |_, &row, width| {
        let action = match row {
            Err(title) => return Line::styled(title.to_string(), bold(t.primary)),
            Ok(action) => action,
        };
        let changed = !app.keys.is_default(action);
        let key_style = if changed { bold(t.primary) } else { bold(t.text) };
        let scopes = action.scopes().iter().map(|s| s.label()).collect::<Vec<_>>().join(", ");
        let label = app.keys.label(action);
        let mut spans = match label.as_str() {
            "" => vec![Span::styled(format!("  {}", pad("menu", 16)), dim())],
            _ => vec![Span::styled(format!("  {}", col(&label, 16)), key_style)],
        };
        // Narrow: just the key and what it does.
        if width >= 80 {
            spans.push(Span::styled(pad(action.name(), 17), dim()));
            spans.push(Span::styled(col(action.what(), 37), Style::new().fg(t.text)));
            spans.push(Span::styled(truncate(&scopes, (width as usize).saturating_sub(72)), dim()));
        } else {
            spans.push(Span::styled(truncate(action.what(), (width as usize).saturating_sub(18)), Style::new().fg(t.text)));
        }
        Line::from(spans)
    });
}

fn draw_folder(f: &mut Frame, value: &str) {
    let t = theme();
    let inner = panel(f, 90, 7, "Download folder", "enter save · esc cancel");
    let field = Rect::new(inner.x, inner.y, inner.width, 1);
    fill(f, field, t.surface);
    let line = Line::from(vec![Span::styled(value.to_string(), Style::new().fg(t.text)), Span::styled("▏", Style::new().fg(t.primary))]);
    put(f, inner.x + 1, inner.y, inner.width.saturating_sub(2), line);
    let help = "{site}, {board}, {thread} and {downloads} are filled in. Empty for the default.";
    put(f, inner.x, inner.y + 2, inner.width, Line::styled(help, dim()));
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
fn draw_hidden_words(f: &mut Frame, app: &App, list: &mut ListState, typing: Option<&str>) {
    let t = theme();
    let hint = if typing.is_some() { "enter add · esc cancel" } else { "a add · x remove · esc close" };
    let empty = if typing.is_some() { "" } else { "None yet. a adds one; so does w in a post's X." };
    let typed = typing.map(|text| Line::from(vec![Span::styled("New  ", dim()), Span::styled(text.to_string(), bold(t.text)), Span::styled("▏", Style::new().fg(t.primary))]));
    let foot = |_| vec![typed.unwrap_or_default(), Line::styled("Whole words, any case. Kept in the config as hidden_words.", dim())];
    list_panel(f, (70, "Hidden words", hint), list, &app.hidden_words, (empty, typing.is_none()), foot, |_, word, _| Line::styled(word.clone(), Style::new().fg(t.text)));
}

/// Settings › Catalog › Board images: boards with their own image setting.
fn draw_board_images(f: &mut Frame, app: &App, list: &mut ListState) {
    let t = theme();
    let boards = app.boards_with_images_set();
    let empty = "None: every board follows the default. A board's . menu changes it.";
    let note = "Boards the site marks NSFW follow the NSFW boards setting; the rest show images.";
    list_panel(f, (70, "Board images", "x back to the default · esc close"), list, &boards, (empty, true), |_| vec![Line::styled(note, dim())], |_, (key, on), width| {
        spread(vec![Span::styled(key.clone(), Style::new().fg(t.text))], vec![Span::styled(if *on { "images on" } else { "images off" }, dim())], width as usize)
    });
}

/// Settings › Sites › Your sites: the config's `[[site]]` tables.
fn draw_my_sites(f: &mut Frame, m: &mut crate::app::MySites) {
    let t = theme();
    let empty = "None yet: only the built-in sites. a adds one from a link to it.";
    let note = "Kept in the config as [[site]] tables; the built-in sites are always there too.";
    let hint = "a add · r update a vichan site's boards · x remove · esc close";
    list_panel(f, (100, "Your sites", hint), &mut m.list, &m.sites, (empty, true), |_| vec![Line::default(), Line::styled(note, dim())], |k, s, width| {
        let left = vec![
            Span::styled(col(&s.name, 18), bold(t.text)),
            Span::styled(pad(s.kind.as_str(), 11), Style::new().fg(t.text)),
            Span::styled(truncate(s.url.as_deref().unwrap_or(""), (width as usize).saturating_sub(52)), dim()),
        ];
        let tag = if m.armed == Some(k) { "x again removes it".to_string() } else { crate::app::site_origin(s).to_string() };
        spread(left, vec![Span::styled(tag, dim())], width as usize)
    });
}

fn draw_filter_list(f: &mut Frame, app: &App, list: &mut ListState, counts: &[(usize, usize)]) {
    let t = theme();
    let empty = format!("No filters yet. a adds one; {} on a post makes one like it.", app.keys.key(Action::Filter));
    let note = "Counts are for the open catalog and thread. Kept in the config as [[filter]] tables.";
    let hint = "enter edit · space on/off · a add · x remove · esc close";
    list_panel(f, (110, "Filters", hint), list, &app.filter_cfgs, (&empty, true), |_| vec![Line::default(), Line::styled(note, dim())], |k, c, width| {
        let wide = width >= 90;
        let style = if c.enabled { Style::new().fg(t.text) } else { dim() };
        let fields = c.fields().iter().map(|f| f.as_str()).collect::<Vec<_>>().join("+");
        let count = counts.get(k).map_or(String::new(), |&n| counts_text(n));
        // Narrow: the label and fields share what the count leaves.
        let room = (width as usize).saturating_sub(markup::columns(&count) + 15);
        let (label_w, fields_w) = if wide { (22, 18) } else { (room * 3 / 5, room * 2 / 5) };
        let mut left = vec![
            if c.enabled { chip(pad(c.action.as_str(), 9), t.on_primary_container, t.primary_container) } else { chip(pad("off", 9), t.text_dim, t.surface_high) },
            Span::raw("  "),
            Span::styled(col(c.label(), label_w), if c.enabled { bold(t.text) } else { dim() }),
            Span::styled(col(&fields, fields_w), style),
        ];
        if wide {
            left.push(Span::styled(col(&filter_scope(c), 20), style));
            // The pattern gets what's left, beside the count.
            let used = markup::spans_columns(&left) + markup::columns(&count) + 2;
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
    let fields = draft.fields();
    // What's wrong with what's being typed, as it's typed; else what it catches.
    let r = EDIT_ROWS.get(row).copied().unwrap_or(EditRow::Pattern);
    let note = match typing.map(|text| crate::app::filters_problem(&crate::app::filters_with_text(draft, r, text))) {
        Some(Some(e)) => Line::styled(e, Style::new().fg(t.error)),
        _ if draft.pattern.is_empty() => Line::styled("Type a pattern (a regex; an MD5 for md5, a range like >2MB for filesize).", dim()),
        _ => Line::styled(format!("Catches {} now. Each change is saved in the config.", counts_text(app.filter_counts(draft))), dim()),
    };
    // On a short screen the rows scroll, the selected one in view.
    let mut list = ListState::default().with_selected(Some(row));
    list_panel(f, (84, &title, hint), &mut list, &EDIT_ROWS, ("", true), |_| vec![Line::default(), note], |k, r, width| {
        let (name, value): (String, String) = match r {
            EditRow::Pattern => ("Pattern".into(), draft.pattern.clone()),
            EditRow::Label => ("Label".into(), draft.label.clone().unwrap_or_else(|| "(the pattern)".into())),
            EditRow::Action => ("Action".into(), draft.action.as_str().into()),
            EditRow::Recursive => (
                "Replies".into(),
                match (draft.recursive, draft.action) {
                    (true, crate::filter::FilterAction::Hide) => "hidden too (and theirs, in threads)".into(),
                    (true, _) => "hidden too, when it hides".into(),
                    (false, _) => "as they are".into(),
                },
            ),
            EditRow::Field(field) => (format!("  {}", field.as_str()), if fields.contains(field) { "✓ looked at".into() } else { "·".into() }),
            EditRow::Sites => ("Sites".into(), if draft.sites.is_empty() { "(any)".into() } else { draft.sites.join(", ") }),
            EditRow::Boards => ("Boards".into(), if draft.boards.is_empty() { "(any)".into() } else { draft.boards.join(", ") }),
            EditRow::Enabled => ("On".into(), if draft.enabled { "yes".into() } else { "no (kept, not applied)".into() }),
            EditRow::Posts => (
                "Posts".into(),
                match (draft.op, draft.reply) {
                    (true, _) => "OPs only".into(),
                    (_, true) => "replies only".into(),
                    _ => "all".into(),
                },
            ),
            EditRow::Notify => ("Notify".into(), if draft.notify { "yes, in watched threads and followed boards".into() } else { "no".into() }),
            EditRow::Top => (
                "Top".into(),
                match (draft.top, draft.action) {
                    (true, crate::filter::FilterAction::Highlight) => "first in catalogs".into(),
                    (true, _) => "first in catalogs, when it highlights".into(),
                    (false, _) => "no".into(),
                },
            ),
        };
        let mut spans = vec![Span::styled(pad(&name, 13), dim())];
        match typing.filter(|_| k == row) {
            Some(text) => spans.extend([Span::styled(text.to_string(), bold(t.text)), Span::styled("▏", Style::new().fg(t.primary))]),
            None => spans.push(Span::styled(truncate(&value, (width as usize).saturating_sub(14)), Style::new().fg(t.text))),
        }
        Line::from(spans)
    });
}

// ----- the image viewer -----
