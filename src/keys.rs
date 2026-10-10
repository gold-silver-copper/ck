//! Every key the views take, from two tables: the fixed keys that move around (`nav`), and
//! the commands (`Action`), remappable in the `[keys]` section of config.toml and on the
//! Settings screen. `KeyMap` looks both up, and won't give a command a key that's taken where
//! it applies, so what a key does and whether it can be rebound come from the same place.
//! Keys inside text inputs and popups are their own, and ctrl-c always quits.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

/// Where a key applies. Global commands work in every view, but not in the image viewer and
/// gallery, which have their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    /// Sites, Boards and Settings.
    Lists,
    Catalog,
    Thread,
    /// Watched and History.
    Saved,
    /// A thread's files as a grid.
    Gallery,
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
            Scope::Gallery => "gallery",
            Scope::Viewer => "image viewer",
        }
    }

    /// Whether a command for `self` applies in `place`.
    fn covers(self, place: Scope) -> bool {
        self == place || (self == Scope::Global && !matches!(place, Scope::Gallery | Scope::Viewer))
    }
}

/// Where keys go; `Global` stands for most of them.
const PLACES: [Scope; 6] = [Scope::Lists, Scope::Catalog, Scope::Thread, Scope::Saved, Scope::Gallery, Scope::Viewer];

/// Declares `Action`, every command with its config name, default key, scopes, and what it
/// does, from one list. Actions without a default key are in the `.` menu, and can be given
/// one.
macro_rules! actions {
    ($($action:ident, $name:literal, $key:expr, $scopes:expr, $what:literal;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Action {
            $($action,)*
        }

        impl Action {
            pub const ALL: &[Action] = &[$(Action::$action,)*];

            /// Its name in `[keys]`, its default key, where it applies, and what it does.
            const fn spec(self) -> (&'static str, Option<Key>, &'static [Scope], &'static str) {
                match self {
                    $(Action::$action => ($name, $key, $scopes, $what),)*
                }
            }
        }
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
    Menu, "menu", Some(Key::char('.')), &[Scope::Global, Scope::Gallery, Scope::Viewer], "what you can do with what's selected";
    Hints, "hints", Some(Key::char('f')), &[Scope::Lists, Scope::Catalog, Scope::Thread, Scope::Saved], "label what's on screen; type a label to open it";
    NextPart, "next_part", Some(Key::code(KeyCode::Tab)), &[Scope::Thread], "focus the post's next image or link (then the next post's)";
    PrevPart, "prev_part", Some(Key::code(KeyCode::BackTab)), &[Scope::Thread], "focus the previous image or link";
    CloseTab, "close_tab", Some(Key::ctrl('w')), &[Scope::Global], "close the tab";
    View, "view", Some(Key::char('v')), &[Scope::Catalog, Scope::Thread, Scope::Gallery], "view the post's images";
    Watch, "watch", Some(Key::char('w')), &[Scope::Catalog, Scope::Thread], "watch / unwatch the thread";
    Sort, "sort", Some(Key::char('s')), &[Scope::Catalog], "cycle the sort order";
    Compact, "compact", Some(Key::char('c')), &[Scope::Catalog], "layout: cards, compact, grid";
    OpenFile, "open_file", Some(Key::char('i')), &[Scope::Thread, Scope::Viewer], "open the file (videos in mpv)";
    Replies, "replies", Some(Key::char('b')), &[Scope::Thread], "jump to the first reply";
    JumpBack, "jump_back", Some(Key::char('u')), &[Scope::Lists, Scope::Catalog, Scope::Thread, Scope::Saved], "jump back: the post before, or the thread you left";
    JumpForward, "jump_forward", Some(Key::ctrl('r')), &[Scope::Lists, Scope::Catalog, Scope::Thread, Scope::Saved], "jump forward again (after jumping back)";
    Watched, "watched", Some(Key::char('W')), &[Scope::Global], "the watched threads";
    Unread, "unread", Some(Key::char('U')), &[Scope::Thread], "jump to the first unread post";
    Preview, "preview", Some(Key::char('p')), &[Scope::Thread], "preview the quoted posts";
    NextMatch, "next_match", Some(Key::char('n')), &[Scope::Thread, Scope::Lists], "next search match (archive search: more results)";
    PrevMatch, "prev_match", Some(Key::char('N')), &[Scope::Thread], "previous search match";
    Spoiler, "spoiler", Some(Key::char('s')), &[Scope::Thread], "show the post's spoilers";
    AllSpoilers, "all_spoilers", Some(Key::char('S')), &[Scope::Thread], "show all spoilers";
    Download, "download", Some(Key::char('d')), &[Scope::Thread, Scope::Gallery, Scope::Viewer], "save the file in front of you: focused, viewed, or in the gallery";
    DownloadPost, "download_post", None, &[Scope::Thread], "save all the post's files";
    DownloadThread, "download_thread", None, &[Scope::Thread, Scope::Gallery], "save all the thread's files (asks first)";
    Archive, "archive", Some(Key::char('a')), &[Scope::Thread], "open a 404'd thread in the archive";
    ArchiveSearch, "archive_search", Some(Key::char('A')), &[Scope::Catalog], "search the board's archive";
    Links, "links", Some(Key::char('O')), &[Scope::Catalog, Scope::Thread], "the post's links and files";
    Hide, "hide", Some(Key::char('H')), &[Scope::Catalog, Scope::Thread], "hide / unhide the thread or post";
    Filter, "filter", Some(Key::char('X')), &[Scope::Catalog, Scope::Thread], "hide or highlight posts like this one (a filter)";
    ShowHidden, "show_hidden", Some(Key::char('Z')), &[Scope::Catalog, Scope::Thread], "show hidden threads and posts";
    ImageSearch, "image_search", Some(Key::char('R')), &[Scope::Thread, Scope::Viewer], "reverse image search";
    Gallery, "gallery", Some(Key::char('V')), &[Scope::Thread, Scope::Gallery], "the thread's files as a grid";
    Export, "export", None, &[Scope::Thread, Scope::Gallery], "save the thread as a page, HTML and JSON (asks first)";
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
    Copy, "copy", Some(Key::char('y')), &[Scope::Catalog, Scope::Thread, Scope::Saved, Scope::Gallery, Scope::Viewer], "copy the text (viewer: file URL)";
    CopyLink, "copy_link", Some(Key::char('Y')), &[Scope::Catalog, Scope::Thread, Scope::Saved, Scope::Gallery, Scope::Viewer], "copy the link";
}

impl Action {
    pub fn name(self) -> &'static str {
        self.spec().0
    }

    pub fn default_key(self) -> Option<Key> {
        self.spec().1
    }

    pub fn scopes(self) -> &'static [Scope] {
        self.spec().2
    }

    pub fn what(self) -> &'static str {
        self.spec().3
    }

    /// Moves repeat with a count (`3n`); anything else (a toggle) happens once.
    pub fn repeats(self) -> bool {
        matches!(self, Action::NextMatch | Action::PrevMatch | Action::JumpBack | Action::JumpForward | Action::NextTab | Action::PrevTab | Action::NextPart | Action::PrevPart)
    }
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Action::ALL.iter().copied().find(|a| a.name() == name).ok_or_else(|| {
            let known: Vec<_> = Action::ALL.iter().map(|a| a.name()).collect();
            D::Error::custom(format!("unknown action `{name}` (known: {})", known.join(", ")))
        })
    }
}

/// A key with its modifiers: `w`, `W`, `ctrl-w`, `alt-x`, `tab`, `shift-tab`, `f5`, ...
/// Read from a terminal event or parsed, the same way: shift is part of a character (`W`)
/// and only kept for tab (`shift-tab`), and ctrl-i is tab, as terminals send it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    code: KeyCode,
    mods: KeyModifiers,
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
    const fn char(c: char) -> Self {
        Self { code: KeyCode::Char(c), mods: KeyModifiers::NONE }
    }

    const fn ctrl(c: char) -> Self {
        Self { code: KeyCode::Char(c), mods: KeyModifiers::CONTROL }
    }

    const fn code(code: KeyCode) -> Self {
        Self { code, mods: KeyModifiers::NONE }
    }

    fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        match (code, mods & (KeyModifiers::CONTROL | KeyModifiers::ALT)) {
            (KeyCode::Char('i'), KeyModifiers::CONTROL) => Self::code(KeyCode::Tab),
            (code, mods) => Self { code, mods },
        }
    }
}

impl From<KeyEvent> for Key {
    fn from(ev: KeyEvent) -> Self {
        let code = match ev.code {
            KeyCode::Tab if ev.modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
            c => c,
        };
        Self::new(code, ev.modifiers)
    }
}

impl From<Key> for KeyEvent {
    fn from(key: Key) -> Self {
        KeyEvent::new(key.code, key.mods)
    }
}

impl FromStr for Key {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
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
        match Self::new(code, mods) {
            key if key == Self::ctrl('c') => bail!("ctrl-c always quits"),
            key => Ok(key),
        }
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

/// What the fixed keys do: move around, and start a count (`10j`) or a two-key command (`gg`,
/// `zz`). Each place takes them its own way: in the catalog grid left is a column, in the
/// image viewer the file before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Down,
    Up,
    Left,
    Right,
    Open,
    /// Back out a step.
    Esc,
    Back,
    LineDown,
    LineUp,
    HalfDown,
    HalfUp,
    PageDown,
    PageUp,
    Top,
    Bottom,
    /// `gg` is the top; after a count, `g` is that entry.
    G,
    /// `zz`, `zt` and `zb` put the post at the screen's middle, top or bottom.
    Z,
    /// A count's (on the home screen, a favorite's).
    Digit,
    Refresh,
    ZoomIn,
    ZoomOut,
    Fit,
    /// Pause an animated GIF, or the next file.
    Play,
}

const EVERYWHERE: &[Scope] = &[Scope::Global, Scope::Gallery, Scope::Viewer];
const LISTS: &[Scope] = &[Scope::Lists, Scope::Catalog, Scope::Saved];

/// The fixed keys, which no command may have where they work, as the commands are listed:
/// where, what they do, their keys, and what help says.
const FIXED: &[(&[Scope], Nav, &[Key], &str)] = &[
    (EVERYWHERE, Nav::Down, &[Key::char('j'), Key::code(KeyCode::Down)], "down (10j: ten)"),
    (EVERYWHERE, Nav::Up, &[Key::char('k'), Key::code(KeyCode::Up)], "up"),
    (EVERYWHERE, Nav::Left, &[Key::char('h'), Key::code(KeyCode::Left)], "back (in a grid: left)"),
    (EVERYWHERE, Nav::Right, &[Key::char('l'), Key::code(KeyCode::Right)], "open (in a grid: right)"),
    (EVERYWHERE, Nav::Open, &[Key::code(KeyCode::Enter)], "open (in a thread: follow a quote)"),
    (EVERYWHERE, Nav::Esc, &[Key::code(KeyCode::Esc)], "back, close; drops a count"),
    (&[Scope::Global], Nav::Back, &[Key::code(KeyCode::Backspace)], "back"),
    (&[Scope::Global], Nav::Refresh, &[Key::code(KeyCode::F(5))], "reload"),
    (&[Scope::Global], Nav::Top, &[Key::code(KeyCode::Home)], "top"),
    (&[Scope::Global], Nav::G, &[Key::char('g')], "gg: top (10gg, 10g: the tenth)"),
    (&[Scope::Global], Nav::Bottom, &[Key::char('G'), Key::code(KeyCode::End)], "bottom (10G: the tenth)"),
    (&[Scope::Global], Nav::Digit, &[Key::char('0'), Key::char('1'), Key::char('2'), Key::char('3'), Key::char('4'), Key::char('5'), Key::char('6'), Key::char('7'), Key::char('8'), Key::char('9')], "a count (home: a favorite)"),
    (&[Scope::Global], Nav::HalfDown, &[Key::ctrl('d')], "half a page down (a list: 10 rows)"),
    (&[Scope::Global], Nav::HalfUp, &[Key::ctrl('u')], "half a page up"),
    (&[Scope::Global], Nav::PageDown, &[Key::ctrl('f')], "a page down (a list: 20 rows)"),
    (&[Scope::Global], Nav::PageUp, &[Key::ctrl('b')], "a page up"),
    (&[Scope::Global], Nav::LineDown, &[Key::ctrl('e')], "a line down (a list: a row)"),
    (&[Scope::Global], Nav::LineUp, &[Key::ctrl('y')], "a line up"),
    (LISTS, Nav::HalfDown, &[Key::code(KeyCode::PageDown)], "10 rows down"),
    (LISTS, Nav::HalfUp, &[Key::code(KeyCode::PageUp)], "10 rows up"),
    (&[Scope::Thread], Nav::PageDown, &[Key::char(' '), Key::code(KeyCode::PageDown)], "a page down"),
    (&[Scope::Thread], Nav::PageUp, &[Key::code(KeyCode::PageUp)], "a page up"),
    (&[Scope::Thread], Nav::LineDown, &[Key::char('J')], "a line down"),
    (&[Scope::Thread], Nav::LineUp, &[Key::char('K')], "a line up"),
    (&[Scope::Thread], Nav::Z, &[Key::char('z')], "zz zt zb: the post to the middle, top, bottom"),
    (&[Scope::Gallery], Nav::Esc, &[Key::char('q')], "back to the thread"),
    (&[Scope::Gallery], Nav::Back, &[Key::code(KeyCode::Backspace)], "back to the thread"),
    (&[Scope::Gallery], Nav::Top, &[Key::char('g'), Key::code(KeyCode::Home)], "the first file"),
    (&[Scope::Gallery], Nav::Bottom, &[Key::char('G'), Key::code(KeyCode::End)], "the last file"),
    (&[Scope::Gallery], Nav::PageDown, &[Key::code(KeyCode::PageDown)], "three rows down"),
    (&[Scope::Gallery], Nav::PageUp, &[Key::code(KeyCode::PageUp)], "three rows up"),
    (&[Scope::Viewer], Nav::Esc, &[Key::char('q'), Key::char('v')], "close"),
    (&[Scope::Viewer], Nav::PageDown, &[Key::code(KeyCode::PageDown)], "the next file"),
    (&[Scope::Viewer], Nav::PageUp, &[Key::code(KeyCode::PageUp)], "the previous file"),
    (&[Scope::Viewer], Nav::ZoomIn, &[Key::char('+'), Key::char('=')], "zoom in (then h/j/k/l move)"),
    (&[Scope::Viewer], Nav::ZoomOut, &[Key::char('-')], "zoom out"),
    (&[Scope::Viewer], Nav::Fit, &[Key::char('0')], "fit the screen"),
    (&[Scope::Viewer], Nav::Play, &[Key::char(' ')], "pause an animated GIF, or the next file"),
];

/// What a key does somewhere: a fixed move, or a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Nav(Nav),
    Act(Action),
}

impl Command {
    fn name(self) -> &'static str {
        match self {
            Command::Nav(_) => "navigation",
            Command::Act(a) => a.name(),
        }
    }
}

/// The commands' keys, and what every key does in each place: made together by `new`, which
/// refuses a key that would do two things in one place.
#[derive(Debug, Clone)]
pub struct KeyMap {
    keys: HashMap<Action, Vec<Key>>,
    commands: HashMap<(Scope, Key), Command>,
}

impl Default for KeyMap {
    fn default() -> Self {
        // The defaults don't clash (a test checks).
        Self::new(HashMap::new()).unwrap_or_else(|_| Self { keys: HashMap::new(), commands: HashMap::new() })
    }
}

impl KeyMap {
    /// These keys, and the defaults for the actions not given (`[]`: no key, in the menu
    /// only). Refused if a key would do two things in one place.
    pub fn new(mut keys: HashMap<Action, Vec<Key>>) -> Result<Self> {
        for &action in Action::ALL {
            keys.entry(action).or_insert_with(|| action.default_key().into_iter().collect());
        }
        let fixed = FIXED.iter().map(|&(scopes, nav, keys, _)| (scopes, Command::Nav(nav), keys));
        let acts = Action::ALL.iter().map(|&a| (a.scopes(), Command::Act(a), keys.get(&a).map_or(&[][..], Vec::as_slice)));
        let mut commands = HashMap::new();
        for place in PLACES {
            for (_, command, keys) in fixed.clone().chain(acts.clone()).filter(|(scopes, ..)| scopes.iter().any(|s| s.covers(place))) {
                for &key in keys {
                    if let Some(other) = commands.insert((place, key), command).filter(|&other| other != command) {
                        bail!("`{}` and `{}` both use '{key}' in the {} view", command.name(), other.name(), place.label());
                    }
                }
            }
        }
        Ok(Self { keys, commands })
    }

    /// What a key does in `place`.
    pub fn command(&self, place: Scope, key: impl Into<Key>) -> Option<Command> {
        self.commands.get(&(place, key.into())).copied()
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
        self.keys(action) == action.default_key().as_slice()
    }

    /// A copy with an action's keys replaced (`None`: the default), if that leaves no
    /// conflicts.
    pub fn with(&self, action: Action, keys: Option<Vec<Key>>) -> Result<Self> {
        let mut all = self.keys.clone();
        match keys {
            Some(keys) => all.insert(action, keys),
            None => all.remove(&action),
        };
        Self::new(all)
    }

    /// What `[keys]` holds for an action: `None` when it has the default.
    pub fn binding(&self, action: Action) -> Option<&[Key]> {
        (!self.is_default(action)).then(|| self.keys(action))
    }
}

/// `[keys]`: `watch = "W"` or `watch = ["W", "alt-w"]`; `watch = []` for none. Checked as it's
/// read: a config whose keys clash doesn't load.
impl<'de> Deserialize<'de> for KeyMap {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Spec {
            One(String),
            Many(Vec<String>),
        }
        let mut keys = HashMap::new();
        for (action, spec) in BTreeMap::<Action, Spec>::deserialize(d)? {
            let specs = match spec {
                Spec::One(s) => vec![s],
                Spec::Many(v) => v,
            };
            let parsed = specs.iter().map(|s| s.parse()).collect::<Result<_>>();
            keys.insert(action, parsed.map_err(|e| D::Error::custom(format!("`{}`: {e}", action.name())))?);
        }
        Self::new(keys).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `[keys]` table of these, as the config's is read.
    fn map(pairs: &[(&str, &str)]) -> Result<KeyMap, toml::de::Error> {
        toml::from_str(&pairs.iter().map(|(a, k)| format!("{a} = {k:?}\n")).collect::<String>())
    }

    /// What a character does in `place`.
    fn on(m: &KeyMap, place: Scope, c: char) -> Option<Command> {
        m.command(place, KeyEvent::from(KeyCode::Char(c)))
    }

    #[test]
    fn defaults_are_consistent() {
        let m = KeyMap::new(HashMap::new()).unwrap();
        assert_eq!(on(&m, Scope::Catalog, 's'), Some(Command::Act(Action::Sort)));
        assert_eq!(on(&m, Scope::Thread, 's'), Some(Command::Act(Action::Spoiler)));
        assert_eq!(on(&m, Scope::Saved, 'q'), Some(Command::Act(Action::Quit)));
        assert_eq!(on(&m, Scope::Lists, 'z'), None);
        assert_eq!(on(&m, Scope::Thread, 'z'), Some(Command::Nav(Nav::Z)));
        // Global keys don't apply in the image viewer and gallery, which have their own.
        assert_eq!(on(&m, Scope::Viewer, 'q'), Some(Command::Nav(Nav::Esc)));
        assert_eq!(on(&m, Scope::Gallery, 'W'), None);
        // Shift is part of the character.
        assert_eq!(m.command(Scope::Thread, KeyEvent::new(KeyCode::Char('U'), KeyModifiers::SHIFT)), Some(Command::Act(Action::Unread)));
    }

    #[test]
    fn fixed_keys_cant_be_rebound() {
        // Counts, prefixes, vim's scrolls, and the gallery's q (with quit moved).
        for pairs in [[("watched", "z")], [("sort", "3")], [("expand", "0")], [("jump_back", "ctrl-f")], [("reload", "ctrl-e")], [("preview", "j")], [("preview", "ctrl-d")]] {
            assert!(map(&pairs).unwrap_err().to_string().contains("navigation"), "{pairs:?}");
        }
        assert!(map(&[("quit", "Q"), ("view", "q")]).is_err());
    }

    #[test]
    fn the_manuals_example_loads() {
        let doc = include_str!("../docs/manual.md");
        let block = doc.split("```toml\n[keys]").nth(1).and_then(|b| b.split("```").next()).unwrap();
        let cfg: crate::config::Config = toml::from_str(&format!("[keys]{block}")).unwrap();
        assert_eq!(cfg.keys.keys(Action::Watch), [Key::char('Q')]);
    }

    #[test]
    fn overrides_and_errors() {
        let m = map(&[("watch", "Q")]).unwrap();
        assert_eq!(on(&m, Scope::Thread, 'Q'), Some(Command::Act(Action::Watch)));
        assert_eq!(on(&m, Scope::Thread, 'w'), None);

        let err = |pairs| map(pairs).unwrap_err().to_string();
        assert!(err(&[("nope", "z")]).contains("unknown action `nope`"));
        assert!(err(&[("watch", "ctrl-")]).contains("`watch`: `ctrl-` isn't a key"));
        assert!(err(&[("watch", "hyper-w")]).contains("isn't a key"));
        assert!(err(&[("watch", "ctrl-c")]).contains("ctrl-c always quits"));
        assert!(err(&[("sort", "c")]).contains("both use 'c' in the catalog view"));
        // A global key can't shadow a view's key, nor ctrl-i tab (to a terminal they're one).
        assert!(err(&[("reload", "v")]).contains("'v'"));
        assert!(err(&[("jump_forward", "ctrl-i")]).contains("'tab'"));
    }

    #[test]
    fn key_specs_round_trip() {
        for s in ["w", "W", ":", ",", "ctrl-w", "alt-x", "ctrl-alt-k", "tab", "shift-tab", "f5", "f12", "pageup", "space", "delete"] {
            assert_eq!(s.parse::<Key>().unwrap().to_string(), s, "{s}");
        }
        assert_eq!("Tab".parse::<Key>().unwrap(), Key::code(KeyCode::Tab));
        assert!(["f13", "", "ww"].iter().all(|s| s.parse::<Key>().is_err()));
        // Terminal events map onto the same keys.
        for (ev, s) in [(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT), "shift-tab"), (KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL), "ctrl-w"), (KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL), "ctrl-i")] {
            assert_eq!(Key::from(ev), s.parse().unwrap());
        }
    }

    #[test]
    fn several_keys_per_action() {
        let m: KeyMap = toml::from_str("watch = [\"Q\", \"alt-w\"]").unwrap();
        assert_eq!(on(&m, Scope::Catalog, 'Q'), Some(Command::Act(Action::Watch)));
        assert_eq!(m.command(Scope::Catalog, KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT)), Some(Command::Act(Action::Watch)));
        assert_eq!(m.label(Action::Watch), "Q, alt-w");
        assert_eq!(m.binding(Action::Watch), Some(["Q".parse().unwrap(), "alt-w".parse().unwrap()].as_slice()));
        assert_eq!(m.binding(Action::Sort), None);
        // Conflicts are checked across every key of every action.
        let bad = toml::from_str::<KeyMap>("sort = [\"z\", \"W\"]\nwatch = \"W\"");
        assert!(bad.unwrap_err().to_string().contains("'W'"));
        // Editing: a conflicting change is refused, a reset restores the default.
        assert!(m.with(Action::Sort, Some(vec![Key::char('v')])).is_err());
        let m = m.with(Action::Watch, None).unwrap();
        assert!(m.is_default(Action::Watch));
    }

    #[test]
    fn actions_without_keys() {
        // Saving all the thread's files or the page has no key by default: it's in the menu.
        let m = KeyMap::default();
        assert!(m.keys(Action::Export).is_empty() && m.key(Action::Export).is_empty());
        assert_eq!(on(&m, Scope::Thread, 'E'), None);
        assert_eq!(m.how(Action::Export), "the . menu");
        assert_eq!(m.how(Action::Download), "d");
        assert_eq!(m.binding(Action::Export), None);
        // `[]` takes a key away; it's kept as `[]`, and the key is free.
        let none: KeyMap = toml::from_str("download = []").unwrap();
        assert_eq!(on(&none, Scope::Thread, 'd'), None);
        assert_eq!(none.binding(Action::Download), Some([].as_slice()));
        assert!(map(&[("watch", "d")]).unwrap_err().to_string().contains("`download` and `watch` both use 'd' in the thread view"));
        assert_eq!(on(&none.with(Action::Watch, Some(vec![Key::char('d')])).unwrap(), Scope::Thread, 'd'), Some(Command::Act(Action::Watch)));
        // Given a key, it works like any other.
        let m = map(&[("export", "E")]).unwrap();
        assert_eq!(on(&m, Scope::Thread, 'E'), Some(Command::Act(Action::Export)));
        let m = map(&[("menu", "alt-m")]).unwrap().with(Action::Menu, Some(vec![])).unwrap();
        assert_eq!(m.how(Action::Export), "the menu (right-click)");
    }
}
