//! Remappable commands, configured in the `[keys]` section of config.toml and on the
//! Settings screen. Navigation (j/k/g/G/h/l/J/K, arrows, enter, esc, space, ctrl-d/u) is
//! fixed, and so are keys inside text inputs and popups.

use std::collections::HashMap;
use std::fmt;

use anyhow::{Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Help,
    Settings,
    Search,
    Reload,
    Browser,
    View,
    Watch,
    Sort,
    Compact,
    OpenFile,
    Replies,
    JumpBack,
    Unread,
    Preview,
    NextMatch,
    PrevMatch,
    Spoiler,
    AllSpoilers,
    Download,
    DownloadThread,
    Archive,
    Remove,
    Copy,
    CopyLink,
    Goto,
    Links,
    Hide,
    ShowHidden,
    Mine,
    Expand,
    Gallery,
    Export,
}

/// Where a key applies. Global keys work in every view but the image viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    /// Sites, Boards and Settings.
    Lists,
    Catalog,
    Thread,
    /// Watched and History.
    Saved,
    /// The full-screen image viewer.
    Viewer,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::Global => "everywhere",
            Scope::Lists => "lists",
            Scope::Catalog => "catalog",
            Scope::Thread => "thread",
            Scope::Saved => "watched, history",
            Scope::Viewer => "image viewer",
        }
    }
}

const VIEWS: [Scope; 5] = [Scope::Lists, Scope::Catalog, Scope::Thread, Scope::Saved, Scope::Viewer];

/// Every action: config name, default key, scopes, and what it does.
pub const ACTIONS: &[(Action, &str, &str, &[Scope], &str)] = &[
    (Action::Quit, "quit", "q", &[Scope::Global], "quit"),
    (Action::Help, "help", "?", &[Scope::Global], "help"),
    (Action::Settings, "settings", ",", &[Scope::Global], "settings: theme, colors, keys, …"),
    (Action::Search, "search", "/", &[Scope::Global], "filter the list; search a thread"),
    (Action::Reload, "reload", "r", &[Scope::Global], "reload"),
    (Action::Browser, "browser", "o", &[Scope::Global], "open in the browser"),
    (Action::Goto, "goto", ":", &[Scope::Global], "go to a URL or site/board/thread"),
    (Action::View, "view", "v", &[Scope::Catalog, Scope::Thread], "view the post's images"),
    (Action::Watch, "watch", "w", &[Scope::Catalog, Scope::Thread], "watch / unwatch the thread"),
    (Action::Sort, "sort", "s", &[Scope::Catalog], "cycle the sort order"),
    (Action::Compact, "compact", "c", &[Scope::Catalog], "layout: cards, compact, grid"),
    (Action::OpenFile, "open_file", "i", &[Scope::Thread], "open the file (videos in mpv)"),
    (Action::Replies, "replies", "b", &[Scope::Thread], "jump to the first reply"),
    (Action::JumpBack, "jump_back", "u", &[Scope::Thread], "jump back (also to the last thread)"),
    (Action::Unread, "unread", "U", &[Scope::Thread], "jump to the first unread post"),
    (Action::Preview, "preview", "p", &[Scope::Thread], "preview the quoted posts"),
    (Action::NextMatch, "next_match", "n", &[Scope::Thread], "next search match"),
    (Action::PrevMatch, "prev_match", "N", &[Scope::Thread], "previous search match"),
    (Action::Spoiler, "spoiler", "s", &[Scope::Thread], "show the post's spoilers"),
    (Action::AllSpoilers, "all_spoilers", "S", &[Scope::Thread], "show all spoilers"),
    (Action::Download, "download", "d", &[Scope::Thread], "save the post's files"),
    (Action::DownloadThread, "download_thread", "D", &[Scope::Thread], "save the thread's files"),
    (Action::Archive, "archive", "a", &[Scope::Thread], "open a 404'd thread in the archive"),
    (Action::Links, "links", "O", &[Scope::Catalog, Scope::Thread], "the post's links and files"),
    (Action::Hide, "hide", "H", &[Scope::Catalog, Scope::Thread], "hide / unhide the thread or post"),
    (Action::ShowHidden, "show_hidden", "Z", &[Scope::Catalog, Scope::Thread], "show hidden threads and posts"),
    (Action::Gallery, "gallery", "V", &[Scope::Thread], "the thread's files as a grid"),
    (Action::Export, "export", "E", &[Scope::Thread], "save the thread as HTML and JSON"),
    (Action::Expand, "expand", "e", &[Scope::Thread], "show / hide the post's replies under it"),
    (Action::Mine, "mine", "m", &[Scope::Thread], "mark the post as yours (notified of replies)"),
    (Action::Remove, "remove", "x", &[Scope::Saved], "remove the entry"),
    (Action::Copy, "copy", "y", &[Scope::Catalog, Scope::Thread, Scope::Saved, Scope::Viewer], "copy the text (viewer: file URL)"),
    (Action::CopyLink, "copy_link", "Y", &[Scope::Catalog, Scope::Thread, Scope::Saved, Scope::Viewer], "copy the link"),
];

/// A key with its modifiers: `w`, `W`, `ctrl-w`, `alt-x`, `tab`, `shift-tab`, `f5`, ...
/// Shift is part of a character (`W`), so it's only kept for tab (`shift-tab`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

const NAMED: &[(&str, KeyCode)] = &[
    ("tab", KeyCode::Tab),
    ("shift-tab", KeyCode::BackTab),
    ("enter", KeyCode::Enter),
    ("esc", KeyCode::Esc),
    ("backspace", KeyCode::Backspace),
    ("delete", KeyCode::Delete),
    ("insert", KeyCode::Insert),
    ("up", KeyCode::Up),
    ("down", KeyCode::Down),
    ("left", KeyCode::Left),
    ("right", KeyCode::Right),
    ("home", KeyCode::Home),
    ("end", KeyCode::End),
    ("pageup", KeyCode::PageUp),
    ("pagedown", KeyCode::PageDown),
    ("space", KeyCode::Char(' ')),
];

impl Key {
    pub const fn char(c: char) -> Self {
        Self { code: KeyCode::Char(c), mods: KeyModifiers::NONE }
    }

    const fn ctrl(c: char) -> Self {
        Self { code: KeyCode::Char(c), mods: KeyModifiers::CONTROL }
    }

    const fn code(code: KeyCode) -> Self {
        Self { code, mods: KeyModifiers::NONE }
    }

    /// The key a terminal event stands for.
    pub fn from_event(ev: &KeyEvent) -> Self {
        let mods = ev.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT);
        let code = match ev.code {
            KeyCode::Tab if ev.modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
            c => c,
        };
        Self { code, mods }
    }

    pub fn parse(s: &str) -> Result<Self> {
        let mut mods = KeyModifiers::NONE;
        let mut rest = s;
        loop {
            if let Some(r) = rest.strip_prefix("ctrl-").filter(|r| !r.is_empty()) {
                mods |= KeyModifiers::CONTROL;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("alt-").filter(|r| !r.is_empty()) {
                mods |= KeyModifiers::ALT;
                rest = r;
            } else {
                break;
            }
        }
        let mut chars = rest.chars();
        let code = match (chars.next(), chars.next()) {
            (Some(c), None) => KeyCode::Char(c),
            _ => {
                let lower = rest.to_lowercase();
                match NAMED.iter().find(|(n, _)| *n == lower) {
                    Some(&(_, code)) => code,
                    None => match lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                        Some(n @ 1..=12) => KeyCode::F(n),
                        _ => bail!("`{s}` isn't a key (try \"w\", \"W\", \"ctrl-w\", \"alt-w\", \"tab\", \"f5\")"),
                    },
                }
            }
        };
        Ok(Self { code, mods })
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.mods.contains(KeyModifiers::CONTROL) {
            f.write_str("ctrl-")?;
        }
        if self.mods.contains(KeyModifiers::ALT) {
            f.write_str("alt-")?;
        }
        if let Some((name, _)) = NAMED.iter().find(|(_, c)| *c == self.code) {
            return f.write_str(name);
        }
        match self.code {
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::F(n) => write!(f, "f{n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// `watch = "W"` or `watch = ["W", "ctrl-w"]`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Binding {
    One(String),
    Many(Vec<String>),
}

impl Binding {
    fn specs(&self) -> Vec<&str> {
        match self {
            Binding::One(s) => vec![s],
            Binding::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

/// Fixed navigation keys per view, which remapped keys must not collide with.
fn fixed(scope: Scope) -> Vec<Key> {
    let codes = [
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Backspace,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::F(5),
    ];
    let mut keys: Vec<Key> = codes.into_iter().map(Key::code).collect();
    keys.push(Key::ctrl('c'));
    let chars: &[char] = match scope {
        Scope::Thread => &['j', 'k', 'g', 'G', 'h', 'l', 'J', 'K', ' '],
        Scope::Viewer => &['j', 'k', 'h', 'l', 'i', 'q', 'v', ' '],
        _ => &['j', 'k', 'g', 'G', 'h', 'l'],
    };
    keys.extend(chars.iter().map(|&c| Key::char(c)));
    if scope != Scope::Viewer {
        keys.extend([Key::ctrl('d'), Key::ctrl('u')]);
    }
    keys
}

fn applies(scopes: &[Scope], view: Scope) -> bool {
    scopes.iter().any(|&s| s == view || (s == Scope::Global && view != Scope::Viewer))
}

#[derive(Debug, Clone)]
pub struct KeyMap {
    keys: HashMap<Action, Vec<Key>>,
}

impl Default for KeyMap {
    fn default() -> Self {
        Self { keys: ACTIONS.iter().map(|&(a, ..)| (a, vec![default_key(a)])).collect() }
    }
}

fn default_key(action: Action) -> Key {
    let spec = ACTIONS.iter().find(|e| e.0 == action).expect("every action is listed").2;
    Key::parse(spec).expect("default keys parse")
}

impl KeyMap {
    /// Apply `[keys]` overrides, rejecting unknown actions, things that aren't keys, and two
    /// commands on one key in the same view.
    pub fn new(overrides: &HashMap<String, Binding>) -> Result<Self> {
        let mut map = Self::default();
        let mut names: Vec<_> = overrides.keys().collect();
        names.sort();
        for name in names {
            let Some(&(action, ..)) = ACTIONS.iter().find(|e| e.1 == name) else {
                let known: Vec<_> = ACTIONS.iter().map(|e| e.1).collect();
                bail!("[keys]: unknown action `{name}` (known: {})", known.join(", "));
            };
            let specs = overrides[name].specs();
            if specs.is_empty() {
                bail!("[keys]: `{name}` has no keys");
            }
            let keys = specs.iter().map(|s| Key::parse(s)).collect::<Result<Vec<_>>>();
            map.keys.insert(action, keys.map_err(|e| anyhow::anyhow!("[keys]: `{name}`: {e}"))?);
        }
        map.check().map_err(|e| anyhow::anyhow!("[keys]: {e}"))?;
        Ok(map)
    }

    /// No key does two things in one view.
    fn check(&self) -> Result<()> {
        for view in VIEWS {
            let mut seen: HashMap<Key, &str> = fixed(view).into_iter().map(|k| (k, "navigation")).collect();
            for &(action, name, _, scopes, _) in ACTIONS {
                if !applies(scopes, view) {
                    continue;
                }
                for &key in &self.keys[&action] {
                    if let Some(other) = seen.insert(key, name)
                        && other != name
                    {
                        bail!("`{name}` and `{other}` both use '{key}' in the {} view", view.label());
                    }
                }
            }
        }
        Ok(())
    }

    /// The first key of an action, as shown in hints.
    pub fn key(&self, action: Action) -> String {
        self.keys[&action][0].to_string()
    }

    /// All of an action's keys, for help.
    pub fn label(&self, action: Action) -> String {
        self.keys[&action].iter().map(Key::to_string).collect::<Vec<_>>().join(", ")
    }

    pub fn keys(&self, action: Action) -> &[Key] {
        &self.keys[&action]
    }

    pub fn is_default(&self, action: Action) -> bool {
        self.keys[&action] == [default_key(action)]
    }

    /// The action bound to a key in `scope` (or a global one).
    pub fn action(&self, scope: Scope, ev: &KeyEvent) -> Option<Action> {
        let key = Key::from_event(ev);
        ACTIONS
            .iter()
            .find(|&&(a, _, _, scopes, _)| applies(scopes, scope) && self.keys[&a].contains(&key))
            .map(|&(a, ..)| a)
    }

    /// A copy with an action's keys replaced (`None`: the default), if that leaves no
    /// conflicts.
    pub fn with(&self, action: Action, keys: Option<Vec<Key>>) -> Result<Self> {
        let mut map = self.clone();
        map.keys.insert(action, keys.unwrap_or_else(|| vec![default_key(action)]));
        map.check()?;
        Ok(map)
    }

    /// What `[keys]` holds for an action: `None` when it has the default.
    pub fn binding(&self, action: Action) -> Option<Binding> {
        if self.is_default(action) {
            return None;
        }
        let keys: Vec<String> = self.keys[&action].iter().map(Key::to_string).collect();
        Some(if keys.len() == 1 { Binding::One(keys[0].clone()) } else { Binding::Many(keys) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> Result<KeyMap> {
        KeyMap::new(&pairs.iter().map(|(a, b)| (a.to_string(), Binding::One(b.to_string()))).collect())
    }

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn ch(c: char) -> KeyEvent {
        ev(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn defaults_are_consistent() {
        let m = map(&[]).unwrap();
        assert_eq!(m.action(Scope::Catalog, &ch('s')), Some(Action::Sort));
        assert_eq!(m.action(Scope::Thread, &ch('s')), Some(Action::Spoiler));
        assert_eq!(m.action(Scope::Saved, &ch('q')), Some(Action::Quit));
        assert_eq!(m.action(Scope::Lists, &ch('x')), None);
        // Global keys don't apply in the image viewer, which has its own.
        assert_eq!(m.action(Scope::Viewer, &ch('q')), None);
        // Shift is part of the character.
        assert_eq!(m.action(Scope::Thread, &ev(KeyCode::Char('U'), KeyModifiers::SHIFT)), Some(Action::Unread));
    }

    #[test]
    fn overrides_and_errors() {
        let m = map(&[("watch", "W")]).unwrap();
        assert_eq!(m.action(Scope::Thread, &ch('W')), Some(Action::Watch));
        assert_eq!(m.action(Scope::Thread, &ch('w')), None);

        let err = |pairs| map(pairs).unwrap_err().to_string();
        assert!(err(&[("nope", "z")]).contains("unknown action `nope`"));
        assert!(err(&[("watch", "ctrl-")]).contains("isn't a key"));
        assert!(err(&[("watch", "hyper-w")]).contains("isn't a key"));
        assert!(err(&[("sort", "c")]).contains("both use 'c' in the catalog view"));
        assert!(err(&[("preview", "j")]).contains("navigation"));
        assert!(err(&[("preview", "ctrl-d")]).contains("navigation"));
        // A global key can't shadow a view's key.
        assert!(err(&[("reload", "v")]).contains("'v'"));
    }

    #[test]
    fn key_specs_round_trip() {
        for s in ["w", "W", ":", ",", "ctrl-w", "alt-x", "ctrl-alt-k", "tab", "shift-tab", "f5", "f12", "pageup", "space", "delete"] {
            assert_eq!(Key::parse(s).unwrap().to_string(), s, "{s}");
        }
        assert_eq!(Key::parse("Tab").unwrap(), Key::code(KeyCode::Tab));
        assert!(Key::parse("f13").is_err() && Key::parse("").is_err() && Key::parse("ww").is_err());
        // Terminal events map onto the same keys.
        assert_eq!(Key::from_event(&ev(KeyCode::Tab, KeyModifiers::SHIFT)), Key::parse("shift-tab").unwrap());
        assert_eq!(Key::from_event(&ev(KeyCode::Char('w'), KeyModifiers::CONTROL)), Key::parse("ctrl-w").unwrap());
    }

    #[test]
    fn several_keys_per_action() {
        let overrides = HashMap::from([("watch".to_string(), Binding::Many(vec!["W".into(), "ctrl-w".into()]))]);
        let m = KeyMap::new(&overrides).unwrap();
        assert_eq!(m.action(Scope::Catalog, &ch('W')), Some(Action::Watch));
        assert_eq!(m.action(Scope::Catalog, &ev(KeyCode::Char('w'), KeyModifiers::CONTROL)), Some(Action::Watch));
        assert_eq!(m.label(Action::Watch), "W, ctrl-w");
        assert_eq!(m.binding(Action::Watch), Some(Binding::Many(vec!["W".into(), "ctrl-w".into()])));
        assert_eq!(m.binding(Action::Sort), None);
        // Conflicts are checked across every key of every action.
        let bad = HashMap::from([("sort".to_string(), Binding::Many(vec!["z".into(), "W".into()])), ("watch".to_string(), Binding::One("W".into()))]);
        assert!(KeyMap::new(&bad).unwrap_err().to_string().contains("'W'"));
        // Editing: a conflicting change is refused, a reset restores the default.
        assert!(m.with(Action::Sort, Some(vec![Key::char('v')])).is_err());
        let m = m.with(Action::Watch, None).unwrap();
        assert!(m.is_default(Action::Watch));
    }
}
