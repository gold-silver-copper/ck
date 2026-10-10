//! Remappable commands, configured in the `[keys]` section of config.toml and on the
//! Settings screen. Navigation (j/k/g/G/h/l/J/K, arrows, enter, esc, space, ctrl-d/u) is
//! fixed, and so are keys inside text inputs and popups.

use std::collections::HashMap;
use std::fmt;

use anyhow::{Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

/// Where a key applies. Global keys work in every view but the image viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// Declares `Action` and `ACTIONS` (every action: config name, default key, scopes, and
/// what it does) from one list. Actions without a default key are in the `.` menu, and can
/// be given one.
macro_rules! actions {
    ($($action:ident, $name:literal, $key:expr, $scopes:expr, $what:literal;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Action {
            $($action,)*
        }

        pub const ACTIONS: &[(Action, &str, Option<Key>, &[Scope], &str)] = &[$((Action::$action, $name, $key, $scopes, $what),)*];
    };
}

actions! {
    Quit, "quit", Some(Key::char('q')), &[Scope::Global], "quit";
    Help, "help", Some(Key::char('?')), &[Scope::Global], "help";
    Settings, "settings", Some(Key::char(',')), &[Scope::Global], "settings: theme, colors, keys, …";
    Search, "search", Some(Key::char('/')), &[Scope::Global], "filter the list; search a thread";
    Reload, "reload", Some(Key::char('r')), &[Scope::Global], "reload";
    Browser, "browser", Some(Key::char('o')), &[Scope::Global], "open in the browser";
    Goto, "goto", Some(Key::char(':')), &[Scope::Global], "go to a URL or site/board/thread";
    NextTab, "next_tab", Some(Key::char(']')), &[Scope::Global], "next tab";
    PrevTab, "prev_tab", Some(Key::char('[')), &[Scope::Global], "previous tab";
    Menu, "menu", Some(Key::char('.')), &[Scope::Global, Scope::Viewer], "what you can do with what's selected";
    Hints, "hints", Some(Key::char('f')), &[Scope::Lists, Scope::Catalog, Scope::Thread, Scope::Saved], "label what's on screen; type a label to open it";
    NextPart, "next_part", Some(Key::code(KeyCode::Tab)), &[Scope::Thread], "focus the post's next image or link (then the next post's)";
    PrevPart, "prev_part", Some(Key::code(KeyCode::BackTab)), &[Scope::Thread], "focus the previous image or link";
    CloseTab, "close_tab", Some(Key::ctrl('w')), &[Scope::Global], "close the tab";
    View, "view", Some(Key::char('v')), &[Scope::Catalog, Scope::Thread], "view the post's images";
    Watch, "watch", Some(Key::char('w')), &[Scope::Catalog, Scope::Thread], "watch / unwatch the thread";
    Sort, "sort", Some(Key::char('s')), &[Scope::Catalog], "cycle the sort order";
    Compact, "compact", Some(Key::char('c')), &[Scope::Catalog], "layout: cards, compact, grid";
    OpenFile, "open_file", Some(Key::char('i')), &[Scope::Thread], "open the file (videos in mpv)";
    Replies, "replies", Some(Key::char('b')), &[Scope::Thread], "jump to the first reply";
    JumpBack, "jump_back", Some(Key::char('u')), &[Scope::Lists, Scope::Catalog, Scope::Thread, Scope::Saved], "jump back: the post before, or the thread you left";
    JumpForward, "jump_forward", Some(Key::ctrl('r')), &[Scope::Lists, Scope::Catalog, Scope::Thread, Scope::Saved], "jump forward again (after jumping back)";
    Watched, "watched", Some(Key::char('W')), &[Scope::Global], "the watched threads";
    Unread, "unread", Some(Key::char('U')), &[Scope::Thread], "jump to the first unread post";
    Preview, "preview", Some(Key::char('p')), &[Scope::Thread], "preview the quoted posts";
    NextMatch, "next_match", Some(Key::char('n')), &[Scope::Thread], "next search match";
    PrevMatch, "prev_match", Some(Key::char('N')), &[Scope::Thread], "previous search match";
    Spoiler, "spoiler", Some(Key::char('s')), &[Scope::Thread], "show the post's spoilers";
    AllSpoilers, "all_spoilers", Some(Key::char('S')), &[Scope::Thread], "show all spoilers";
    Download, "download", Some(Key::char('d')), &[Scope::Thread, Scope::Viewer], "save the file in front of you: focused, viewed, or in the gallery";
    DownloadPost, "download_post", None, &[Scope::Thread], "save all the post's files";
    DownloadThread, "download_thread", None, &[Scope::Thread], "save all the thread's files (asks first)";
    Archive, "archive", Some(Key::char('a')), &[Scope::Thread], "open a 404'd thread in the archive";
    ArchiveSearch, "archive_search", Some(Key::char('A')), &[Scope::Catalog], "search the board's archive";
    Links, "links", Some(Key::char('O')), &[Scope::Catalog, Scope::Thread], "the post's links and files";
    Hide, "hide", Some(Key::char('H')), &[Scope::Catalog, Scope::Thread], "hide / unhide the thread or post";
    Filter, "filter", Some(Key::char('X')), &[Scope::Catalog, Scope::Thread], "hide or highlight posts like this one (a filter)";
    ShowHidden, "show_hidden", Some(Key::char('Z')), &[Scope::Catalog, Scope::Thread], "show hidden threads and posts";
    ImageSearch, "image_search", Some(Key::char('R')), &[Scope::Thread, Scope::Viewer], "reverse image search";
    Gallery, "gallery", Some(Key::char('V')), &[Scope::Thread], "the thread's files as a grid";
    Export, "export", None, &[Scope::Thread], "save the thread as a page, HTML and JSON (asks first)";
    Expand, "expand", Some(Key::char('e')), &[Scope::Thread], "show / hide the post's replies under it";
    Conversation, "conversation", Some(Key::char('c')), &[Scope::Thread], "the post's conversation alone: what it replies to, and its replies";
    Poster, "poster", Some(Key::char('I')), &[Scope::Thread], "the post's poster's posts alone (by poster ID)";
    Media, "media", Some(Key::char('M')), &[Scope::Thread], "show all posts, only those with files, or all with images hidden (in turn)";
    Mine, "mine", Some(Key::char('m')), &[Scope::Thread], "mark the post as yours (notified of replies)";
    Reply, "reply", Some(Key::char('P')), &[Scope::Catalog, Scope::Thread], "post: reply to the thread (quoting the post), or start one in the catalog";
    Quote, "quote", None, &[Scope::Thread], "reply quoting the post's text";
    NewTab, "new_tab", Some(Key::char('T')), &[Scope::Catalog, Scope::Thread, Scope::Saved], "open the thread (or link) in a new tab";
    Favorite, "favorite", Some(Key::char('*')), &[Scope::Lists, Scope::Catalog], "favorite board: on / off";
    AddSite, "add_site", None, &[Scope::Lists], "add a site from a link to any page of it";
    SearchSaved, "search_saved", None, &[Scope::Saved], "search inside the saved threads (also :saved WORDS)";
    UpdateBoards, "update_boards", None, &[Scope::Lists], "read a vichan site's board list again from its pages";
    BoardImages, "board_images", None, &[Scope::Lists, Scope::Catalog, Scope::Thread], "images on this board: on / off";
    Follow, "follow", Some(Key::char('F')), &[Scope::Catalog, Scope::Thread, Scope::Saved], "follow as a general: watch its next thread";
    Remove, "remove", Some(Key::char('x')), &[Scope::Saved, Scope::Lists], "remove the entry (home: a favorite)";
    Copy, "copy", Some(Key::char('y')), &[Scope::Catalog, Scope::Thread, Scope::Saved, Scope::Viewer], "copy the text (viewer: file URL)";
    CopyLink, "copy_link", Some(Key::char('Y')), &[Scope::Catalog, Scope::Thread, Scope::Saved, Scope::Viewer], "copy the link";
}

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

    pub const fn ctrl(c: char) -> Self {
        Self { code: KeyCode::Char(c), mods: KeyModifiers::CONTROL }
    }

    pub const fn code(code: KeyCode) -> Self {
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

/// `watch = "W"` or `watch = ["W", "alt-w"]`; `watch = []` for none.
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
        Scope::Viewer => &['j', 'k', 'h', 'l', 'i', 'q', 'v', ' ', '+', '=', '-', '0'],
        // The home screen opens favorites with 1-9.
        Scope::Lists => &['j', 'k', 'g', 'G', 'h', 'l', '1', '2', '3', '4', '5', '6', '7', '8', '9'],
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
    /// What each key does in each view, as `check` found it.
    actions: HashMap<(Scope, Key), Action>,
}

impl Default for KeyMap {
    fn default() -> Self {
        let keys = ACTIONS.iter().map(|&(a, _, key, ..)| (a, key.into_iter().collect())).collect();
        let mut map = Self { keys, actions: HashMap::new() };
        // The defaults don't clash (a test checks), so this can't fail.
        let _ = map.check();
        map
    }
}

fn default_keys(action: Action) -> Vec<Key> {
    ACTIONS.iter().filter(|e| e.0 == action).filter_map(|e| e.2).collect()
}

impl KeyMap {
    /// Apply `[keys]` overrides, rejecting unknown actions, things that aren't keys, and two
    /// commands on one key in the same view. No keys (`[]`) leaves an action to the menu.
    pub fn new(overrides: &HashMap<String, Binding>) -> Result<Self> {
        let mut map = Self::default();
        let mut named: Vec<_> = overrides.iter().collect();
        named.sort_by_key(|&(name, _)| name);
        for (name, binding) in named {
            let Some(&(action, ..)) = ACTIONS.iter().find(|e| e.1 == name) else {
                let known: Vec<_> = ACTIONS.iter().map(|e| e.1).collect();
                bail!("[keys]: unknown action `{name}` (known: {})", known.join(", "));
            };
            let specs = binding.specs();
            let keys = specs.iter().map(|s| Key::parse(s)).collect::<Result<Vec<_>>>();
            map.keys.insert(action, keys.map_err(|e| anyhow::anyhow!("[keys]: `{name}`: {e}"))?);
        }
        map.check().map_err(|e| anyhow::anyhow!("[keys]: {e}"))?;
        Ok(map)
    }

    /// No key does two things in one view. Fills in the lookup from keys to actions.
    fn check(&mut self) -> Result<()> {
        self.actions.clear();
        for view in VIEWS {
            let mut seen: HashMap<Key, &str> = fixed(view).into_iter().map(|k| (k, "navigation")).collect();
            for &(action, name, _, scopes, _) in ACTIONS {
                if !applies(scopes, view) {
                    continue;
                }
                for &key in self.keys.get(&action).into_iter().flatten() {
                    if let Some(other) = seen.insert(key, name)
                        && other != name
                    {
                        bail!("`{name}` and `{other}` both use '{key}' in the {} view", view.label());
                    }
                    self.actions.insert((view, key), action);
                }
            }
        }
        Ok(())
    }

    /// The first key of an action, as shown in hints; empty when it has none.
    pub fn key(&self, action: Action) -> String {
        self.keys(action).first().map(Key::to_string).unwrap_or_default()
    }

    /// All of an action's keys, for help; empty when it has none.
    pub fn label(&self, action: Action) -> String {
        self.keys(action).iter().map(Key::to_string).collect::<Vec<_>>().join(", ")
    }

    /// How to get to an action, for messages: its key, or the menu.
    pub fn how(&self, action: Action) -> String {
        match self.keys(action).first() {
            Some(k) => k.to_string(),
            None => match self.keys(Action::Menu).first() {
                Some(m) => format!("the {m} menu"),
                None => "the menu (right-click)".into(),
            },
        }
    }

    /// An action's keys; empty when it has none.
    pub fn keys(&self, action: Action) -> &[Key] {
        self.keys.get(&action).map_or(&[], Vec::as_slice)
    }

    pub fn is_default(&self, action: Action) -> bool {
        self.keys(action) == default_keys(action)
    }

    /// The action bound to a key in `scope` (or a global one).
    pub fn action(&self, scope: Scope, ev: &KeyEvent) -> Option<Action> {
        self.actions.get(&(scope, Key::from_event(ev))).copied()
    }

    /// A copy with an action's keys replaced (`None`: the default), if that leaves no
    /// conflicts.
    pub fn with(&self, action: Action, keys: Option<Vec<Key>>) -> Result<Self> {
        let mut map = self.clone();
        map.keys.insert(action, keys.unwrap_or_else(|| default_keys(action)));
        map.check()?;
        Ok(map)
    }

    /// What `[keys]` holds for an action: `None` when it has the default.
    pub fn binding(&self, action: Action) -> Option<Binding> {
        if self.is_default(action) {
            return None;
        }
        Some(match self.keys(action) {
            [one] => Binding::One(one.to_string()),
            keys => Binding::Many(keys.iter().map(Key::to_string).collect()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> Result<KeyMap> {
        KeyMap::new(&pairs.iter().map(|(a, b)| (a.to_string(), Binding::One(b.to_string()))).collect())
    }

    fn ch(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn defaults_are_consistent() {
        let m = map(&[]).unwrap();
        assert_eq!(m.action(Scope::Catalog, &ch('s')), Some(Action::Sort));
        assert_eq!(m.action(Scope::Thread, &ch('s')), Some(Action::Spoiler));
        assert_eq!(m.action(Scope::Saved, &ch('q')), Some(Action::Quit));
        assert_eq!(m.action(Scope::Lists, &ch('z')), None);
        // Global keys don't apply in the image viewer, which has its own.
        assert_eq!(m.action(Scope::Viewer, &ch('q')), None);
        // Shift is part of the character.
        assert_eq!(m.action(Scope::Thread, &KeyEvent::new(KeyCode::Char('U'), KeyModifiers::SHIFT)), Some(Action::Unread));
    }

    #[test]
    fn the_manuals_example_loads() {
        let doc = include_str!("../docs/manual.md");
        let block = doc.split("```toml\n[keys]").nth(1).and_then(|b| b.split("```").next()).unwrap();
        let cfg: crate::config::Config = toml::from_str(&format!("[keys]{block}")).unwrap();
        KeyMap::new(&cfg.keys).unwrap();
    }

    #[test]
    fn overrides_and_errors() {
        let m = map(&[("watch", "Q")]).unwrap();
        assert_eq!(m.action(Scope::Thread, &ch('Q')), Some(Action::Watch));
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
        assert_eq!(Key::from_event(&KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)), Key::parse("shift-tab").unwrap());
        assert_eq!(Key::from_event(&KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)), Key::parse("ctrl-w").unwrap());
    }

    #[test]
    fn several_keys_per_action() {
        let overrides = HashMap::from([("watch".to_string(), Binding::Many(vec!["Q".into(), "alt-w".into()]))]);
        let m = KeyMap::new(&overrides).unwrap();
        assert_eq!(m.action(Scope::Catalog, &ch('Q')), Some(Action::Watch));
        assert_eq!(m.action(Scope::Catalog, &KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT)), Some(Action::Watch));
        assert_eq!(m.label(Action::Watch), "Q, alt-w");
        assert_eq!(m.binding(Action::Watch), Some(Binding::Many(vec!["Q".into(), "alt-w".into()])));
        assert_eq!(m.binding(Action::Sort), None);
        // Conflicts are checked across every key of every action.
        let bad = HashMap::from([("sort".to_string(), Binding::Many(vec!["z".into(), "W".into()])), ("watch".to_string(), Binding::One("W".into()))]);
        assert!(KeyMap::new(&bad).unwrap_err().to_string().contains("'W'"));
        // Editing: a conflicting change is refused, a reset restores the default.
        assert!(m.with(Action::Sort, Some(vec![Key::char('v')])).is_err());
        let m = m.with(Action::Watch, None).unwrap();
        assert!(m.is_default(Action::Watch));
    }

    #[test]
    fn actions_without_keys() {
        // Saving all the thread's files or the page has no key by default: it's in the menu.
        let m = map(&[]).unwrap();
        assert!(m.keys(Action::Export).is_empty() && m.key(Action::Export).is_empty());
        assert_eq!(m.action(Scope::Thread, &ch('E')), None);
        assert_eq!(m.how(Action::Export), "the . menu");
        assert_eq!(m.how(Action::Download), "d");
        assert_eq!(m.binding(Action::Export), None);
        // `[]` takes a key away; it's kept as `[]`, and the key is free.
        let none = HashMap::from([("download".to_string(), Binding::Many(vec![]))]);
        let m = KeyMap::new(&none).unwrap();
        assert_eq!(m.action(Scope::Thread, &ch('d')), None);
        assert_eq!(m.binding(Action::Download), Some(Binding::Many(vec![])));
        assert_eq!(map(&[("watch", "d")]).unwrap_err().to_string(), "[keys]: `download` and `watch` both use 'd' in the thread view");
        assert_eq!(KeyMap::new(&none).unwrap().with(Action::Watch, Some(vec![Key::char('d')])).unwrap().action(Scope::Thread, &ch('d')), Some(Action::Watch));
        // Given a key, it works like any other.
        let m = map(&[("export", "E")]).unwrap();
        assert_eq!(m.action(Scope::Thread, &ch('E')), Some(Action::Export));
        let m = map(&[("menu", "alt-m")]).unwrap().with(Action::Menu, Some(vec![])).unwrap();
        assert_eq!(m.how(Action::Export), "the menu (right-click)");
    }
}
