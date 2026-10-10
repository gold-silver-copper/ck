//! The settings screen: theme, colors and other options, saved to the config file (comments
//! kept; created from the default config if there isn't one yet).

use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;

use super::{App, Popup, View, edit_text, list_move};
use crate::config::{self, ColorMode, ImagesMode};
use crate::keys::{ACTIONS, Key, Scope};
use crate::theme::{self, ROLES, Theme, ThemeDef};

/// One row of the settings screen: its label and hint, how its value reads, and what
/// `enter` does.
pub struct Setting {
    pub label: &'static str,
    pub hint: &'static str,
    pub value: fn(&App) -> String,
    pub activate: fn(&mut App),
}

const fn row(label: &'static str, hint: &'static str, value: fn(&App) -> String, activate: fn(&mut App)) -> Setting {
    Setting { label, hint, value, activate }
}

fn open(app: &mut App, popup: SettingsPopup) {
    app.popup = Some(Popup::Settings(popup));
}

/// `n` things: "none", "1 board", "3 boards".
fn count(n: usize, one: &str) -> String {
    match n {
        0 => "none".into(),
        1 => format!("1 {one}"),
        n => format!("{n} {one}s"),
    }
}

/// The settings, by section, in display order.
pub const SECTIONS: &[(&str, &[Setting])] = &[
    ("Appearance", &[
        row("Theme", "Live preview while choosing", |a| a.theme_name.clone(), App::choose_theme_popup),
        row(
            "Colors",
            "Change any color of the current theme",
            |a| match a.themes.get(&a.theme_name) {
                Some(def) if !def.colors.is_empty() => format!("{} changed", def.colors.len()),
                _ => "as the theme has them".into(),
            },
            |a| open(a, SettingsPopup::Colors { list: ListState::default().with_selected(Some(0)), editing: None }),
        ),
        row(
            "Color depth",
            "24-bit color, or the nearest of 256",
            |a| match a.color_mode {
                ColorMode::Auto => format!("auto ({})", if a.truecolor { "24-bit" } else { "256" }),
                m => m.as_str().into(),
            },
            |a| {
                a.color_mode = a.color_mode.next();
                a.truecolor = a.color_mode.truecolor();
                let mode = a.color_mode.as_str();
                a.save_config(&format!("color depth {mode}"), |d| d["color"] = toml_edit::value(mode));
            },
        ),
        row(
            "Reading position",
            "How far from the edges the selected post stays",
            |a| match a.scroll_margin {
                m if m <= 0.0 => "at the edge (no margin)".into(),
                m if m >= 0.5 => "centered".into(),
                m => format!("{}% from the edges", (m * 100.0).round()),
            },
            App::next_scroll_margin,
        ),
        row(
            "Terminal title",
            "Where you are and how many posts are new, in the window's title",
            |a| if a.set_title { "set by ck" } else { "left alone" }.into(),
            |a| {
                a.set_title = !a.set_title;
                let on = a.set_title;
                a.save_config(if on { "setting the terminal's title" } else { "leaving the terminal's title alone" }, |d| d["set_title"] = toml_edit::value(on));
            },
        ),
    ]),
    ("Catalog", &[
        row("Default layout", "c in a catalog sets a board's own", |a| a.default_layout.as_str().into(), App::cycle_default_layout),
        row(
            "Preview lines",
            "How much of each thread a card shows (its thumbnail grows with it)",
            |a| a.catalog_lines.map_or_else(|| "as fit beside the thumbnail".into(), |n| format!("{n} line{}", if n == 1 { "" } else { "s" })),
            App::next_catalog_lines,
        ),
        row(
            "Watched first",
            "Threads you watch at the top of catalogs",
            |a| if a.watched_first { "watched threads first" } else { "in the sort's order" }.into(),
            |a| {
                a.watched_first = !a.watched_first;
                let on = a.watched_first;
                a.save_config(if on { "watched threads first in catalogs" } else { "watched threads in the sort's order" }, |d| d["watched_first"] = toml_edit::value(on));
            },
        ),
        row(
            "Images",
            "Thumbnails and the image viewer (after a restart)",
            |a| match a.images_mode {
                ImagesMode::Auto if a.images.enabled() => format!("on ({})", a.images.protocol_name()),
                ImagesMode::Auto => "on".into(),
                m => m.as_str().into(),
            },
            |a| {
                a.images_mode = a.images_mode.next();
                let mode = a.images_mode.as_str();
                a.save_config("images (from the next start)", |d| d["images"] = toml_edit::value(mode));
            },
        ),
        row(
            "NSFW boards",
            "Images on boards the site marks NSFW",
            |a| if a.nsfw_images == crate::config::NsfwImages::Show { "images shown" } else { "images off" }.into(),
            |a| {
                a.nsfw_images = a.nsfw_images.next();
                let mode = a.nsfw_images.as_str();
                a.save_config(&format!("nsfw_images = \"{mode}\""), |d| d["nsfw_images"] = toml_edit::value(mode));
            },
        ),
        row(
            "Board images",
            "Boards with their own image setting (. menu on a board)",
            |a| count(a.boards_with_images_set().len(), "board"),
            |a| open(a, SettingsPopup::BoardImages { list: ListState::default().with_selected(Some(0)) }),
        ),
        row(
            "Filters",
            "Hide or highlight by pattern; X adds one from a post",
            |a| {
                let hidden = a.store.hidden_count();
                let off = a.filter_cfgs.iter().filter(|f| !f.enabled).count();
                let off = if off > 0 { format!(" ({off} off)") } else { String::new() };
                let n = a.filter_cfgs.len();
                format!("{n} filter{}{off}, {hidden} hidden by hand", if n == 1 { "" } else { "s" })
            },
            |a| open(a, a.filter_list(0)),
        ),
        row(
            "Hidden words",
            "Posts with any of these words are hidden, everywhere",
            |a| count(a.hidden_words.len(), "word"),
            |a| open(a, SettingsPopup::HiddenWords { list: ListState::default().with_selected(Some(0)), typing: None }),
        ),
        row(
            "Hidden replies",
            "In a thread, replies to a hidden post are hidden too",
            |a| if a.hiding.recursive() { "hidden with it" } else { "shown" }.into(),
            |a| {
                let on = !a.hiding.recursive();
                a.rehide(|a| a.hiding.set_recursive(on));
                a.save_config(if on { "hiding replies to hidden posts" } else { "showing replies to hidden posts" }, |d| d["recursive_hiding"] = toml_edit::value(on));
            },
        ),
    ]),
    ("Background refresh", &[
        row(
            "Open thread",
            "How often the open thread updates",
            |a| format!("every {}s", a.refresh_thread.as_secs()),
            |a| {
                let secs = next(REFRESH_THREAD, a.refresh_thread.as_secs());
                a.refresh_thread = Duration::from_secs(secs);
                a.save_config(&format!("refresh every {secs}s"), |d| d["refresh_thread_secs"] = toml_edit::value(secs as i64));
            },
        ),
        row(
            "Watched threads",
            "How often each watched thread updates",
            |a| format!("every {}s", a.refresh_watched.as_secs()),
            |a| {
                let secs = next(REFRESH_WATCHED, a.refresh_watched.as_secs());
                a.refresh_watched = Duration::from_secs(secs);
                a.save_config(&format!("refresh every {secs}s"), |d| d["refresh_watched_secs"] = toml_edit::value(secs as i64));
            },
        ),
        row(
            "Quiet threads",
            "Slower while nothing new comes, to 10×",
            |a| if a.refresh_backoff { "refreshed less often" } else { "at the same interval" }.into(),
            |a| {
                a.refresh_backoff = !a.refresh_backoff;
                let on = a.refresh_backoff;
                a.save_config(if on { "slowing down on quiet threads" } else { "refreshing quiet threads as often" }, |d| d["refresh_backoff"] = toml_edit::value(on));
            },
        ),
        row("Notifications", "New posts in watched threads, replies to yours (m)", App::notify_value, |a| {
            a.notify_mode = a.notify_mode.next();
            let mode = a.notify_mode.as_str();
            a.save_config(&format!("notifications {mode}"), |d| d["notify"] = toml_edit::value(mode));
        }),
        row(
            "Reading the end",
            "New posts come into view while you read the last one",
            |a| if a.follow_new_posts { "new posts come into view" } else { "nothing moves" }.into(),
            |a| {
                a.follow_new_posts = !a.follow_new_posts;
                let on = a.follow_new_posts;
                a.save_config(if on { "following new posts" } else { "not following new posts" }, |d| d["follow_new_posts"] = toml_edit::value(on));
            },
        ),
    ]),
    ("Sites", &[
        row(
            "Add a site",
            "Paste a link to any page of it; ck finds out what it runs",
            |a| format!("{} sites so far", a.sites.len() - a.removed_sites.len()),
            |a| a.popup = Some(Popup::Adding(super::Adding::Typing(String::new()))),
        ),
        row("Your sites", "The ones you added, or built-in ones you changed", App::my_sites_value, |a| {
            a.popup = a.my_sites().map(|m| Popup::Settings(SettingsPopup::Sites(m)))
        }),
    ]),
    ("Downloads", &[row(
        "Folder",
        "Where saved files and pages go",
        |a| a.download_dir.clone().unwrap_or_else(|| "~/Downloads/ck/{site}/{board}/{thread}".into()),
        |a| open(a, SettingsPopup::Folder { value: a.download_dir.clone().unwrap_or_default() }),
    )]),
    ("Startup", &[row(
        "Last place",
        "Start where you left off (ck URL starts elsewhere)",
        |a| if a.restore_session { "restored" } else { "not restored" }.into(),
        |a| {
            a.restore_session = !a.restore_session;
            let on = a.restore_session;
            a.save_config(if on { "restoring the last place" } else { "not restoring the last place" }, |d| d["restore_session"] = toml_edit::value(on));
        },
    )]),
    ("Keys", &[row(
        "Key bindings",
        "Rebind any command",
        |a| match ACTIONS.iter().filter(|e| !a.keys.is_default(e.0)).count() {
            0 => "defaults".into(),
            n => format!("{n} changed"),
        },
        |a| open(a, SettingsPopup::Keys { list: ListState::default().with_selected(key_rows().iter().position(Result::is_ok)), capture: None }),
    )]),
];

const REFRESH_THREAD: &[u64] = &[10, 15, 30, 60, 120];
/// The scroll margins Settings cycles through (a fraction of the screen).
const SCROLL_MARGINS: &[f32] = &[0.0, 0.2, 0.3, 0.5];
const REFRESH_WATCHED: &[u64] = &[60, 120, 300, 600, 1800];

/// Every setting, in display order.
pub fn settings() -> impl Iterator<Item = &'static Setting> {
    SECTIONS.iter().flat_map(|(_, items)| items.iter())
}

/// Where the setting with this label is (tests find rows by it).
#[cfg(test)]
pub fn position(label: &str) -> Option<usize> {
    settings().position(|s| s.label == label)
}

/// The screen's rows, top to bottom: `Err(section title)` for headers, `Ok(index)` for
/// settings, with a blank row between sections. Shared by drawing and mouse clicks.
pub fn rows() -> Vec<Result<usize, &'static str>> {
    let mut out = Vec::new();
    let mut i = 0;
    for (k, (title, items)) in SECTIONS.iter().enumerate() {
        if k > 0 {
            out.push(Err(""));
        }
        out.push(Err(*title));
        for _ in *items {
            out.push(Ok(i));
            i += 1;
        }
    }
    out
}

pub enum SettingsPopup {
    /// Choosing a theme; moving previews it, `before` is restored on esc.
    Themes { list: ListState, names: Vec<String>, before: (String, Theme) },
    /// The current theme's colors; `editing` holds a color being typed.
    Colors { list: ListState, editing: Option<String> },
    /// Typing the download folder.
    Folder { value: String },
    /// The key editor, over `key_rows()`; `capture` is waiting for a key to bind
    /// (`Some(true)`: add it to the action's keys).
    Keys { list: ListState, capture: Option<bool> },
    /// The config's `[[site]]` tables.
    Sites(super::MySites),
    /// Boards with their own image setting.
    BoardImages { list: ListState },
    /// `hidden_words`; `typing` holds one being added.
    HiddenWords { list: ListState, typing: Option<String> },
    /// The `[[filter]]` list, with how many posts and threads each catches on screen now.
    Filters { list: ListState, counts: Vec<(usize, usize)> },
    /// One filter being edited (`index`: none for a new one), on row `row` of `EDIT_ROWS`;
    /// `typing` holds a text row being typed.
    FilterEdit { index: Option<usize>, draft: crate::filter::FilterConfig, row: usize, typing: Option<String> },
}

/// The key editor's rows: `Err(title)` for groups (by an action's first scope),
/// `Ok(index into ACTIONS)` for actions.
pub fn key_rows() -> Vec<Result<usize, &'static str>> {
    let groups = [
        (Scope::Global, "Everywhere"),
        (Scope::Lists, "Lists"),
        (Scope::Catalog, "Catalog"),
        (Scope::Thread, "Thread"),
        (Scope::Saved, "Watched and History"),
        (Scope::Viewer, "Image viewer"),
    ];
    let mut out = Vec::new();
    for (scope, title) in groups {
        let actions: Vec<usize> = ACTIONS.iter().enumerate().filter(|(_, a)| a.3.first() == Some(&scope)).map(|(i, _)| i).collect();
        if actions.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(Err(""));
        }
        out.push(Err(title));
        out.extend(actions.into_iter().map(Ok));
    }
    out
}

impl App {
    /// Enter on a setting.
    pub fn activate_setting(&mut self) {
        if let Some(s) = self.selected_row(View::Settings).and_then(|i| settings().nth(i)) {
            (s.activate)(self);
        }
    }

    fn choose_theme_popup(&mut self) {
        let names = theme::names(&self.themes);
        let list = ListState::default().with_selected(Some(names.iter().position(|n| *n == self.theme_name).unwrap_or(0)));
        let before = (self.theme_name.clone(), theme::theme());
        open(self, SettingsPopup::Themes { list, names, before });
    }

    fn next_catalog_lines(&mut self) {
        const LINES: [Option<u16>; 6] = [None, Some(1), Some(2), Some(3), Some(5), Some(8)];
        let at = LINES.iter().position(|&l| l == self.catalog_lines).unwrap_or(0);
        self.catalog_lines = LINES.get(at + 1).copied().flatten();
        match self.catalog_lines {
            Some(n) => self.save_config(&format!("{n} line{} in catalog cards", if n == 1 { "" } else { "s" }), |d| d["catalog_lines"] = toml_edit::value(i64::from(n))),
            None => self.save_config("catalog cards as tall as their thumbnails", |d| {
                d.remove("catalog_lines");
            }),
        }
    }

    fn next_scroll_margin(&mut self) {
        let next = SCROLL_MARGINS.iter().copied().find(|&m| m > self.scroll_margin + 0.01).unwrap_or(0.0);
        self.scroll_margin = next;
        // Every tab's thread reads with it from now on.
        for t in std::iter::once(&mut self.tab).chain(self.tabs.iter_mut()).filter_map(|t| t.thread.as_mut()) {
            t.margin = next;
        }
        self.save_config(&format!("scroll_margin = {next}"), |d| d["scroll_margin"] = toml_edit::value(f64::from(next)));
    }

    fn notify_value(&self) -> String {
        let method = crate::notify::method(self.notify_mode, self.notify_command.as_deref(), &|k| std::env::var(k).ok());
        let how = match method {
            crate::notify::Method::Osc9 | crate::notify::Method::Osc777 => "desktop notification",
            crate::notify::Method::Bell => "terminal bell",
            crate::notify::Method::Command(_) => "notify_command",
            crate::notify::Method::Off => "off",
        };
        match self.notify_mode {
            crate::notify::NotifyMode::Off => "off".into(),
            m => format!("{} ({how})", m.as_str()),
        }
    }

    fn my_sites_value(&self) -> String {
        let builtin = config::builtin_sites();
        let added = self.sites.iter().filter(|s| !builtin.iter().any(|b| b.name.eq_ignore_ascii_case(&s.cfg.name))).count();
        let changed = self.sites.iter().filter(|s| builtin.iter().any(|b| b.name.eq_ignore_ascii_case(&s.cfg.name) && *b != s.cfg)).count();
        match (added, changed) {
            (0, 0) => "only the built-in ones".into(),
            (a, 0) => format!("{a} added"),
            (0, c) => format!("{c} built-in changed"),
            (a, c) => format!("{a} added, {c} built-in changed"),
        }
    }

    /// Keys while a settings popup is open.
    pub fn on_settings_popup_key(&mut self, key: KeyEvent) {
        let Some(popup) = take_popup!(self, Settings) else { return };
        let next = match popup {
            SettingsPopup::Themes { list, names, before } => self.on_themes_key(key, list, names, before),
            SettingsPopup::Colors { list, editing } => self.on_colors_key(key, list, editing),
            SettingsPopup::Keys { list, capture } => self.on_keys_key(key, list, capture),
            SettingsPopup::Filters { list, counts } => self.on_filter_list_key(key, list, counts),
            SettingsPopup::Sites(m) => self.on_my_sites_key(key, m).map(SettingsPopup::Sites),
            SettingsPopup::HiddenWords { list, typing } => self.on_hidden_words_key(key, list, typing),
            SettingsPopup::BoardImages { list } => self.on_board_images_key(key, list),
            SettingsPopup::FilterEdit { index, draft, row, typing } => self.on_filter_edit_key(key, index, draft, row, typing),
            SettingsPopup::Folder { value } => self.on_folder_key(key, value),
        };
        if let Some(p) = next {
            self.popup = Some(Popup::Settings(p));
        }
    }

    fn on_themes_key(&mut self, key: KeyEvent, mut list: ListState, names: Vec<String>, before: (String, Theme)) -> Option<SettingsPopup> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.theme_name = before.0;
                self.set_theme(before.1);
                None
            }
            KeyCode::Enter => {
                let name = list.selected().and_then(|i| names.get(i)).cloned().unwrap_or(before.0);
                self.choose_theme(&name);
                None
            }
            code => {
                let cur = list.selected().unwrap_or(0);
                let to = list_move(code, cur, names.len()).unwrap_or(cur);
                list.select(Some(to));
                // Live preview.
                if let Some(Ok(t)) = names.get(to).map(|name| theme::resolve(name, &self.themes)) {
                    self.set_theme(t);
                }
                Some(SettingsPopup::Themes { list, names, before })
            }
        }
    }

    fn on_colors_key(&mut self, key: KeyEvent, mut list: ListState, editing: Option<String>) -> Option<SettingsPopup> {
        let cur = list.selected().unwrap_or(0);
        // The rows are ROLES, so the selected one is always there.
        let &(role, _) = ROLES.get(cur)?;
        if let Some(mut text) = editing {
            return match key.code {
                KeyCode::Esc => Some(SettingsPopup::Colors { list, editing: None }),
                KeyCode::Enter => {
                    match theme::parse_color(&text) {
                        Ok(c) => {
                            self.set_role_color(role, Some(&theme::color_string(c)));
                            Some(SettingsPopup::Colors { list, editing: None })
                        }
                        Err(e) => {
                            self.error(e);
                            Some(SettingsPopup::Colors { list, editing: Some(text) })
                        }
                    }
                }
                code => {
                    edit_text(&mut text, code);
                    Some(SettingsPopup::Colors { list, editing: Some(text) })
                }
            };
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left => None,
            // The field starts empty; the current color is on the row above it.
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => Some(SettingsPopup::Colors { list, editing: Some(String::new()) }),
            // Back to what the base theme has.
            KeyCode::Char('x') | KeyCode::Delete => {
                self.set_role_color(role, None);
                Some(SettingsPopup::Colors { list, editing: None })
            }
            code => {
                list.select(Some(list_move(code, cur, ROLES.len()).unwrap_or(cur)));
                Some(SettingsPopup::Colors { list, editing: None })
            }
        }
    }

    fn on_keys_key(&mut self, key: KeyEvent, mut list: ListState, capture: Option<bool>) -> Option<SettingsPopup> {
        if let Some(add) = capture {
            if key.code != KeyCode::Esc {
                self.bind_key(&list, Some((Key::from_event(&key), add)));
            }
            return Some(SettingsPopup::Keys { list, capture: None });
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left => None,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => Some(SettingsPopup::Keys { list, capture: Some(false) }),
            KeyCode::Char('a') => Some(SettingsPopup::Keys { list, capture: Some(true) }),
            // No key: the action is left to the menu.
            KeyCode::Char('u') => {
                self.unbind_key(&list);
                Some(SettingsPopup::Keys { list, capture: None })
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                self.bind_key(&list, None);
                Some(SettingsPopup::Keys { list, capture: None })
            }
            code => {
                let rows = key_rows();
                let actions: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| r.is_ok()).map(|(i, _)| i).collect();
                let cur = actions.iter().position(|&r| Some(r) == list.selected()).unwrap_or(0);
                let to = match code {
                    KeyCode::PageDown => (cur + 10).min(actions.len().saturating_sub(1)),
                    KeyCode::PageUp => cur.saturating_sub(10),
                    _ => list_move(code, cur, actions.len()).unwrap_or(cur),
                };
                list.select(actions.get(to).copied());
                Some(SettingsPopup::Keys { list, capture: None })
            }
        }
    }

    fn on_hidden_words_key(&mut self, key: KeyEvent, mut list: ListState, typing: Option<String>) -> Option<SettingsPopup> {
        if let Some(mut text) = typing {
            return match key.code {
                KeyCode::Esc => Some(SettingsPopup::HiddenWords { list, typing: None }),
                KeyCode::Enter => {
                    self.add_hidden_word(&text);
                    let last = self.hidden_words.len().saturating_sub(1);
                    Some(SettingsPopup::HiddenWords { list: ListState::default().with_selected(Some(last)), typing: None })
                }
                code => {
                    edit_text(&mut text, code);
                    Some(SettingsPopup::HiddenWords { list, typing: Some(text) })
                }
            };
        }
        let cur = list.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left => None,
            KeyCode::Char('a') => Some(SettingsPopup::HiddenWords { list, typing: Some(String::new()) }),
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some(w) = self.hidden_words.get(cur).cloned() {
                    self.remove_hidden_word(&w, true);
                }
                list.select(Some(cur.min(self.hidden_words.len().saturating_sub(1))));
                Some(SettingsPopup::HiddenWords { list, typing: None })
            }
            code => {
                list.select(Some(list_move(code, cur, self.hidden_words.len()).unwrap_or(cur)));
                Some(SettingsPopup::HiddenWords { list, typing: None })
            }
        }
    }

    fn on_board_images_key(&mut self, key: KeyEvent, mut list: ListState) -> Option<SettingsPopup> {
        let boards = self.boards_with_images_set();
        let cur = list.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left => None,
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some((k, _)) = boards.get(cur) {
                    self.reset_board_images(&k.clone());
                }
                list.select(Some(cur.min(boards.len().saturating_sub(2))));
                Some(SettingsPopup::BoardImages { list })
            }
            code => {
                list.select(Some(list_move(code, cur, boards.len()).unwrap_or(cur)));
                Some(SettingsPopup::BoardImages { list })
            }
        }
    }

    fn on_folder_key(&mut self, key: KeyEvent, mut value: String) -> Option<SettingsPopup> {
        match key.code {
            KeyCode::Esc => None,
            KeyCode::Enter => {
                let dir = Some(value.trim().to_string()).filter(|v| !v.is_empty());
                self.download_dir.clone_from(&dir);
                self.save_config("the download folder", |d| match &dir {
                    Some(v) => d["download_dir"] = toml_edit::value(v.as_str()),
                    None => {
                        d.remove("download_dir");
                    }
                });
                None
            }
            code => {
                edit_text(&mut value, code);
                Some(SettingsPopup::Folder { value })
            }
        }
    }

    /// Bind a key to the action selected in the key editor (replacing its keys, or added to
    /// them), or with `None` reset it to the default; refused if it would clash.
    fn bind_key(&mut self, list: &ListState, key: Option<(Key, bool)>) {
        let Some(&(action, name, ..)) = list.selected().and_then(|r| key_rows().get(r).copied()).and_then(Result::ok).and_then(|i| ACTIONS.get(i)) else { return };
        let keys = key.map(|(k, add)| {
            let mut keys = if add { self.keys.keys(action).to_vec() } else { Vec::new() };
            if !keys.contains(&k) {
                keys.push(k);
            }
            keys
        });
        match self.keys.with(action, keys) {
            Ok(map) => {
                self.keys = map;
                let binding = self.keys.binding(action);
                let what = format!("{name} = {}", self.keys.label(action));
                self.save_config(&what, |d| config::set_key(d, name, binding.as_ref()));
            }
            Err(e) => self.error(e),
        }
    }

    /// Take every key away from the action selected in the key editor.
    fn unbind_key(&mut self, list: &ListState) {
        let Some(&(action, name, ..)) = list.selected().and_then(|r| key_rows().get(r).copied()).and_then(Result::ok).and_then(|i| ACTIONS.get(i)) else { return };
        if self.keys.keys(action).is_empty() {
            return self.info(format!("{name} has no key (it's in the menu)"));
        }
        if let Ok(map) = self.keys.with(action, Some(Vec::new())) {
            self.keys = map;
            let binding = self.keys.binding(action);
            self.save_config(&format!("{name} = [] (in the menu)"), |d| config::set_key(d, name, binding.as_ref()));
        }
    }

    /// Switch the look now; cached thread layouts carry colors, so they're redone.
    pub fn set_theme(&mut self, t: Theme) {
        theme::set(t);
        self.invalidate_layouts();
    }

    fn choose_theme(&mut self, name: &str) {
        match theme::resolve(name, &self.themes) {
            Ok(t) => {
                self.theme_name = name.to_string();
                self.set_theme(t);
                self.save_config(&format!("theme {name}"), |d| config::set_theme(d, name));
            }
            Err(e) => self.error(e),
        }
    }

    /// Change (or with `None`, reset) one color of the current theme. A built-in theme is
    /// first copied to `NAME-custom`, which becomes the current theme.
    fn set_role_color(&mut self, role: &str, color: Option<&str>) {
        let builtin = !self.themes.contains_key(&self.theme_name);
        let (name, base) = if builtin {
            (format!("{}-custom", self.theme_name), Some(self.theme_name.clone()))
        } else {
            (self.theme_name.clone(), None)
        };
        if builtin && color.is_none() {
            return; // nothing to reset
        }
        let def = self.themes.entry(name.clone()).or_insert_with(|| ThemeDef { base: base.clone(), ..Default::default() });
        match &color {
            Some(c) => {
                def.colors.insert(role.to_string(), c.to_string());
            }
            None => {
                def.colors.remove(role);
            }
        }
        match theme::resolve(&name, &self.themes) {
            Ok(t) => {
                self.theme_name.clone_from(&name);
                self.set_theme(t);
                let what = match &color {
                    Some(c) => format!("{role} = {c} in theme {name}"),
                    None => format!("{role} reset in theme {name}"),
                };
                self.save_config(&what, |d| {
                    config::set_theme(d, &name);
                    config::set_theme_color(d, &name, base.as_deref(), role, color);
                });
            }
            Err(e) => self.error(e),
        }
    }

    /// Write a change to the config file and say where it went.
    pub fn save_config(&mut self, what: &str, f: impl FnOnce(&mut toml_edit::DocumentMut)) {
        self.save_config_or("Changed", what, |d| {
            f(d);
            Ok(())
        });
    }

    /// `save_config` with an edit that can refuse; whether it was saved. If it wasn't, `done`
    /// is what holds for now ("Changed", "Added").
    pub(super) fn save_config_or(&mut self, done: &str, what: &str, f: impl FnOnce(&mut toml_edit::DocumentMut) -> anyhow::Result<()>) -> bool {
        self.say_saved(&format!("{done} {what}"), self.edit_config(f).map(|p| format!("Saved {what} in {p}")))
    }

    /// Edit the config file, keeping its comments (an edit that refuses changes nothing);
    /// returns its path, for messages.
    pub fn edit_config(&self, f: impl FnOnce(&mut toml_edit::DocumentMut) -> anyhow::Result<()>) -> anyhow::Result<String> {
        let path = self.config_path.as_ref().ok_or_else(|| anyhow::anyhow!("no home directory to keep a config file in"))?;
        config::try_edit_at(path, f)?;
        Ok(tilde(&path.display().to_string()))
    }
}

/// The next value in a cycle of choices.
fn next(choices: &[u64], current: u64) -> u64 {
    choices.iter().copied().find(|&c| c > current).or_else(|| choices.first().copied()).unwrap_or(current)
}

/// A path with the home directory shown as `~`.
pub fn tilde(path: &str) -> String {
    let home = dirs::home_dir().map(|h| h.display().to_string()).unwrap_or_default();
    match path.strip_prefix(&home).filter(|_| !home.is_empty()) {
        Some(rest) => format!("~{rest}"),
        None => path.to_string(),
    }
}
