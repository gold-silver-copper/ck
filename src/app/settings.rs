//! The settings screen: theme, colors and other options, saved to the config file (comments
//! kept; created from the default config if there isn't one yet).

use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;

use super::{App, View, edit_text, list_move};
use crate::config::{self, ColorMode, ImagesMode};
use crate::keys::{ACTIONS, Key, Scope};
use crate::theme::{self, ROLES, Theme, ThemeDef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Theme,
    Colors,
    ColorDepth,
    Compact,
    Images,
    Filters,
    RefreshThread,
    RefreshWatched,
    Notify,
    DownloadDir,
    Restore,
    Keys,
}

/// A setting with its label and hint.
pub type Row = (Item, &'static str, &'static str);

/// The settings, by section, in display order.
pub const SECTIONS: &[(&str, &[Row])] = &[
    ("Appearance", &[
        (Item::Theme, "Theme", "Live preview while choosing"),
        (Item::Colors, "Colors", "Change any color of the current theme"),
        (Item::ColorDepth, "Color depth", "24-bit color, or the nearest of 256"),
    ]),
    ("Catalog", &[
        (Item::Compact, "Default layout", "c in a catalog sets a board's own"),
        (Item::Images, "Images", "Thumbnails and the image viewer (after a restart)"),
        (Item::Filters, "Filters", "Hide or highlight by pattern; X adds one from a post"),
    ]),
    ("Background refresh", &[
        (Item::RefreshThread, "Open thread", "How often the open thread updates"),
        (Item::RefreshWatched, "Watched threads", "How often each watched thread updates"),
        (Item::Notify, "Notifications", "New posts in watched threads, replies to yours (m)"),
    ]),
    ("Downloads", &[(Item::DownloadDir, "Folder", "Where d / D save files")]),
    ("Startup", &[(Item::Restore, "Last place", "Start where you left off (ck URL starts elsewhere)")]),
    ("Keys", &[(Item::Keys, "Key bindings", "Rebind any command")]),
];

const REFRESH_THREAD: &[u64] = &[10, 15, 30, 60, 120];
const REFRESH_WATCHED: &[u64] = &[60, 120, 300, 600, 1800];

pub fn items() -> Vec<Item> {
    SECTIONS.iter().flat_map(|(_, items)| items.iter().map(|&(item, ..)| item)).collect()
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

pub enum Popup {
    /// Choosing a theme; moving previews it, `before` is restored on esc.
    Themes { list: ListState, names: Vec<String>, before: (String, Theme) },
    /// The current theme's colors; `editing` holds a color being typed.
    Colors { list: ListState, editing: Option<String> },
    /// Typing the download folder.
    Folder { value: String },
    /// The key editor, over `key_rows()`; `capture` is waiting for a key to bind
    /// (`Some(true)`: add it to the action's keys).
    Keys { list: ListState, capture: Option<bool> },
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
        let actions: Vec<usize> = (0..ACTIONS.len()).filter(|&i| ACTIONS[i].3[0] == scope).collect();
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
    pub fn open_settings(&mut self) {
        if self.tab.view != View::Settings {
            self.tab.settings_back = Some(self.tab.view);
            self.tab.view = View::Settings;
        }
    }

    pub fn selected_setting(&self) -> Option<Item> {
        self.settings_list.state.selected().and_then(|i| items().get(i).copied())
    }

    /// A setting's current value, as shown.
    pub fn setting_value(&self, item: Item) -> String {
        match item {
            Item::Theme => self.theme_name.clone(),
            Item::Colors => match self.themes.get(&self.theme_name) {
                Some(def) if !def.colors.is_empty() => format!("{} changed", def.colors.len()),
                _ => "as the theme has them".into(),
            },
            Item::ColorDepth => match self.color_mode {
                ColorMode::Auto => format!("auto ({})", if self.truecolor { "24-bit" } else { "256" }),
                m => m.as_str().into(),
            },
            Item::Compact => self.default_layout.as_str().into(),
            Item::Images => match self.images_mode {
                ImagesMode::Auto => "on".into(),
                ImagesMode::Off => "off".into(),
            },
            Item::Filters => {
                let hidden: usize = self.store.hidden.values().map(Vec::len).sum();
                let off = self.filter_cfgs.iter().filter(|f| !f.enabled).count();
                let off = if off > 0 { format!(" ({off} off)") } else { String::new() };
                let n = self.filter_cfgs.len();
                format!("{n} filter{}{off}, {hidden} hidden by hand", if n == 1 { "" } else { "s" })
            }
            Item::RefreshThread => format!("every {}s", self.refresh_thread.as_secs()),
            Item::RefreshWatched => format!("every {}s", self.refresh_watched.as_secs()),
            Item::Notify => {
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
            Item::DownloadDir => self.download_dir.clone().unwrap_or_else(|| "~/Downloads/ck/{site}/{board}/{thread}".into()),
            Item::Restore => if self.restore_session { "restored" } else { "not restored" }.into(),
            Item::Keys => match ACTIONS.iter().filter(|e| !self.keys.is_default(e.0)).count() {
                0 => "defaults".into(),
                n => format!("{n} changed"),
            },
        }
    }

    /// Enter on a setting.
    pub fn activate_setting(&mut self) {
        let Some(item) = self.selected_setting() else { return };
        match item {
            Item::Theme => {
                let names = theme::names(&self.themes);
                let list = ListState::default().with_selected(Some(names.iter().position(|n| *n == self.theme_name).unwrap_or(0)));
                let before = (self.theme_name.clone(), theme::theme());
                self.settings_popup = Some(Popup::Themes { list, names, before });
            }
            Item::Colors => {
                let list = ListState::default().with_selected(Some(0));
                self.settings_popup = Some(Popup::Colors { list, editing: None });
            }
            Item::ColorDepth => {
                self.color_mode = self.color_mode.next();
                self.truecolor = self.color_mode.truecolor();
                let mode = self.color_mode.as_str();
                self.save_config(&format!("color depth {mode}"), |d| d["color"] = toml_edit::value(mode));
            }
            Item::Compact => self.cycle_default_layout(),
            Item::Images => {
                self.images_mode = self.images_mode.next();
                let mode = self.images_mode.as_str();
                self.save_config("images (from the next start)", |d| d["images"] = toml_edit::value(mode));
            }
            Item::Filters => self.settings_popup = Some(self.filter_list(0)),
            Item::RefreshThread => {
                let secs = next(REFRESH_THREAD, self.refresh_thread.as_secs());
                self.refresh_thread = Duration::from_secs(secs);
                self.save_config(&format!("refresh every {secs}s"), |d| d["refresh_thread_secs"] = toml_edit::value(secs as i64));
            }
            Item::RefreshWatched => {
                let secs = next(REFRESH_WATCHED, self.refresh_watched.as_secs());
                self.refresh_watched = Duration::from_secs(secs);
                self.save_config(&format!("refresh every {secs}s"), |d| d["refresh_watched_secs"] = toml_edit::value(secs as i64));
            }
            Item::Notify => {
                self.notify_mode = self.notify_mode.next();
                let mode = self.notify_mode.as_str();
                self.save_config(&format!("notifications {mode}"), |d| d["notify"] = toml_edit::value(mode));
            }
            Item::DownloadDir => {
                self.settings_popup = Some(Popup::Folder { value: self.download_dir.clone().unwrap_or_default() });
            }
            Item::Restore => {
                self.restore_session = !self.restore_session;
                let on = self.restore_session;
                self.save_config(if on { "restoring the last place" } else { "not restoring the last place" }, |d| {
                    d["restore_session"] = toml_edit::value(on)
                });
            }
            Item::Keys => {
                let list = ListState::default().with_selected(key_rows().iter().position(Result::is_ok));
                self.settings_popup = Some(Popup::Keys { list, capture: None });
            }
        }
    }

    /// Keys while a settings popup is open.
    pub fn on_settings_popup_key(&mut self, key: KeyEvent) {
        let Some(popup) = self.settings_popup.take() else { return };
        self.settings_popup = match popup {
            Popup::Themes { mut list, names, before } => match key.code {
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
                    Some(Popup::Themes { list, names, before })
                }
            },
            Popup::Colors { list, editing: Some(mut text) } => match key.code {
                KeyCode::Esc => Some(Popup::Colors { list, editing: None }),
                KeyCode::Enter => {
                    let role = ROLES[list.selected().unwrap_or(0)].0;
                    match theme::parse_color(&text) {
                        Ok(c) => {
                            self.set_role_color(role, Some(theme::color_string(c)));
                            Some(Popup::Colors { list, editing: None })
                        }
                        Err(e) => {
                            self.error(e);
                            Some(Popup::Colors { list, editing: Some(text) })
                        }
                    }
                }
                code => {
                    edit_text(&mut text, code);
                    Some(Popup::Colors { list, editing: Some(text) })
                }
            },
            Popup::Colors { mut list, editing: None } => {
                let cur = list.selected().unwrap_or(0);
                let role = ROLES[cur].0;
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left => None,
                    // The field starts empty; the current color is on the row above it.
                    KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                        Some(Popup::Colors { list, editing: Some(String::new()) })
                    }
                    // Back to what the base theme has.
                    KeyCode::Char('x') | KeyCode::Delete => {
                        self.set_role_color(role, None);
                        Some(Popup::Colors { list, editing: None })
                    }
                    code => {
                        list.select(Some(list_move(code, cur, ROLES.len()).unwrap_or(cur)));
                        Some(Popup::Colors { list, editing: None })
                    }
                }
            }
            Popup::Keys { list, capture: Some(add) } => {
                if key.code != KeyCode::Esc {
                    self.bind_key(&list, Some((Key::from_event(&key), add)));
                }
                Some(Popup::Keys { list, capture: None })
            }
            Popup::Keys { mut list, capture: None } => match key.code {
                KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left => None,
                KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => Some(Popup::Keys { list, capture: Some(false) }),
                KeyCode::Char('a') => Some(Popup::Keys { list, capture: Some(true) }),
                KeyCode::Char('x') | KeyCode::Delete => {
                    self.bind_key(&list, None);
                    Some(Popup::Keys { list, capture: None })
                }
                code => {
                    let rows = key_rows();
                    let actions: Vec<usize> = (0..rows.len()).filter(|&r| rows[r].is_ok()).collect();
                    let cur = actions.iter().position(|&r| Some(r) == list.selected()).unwrap_or(0);
                    let to = match code {
                        KeyCode::PageDown => (cur + 10).min(actions.len().saturating_sub(1)),
                        KeyCode::PageUp => cur.saturating_sub(10),
                        _ => list_move(code, cur, actions.len()).unwrap_or(cur),
                    };
                    list.select(actions.get(to).copied());
                    Some(Popup::Keys { list, capture: None })
                }
            },
            Popup::Filters { list, counts } => self.on_filter_list_key(key, list, counts),
            Popup::FilterEdit { index, draft, row, typing } => self.on_filter_edit_key(key, index, draft, row, typing),
            Popup::Folder { mut value } => match key.code {
                KeyCode::Esc => None,
                KeyCode::Enter => {
                    let dir = Some(value.trim().to_string()).filter(|v| !v.is_empty());
                    self.download_dir = dir.clone();
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
                    Some(Popup::Folder { value })
                }
            },
        };
    }

    /// Bind a key to the action selected in the key editor (replacing its keys, or added to
    /// them), or with `None` reset it to the default; refused if it would clash.
    fn bind_key(&mut self, list: &ListState, key: Option<(Key, bool)>) {
        let Some(Ok(i)) = list.selected().and_then(|r| key_rows().get(r).copied()) else { return };
        let (action, name, ..) = ACTIONS[i];
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
    fn set_role_color(&mut self, role: &str, color: Option<String>) {
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
                def.colors.insert(role.to_string(), c.clone());
            }
            None => {
                def.colors.remove(role);
            }
        }
        match theme::resolve(&name, &self.themes) {
            Ok(t) => {
                self.theme_name = name.clone();
                self.set_theme(t);
                let what = match &color {
                    Some(c) => format!("{role} = {c} in theme {name}"),
                    None => format!("{role} reset in theme {name}"),
                };
                self.save_config(&what, |d| {
                    config::set_theme(d, &name);
                    config::set_theme_color(d, &name, base.as_deref(), role, color.as_deref());
                });
            }
            Err(e) => self.error(e),
        }
    }

    /// Write a change to the config file and say where it went.
    pub fn save_config(&mut self, what: &str, f: impl FnOnce(&mut toml_edit::DocumentMut)) {
        match self.edit_config(f) {
            Ok(path) => self.info(format!("Saved {what} in {path}")),
            Err(e) => self.error(format!("Changed {what} for now; couldn't save it: {e:#}")),
        }
    }

    /// Edit the config file; returns its path, for messages.
    pub fn edit_config(&self, f: impl FnOnce(&mut toml_edit::DocumentMut)) -> anyhow::Result<String> {
        let path = self.config_path.as_ref().ok_or_else(|| anyhow::anyhow!("no home directory to keep a config file in"))?;
        config::edit_at(path, f)?;
        Ok(tilde(&path.display().to_string()))
    }
}

/// The next value in a cycle of choices.
fn next(choices: &[u64], current: u64) -> u64 {
    choices.iter().copied().find(|&c| c > current).unwrap_or(choices[0])
}

/// A path with the home directory shown as `~`.
pub fn tilde(path: &str) -> String {
    let home = dirs::home_dir().map(|h| h.display().to_string()).unwrap_or_default();
    match path.strip_prefix(&home).filter(|_| !home.is_empty()) {
        Some(rest) => format!("~{rest}"),
        None => path.to_string(),
    }
}
