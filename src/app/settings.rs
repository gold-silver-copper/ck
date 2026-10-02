//! The settings screen: theme, colors and other options, saved to the config file (comments
//! kept; created from the default config if there isn't one yet).

use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;

use super::{App, View};
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
    Keys,
}

/// The settings, by section, in display order.
pub const SECTIONS: &[(&str, &[Item])] = &[
    ("Appearance", &[Item::Theme, Item::Colors, Item::ColorDepth]),
    ("Catalog", &[Item::Compact, Item::Images, Item::Filters]),
    ("Background refresh", &[Item::RefreshThread, Item::RefreshWatched, Item::Notify]),
    ("Downloads", &[Item::DownloadDir]),
    ("Keys", &[Item::Keys]),
];

const REFRESH_THREAD: &[u64] = &[10, 15, 30, 60, 120];
const REFRESH_WATCHED: &[u64] = &[60, 120, 300, 600, 1800];

pub fn items() -> Vec<Item> {
    SECTIONS.iter().flat_map(|(_, items)| items.iter().copied()).collect()
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

impl Item {
    pub fn label(self) -> &'static str {
        match self {
            Item::Theme => "Theme",
            Item::Colors => "Colors",
            Item::ColorDepth => "Color depth",
            Item::Compact => "Compact layout",
            Item::Images => "Images",
            Item::Filters => "Filters",
            Item::RefreshThread => "Open thread",
            Item::RefreshWatched => "Watched threads",
            Item::Notify => "Notifications",
            Item::DownloadDir => "Folder",
            Item::Keys => "Key bindings",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Item::Theme => "Live preview while choosing",
            Item::Colors => "Change any color of the current theme",
            Item::ColorDepth => "24-bit color, or the nearest of 256",
            Item::Compact => "One line per thread",
            Item::Images => "Thumbnails and the image viewer (after a restart)",
            Item::Filters => "[[filter]] in the config; H hides by hand, Z shows hidden",
            Item::RefreshThread => "How often the open thread updates",
            Item::RefreshWatched => "How often each watched thread updates",
            Item::Notify => "New posts in watched threads, replies to yours (m)",
            Item::DownloadDir => "Where d / D save files",
            Item::Keys => "Rebind any command",
        }
    }
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

#[derive(Default)]
pub struct Settings {
    pub popup: Option<Popup>,
    /// Where esc goes back to.
    pub back: Option<View>,
}

impl App {
    pub fn open_settings(&mut self) {
        if self.view != View::Settings {
            self.settings.back = Some(self.view);
            self.view = View::Settings;
        }
    }

    pub fn selected_setting(&self) -> Option<Item> {
        self.settings_list.state.selected().and_then(|i| items().get(i).copied())
    }

    /// A setting's current value, as shown.
    pub fn setting_value(&self, item: Item) -> String {
        let on = |b: bool| if b { "on" } else { "off" }.to_string();
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
            Item::Compact => on(self.compact),
            Item::Images => match self.images_mode {
                ImagesMode::Auto => "on".into(),
                ImagesMode::Off => "off".into(),
            },
            Item::Filters => {
                let hidden: usize = self.store.hidden.values().map(Vec::len).sum();
                format!("{} filters, {hidden} hidden by hand", self.filters.len())
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
                let mut list = ListState::default();
                list.select(Some(names.iter().position(|n| *n == self.theme_name).unwrap_or(0)));
                let before = (self.theme_name.clone(), theme::theme());
                self.settings.popup = Some(Popup::Themes { list, names, before });
            }
            Item::Colors => {
                let mut list = ListState::default();
                list.select(Some(0));
                self.settings.popup = Some(Popup::Colors { list, editing: None });
            }
            Item::ColorDepth => {
                self.color_mode = match self.color_mode {
                    ColorMode::Auto => ColorMode::Truecolor,
                    ColorMode::Truecolor => ColorMode::Ansi256,
                    ColorMode::Ansi256 => ColorMode::Auto,
                };
                self.truecolor = self.color_mode.truecolor();
                let mode = self.color_mode.as_str();
                self.save_config(&format!("color depth {mode}"), |d| d["color"] = toml_edit::value(mode));
            }
            Item::Compact => self.toggle_compact(),
            Item::Images => {
                self.images_mode = match self.images_mode {
                    ImagesMode::Auto => ImagesMode::Off,
                    ImagesMode::Off => ImagesMode::Auto,
                };
                let mode = if self.images_mode == ImagesMode::Auto { "auto" } else { "off" };
                self.save_config("images (from the next start)", |d| d["images"] = toml_edit::value(mode));
            }
            Item::Filters => {
                let path = self.config_path.as_ref().map_or("the config".into(), |p| tilde(&p.display().to_string()));
                self.status = Some((format!("Filters are [[filter]] tables in {path} (see the README); they apply from the next start"), false));
            }
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
                self.settings.popup = Some(Popup::Folder { value: self.download_dir.clone().unwrap_or_default() });
            }
            Item::Keys => {
                let mut list = ListState::default();
                list.select(key_rows().iter().position(Result::is_ok));
                self.settings.popup = Some(Popup::Keys { list, capture: None });
            }
        }
    }

    /// Keys while a settings popup is open. Returns false if there's none.
    pub fn on_settings_popup_key(&mut self, key: KeyEvent) -> bool {
        let Some(popup) = self.settings.popup.take() else { return false };
        self.settings.popup = match popup {
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
                    let len = names.len();
                    let cur = list.selected().unwrap_or(0);
                    let to = match code {
                        KeyCode::Char('j') | KeyCode::Down => (cur + 1).min(len - 1),
                        KeyCode::Char('k') | KeyCode::Up => cur.saturating_sub(1),
                        KeyCode::Char('g') | KeyCode::Home => 0,
                        KeyCode::Char('G') | KeyCode::End => len - 1,
                        _ => cur,
                    };
                    list.select(Some(to));
                    // Live preview.
                    if let Ok(t) = theme::resolve(&names[to], &self.themes) {
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
                            self.status = Some((format!("{e:#}"), true));
                            Some(Popup::Colors { list, editing: Some(text) })
                        }
                    }
                }
                KeyCode::Backspace => {
                    text.pop();
                    Some(Popup::Colors { list, editing: Some(text) })
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    Some(Popup::Colors { list, editing: Some(text) })
                }
                _ => Some(Popup::Colors { list, editing: Some(text) }),
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
                        let to = match code {
                            KeyCode::Char('j') | KeyCode::Down => (cur + 1).min(ROLES.len() - 1),
                            KeyCode::Char('k') | KeyCode::Up => cur.saturating_sub(1),
                            KeyCode::Char('g') | KeyCode::Home => 0,
                            KeyCode::Char('G') | KeyCode::End => ROLES.len() - 1,
                            _ => cur,
                        };
                        list.select(Some(to));
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
                        KeyCode::Char('j') | KeyCode::Down => (cur + 1).min(actions.len() - 1),
                        KeyCode::Char('k') | KeyCode::Up => cur.saturating_sub(1),
                        KeyCode::Char('g') | KeyCode::Home => 0,
                        KeyCode::Char('G') | KeyCode::End => actions.len() - 1,
                        KeyCode::PageDown => (cur + 10).min(actions.len() - 1),
                        KeyCode::PageUp => cur.saturating_sub(10),
                        _ => cur,
                    };
                    list.select(Some(actions[to]));
                    Some(Popup::Keys { list, capture: None })
                }
            },
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
                KeyCode::Backspace => {
                    value.pop();
                    Some(Popup::Folder { value })
                }
                KeyCode::Char(c) => {
                    value.push(c);
                    Some(Popup::Folder { value })
                }
                _ => Some(Popup::Folder { value }),
            },
        };
        true
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
            Err(e) => self.status = Some((format!("{e:#}"), true)),
        }
    }

    /// Switch the look now; cached thread layouts carry colors, so they're redone.
    pub fn set_theme(&mut self, t: Theme) {
        theme::set(t);
        if let Some(th) = &mut self.thread {
            th.layout = None;
        }
    }

    fn choose_theme(&mut self, name: &str) {
        match theme::resolve(name, &self.themes) {
            Ok(t) => {
                self.theme_name = name.to_string();
                self.set_theme(t);
                self.save_config(&format!("theme {name}"), |d| config::set_theme(d, name));
            }
            Err(e) => self.status = Some((format!("{e:#}"), true)),
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
            Err(e) => self.status = Some((format!("{e:#}"), true)),
        }
    }

    /// Write a change to the config file and say where it went.
    pub fn save_config(&mut self, what: &str, f: impl FnOnce(&mut toml_edit::DocumentMut)) {
        self.status = Some(match self.edit_config(f) {
            Ok(path) => (format!("Saved {what} in {path}"), false),
            Err(e) => (format!("Changed {what} for now; couldn't save it: {e:#}"), true),
        });
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
    match dirs::home_dir().map(|h| h.display().to_string()) {
        Some(home) if path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}
