//! Every key the views take, from two tables: the fixed keys that move around (`FIXED`), and
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
            Scope::Global => "Everywhere",
            Scope::Lists => "Lists",
            Scope::Catalog => "Catalog",
            Scope::Thread => "Thread",
            Scope::Saved => "Watched and History",
            Scope::Gallery => "Gallery",
            Scope::Viewer => "Image viewer",
        }
    }

    /// Whether a command for `self` applies in `place`.
    fn covers(self, place: Scope) -> bool {
        self == place || (self == Scope::Global && !matches!(place, Scope::Gallery | Scope::Viewer))
    }
}

/// In the order help and the key editor list them. Keys go to all but `Global`, which stands
/// for most of them.
pub const SCOPES: [Scope; 7] = [Scope::Global, Scope::Lists, Scope::Catalog, Scope::Thread, Scope::Saved, Scope::Gallery, Scope::Viewer];

/// Where an action is to be had by default: on a key of its own where it works, or on a
/// letter in a menu (the `.` menu, or for what the view shows, the `c` menu), the less used
/// ones. Given a key in `[keys]`, an action's on that key, and its menu row shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bind {
    Key(Key),
    Menu(Key),
}

const fn key(c: char) -> Bind {
    Bind::Key(Key::char(c))
}

const fn menu(c: char) -> Bind {
    Bind::Menu(Key::char(c))
}

/// Declares `Action`, every command with its config name, default key or menu letter, scopes,
/// and what it does, from one list.
macro_rules! actions {
    ($($action:ident, $name:literal, $bind:expr, $scopes:expr, $what:literal;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Action {
            $($action,)*
        }

        impl Action {
            pub const ALL: &[Action] = &[$(Action::$action,)*];

            /// Its name in `[keys]`, its default key or letter, where it applies, and what it
            /// does.
            const fn spec(self) -> (&'static str, Bind, &'static [Scope], &'static str) {
                match self {
                    $(Action::$action => ($name, $bind, $scopes, $what),)*
                }
            }
        }
    };
}

actions! {
    Quit, "quit", key('q'), &[Scope::Global], "quit";
    Help, "help", key('?'), &[Scope::Global, Scope::Gallery, Scope::Viewer], "help";
    Settings, "settings", key(','), &[Scope::Global], "settings: theme, colors, keys, …";
    Search, "search", key('/'), &[Scope::Global], "filter the list; search a thread";
    Reload, "reload", key('R'), &[Scope::Global], "reload";
    Browser, "browser", key('o'), &[Scope::Global], "open in the browser";
    Goto, "goto", key(':'), &[Scope::Global], "go to a URL or site/board/thread";
    NextTab, "next_tab", key(']'), &[Scope::Global], "next tab";
    PrevTab, "prev_tab", key('['), &[Scope::Global], "previous tab";
    Menu, "menu", key('.'), &[Scope::Global, Scope::Gallery, Scope::Viewer], "everything you can do with what's selected";
    Hints, "hints", key('f'), &[Scope::Global], "label what's on screen; type a label to open it";
    Watched, "watched", key('W'), &[Scope::Global], "the watched threads";
    CloseTab, "close_tab", Bind::Key(Key::ctrl('w')), &[Scope::Global], "close the tab";
    NextPart, "next_part", Bind::Key(Key::code(KeyCode::Tab)), &[Scope::Thread], "focus the post's next image or link (then the next post's)";
    PrevPart, "prev_part", Bind::Key(Key::code(KeyCode::BackTab)), &[Scope::Thread], "focus the previous image or link";
    Reply, "reply", key('r'), &[Scope::Catalog, Scope::Thread], "post: reply to the thread (quoting the post), or start one in the catalog";
    Watch, "watch", key('w'), &[Scope::Catalog, Scope::Thread], "watch / unwatch the thread";
    Show, "show", key('c'), &[Scope::Catalog, Scope::Thread], "what the view shows: its layout and order, or which posts";
    Preview, "preview", key('p'), &[Scope::Thread], "preview the quoted posts";
    NextMatch, "next_match", key('n'), &[Scope::Thread, Scope::Lists], "next search match (archive search: more results)";
    PrevMatch, "prev_match", key('N'), &[Scope::Thread], "previous search match";
    Gallery, "gallery", key('V'), &[Scope::Thread, Scope::Gallery], "the thread's files as a grid";
    Download, "download", key('d'), &[Scope::Thread, Scope::Gallery, Scope::Viewer], "save what's in front of you: the focused or viewed file, else the post's";
    Copy, "copy", key('y'), &[Scope::Catalog, Scope::Thread, Scope::Saved, Scope::Gallery, Scope::Viewer], "copy the text (a focused link or file: its URL)";
    Favorite, "favorite", key('*'), &[Scope::Lists, Scope::Catalog], "favorite board: on / off";
    Remove, "remove", key('x'), &[Scope::Saved, Scope::Lists], "remove the entry (home: a favorite)";
    JumpBack, "jump_back", menu('u'), &[Scope::Global], "back to the thread you left";
    JumpForward, "jump_forward", Bind::Menu(Key::ctrl('r')), &[Scope::Global], "forward again, after going back";
    View, "view", menu('v'), &[Scope::Catalog, Scope::Thread, Scope::Gallery], "view the post's images";
    OpenFile, "open_file", menu('i'), &[Scope::Thread, Scope::Viewer], "open the file outside ck (videos in mpv)";
    Replies, "replies", menu('b'), &[Scope::Thread], "jump to the first reply";
    Unread, "unread", menu('U'), &[Scope::Thread], "jump to the first unread post";
    Expand, "expand", menu('e'), &[Scope::Thread], "show / hide the post's replies under it";
    Spoiler, "spoiler", menu('s'), &[Scope::Thread], "show the post's spoilers";
    Mine, "mine", menu('m'), &[Scope::Thread], "mark the post as yours (notified of replies)";
    Quote, "quote", menu('Q'), &[Scope::Thread], "reply quoting the post's text";
    NewTab, "new_tab", menu('T'), &[Scope::Catalog, Scope::Thread, Scope::Saved], "open the thread (or link) in a new tab";
    Follow, "follow", menu('F'), &[Scope::Catalog, Scope::Thread, Scope::Saved], "follow as a general: watch its next thread";
    CopyLink, "copy_link", menu('Y'), &[Scope::Catalog, Scope::Thread, Scope::Saved, Scope::Gallery, Scope::Viewer], "copy the link";
    Links, "links", menu('O'), &[Scope::Catalog, Scope::Thread], "the post's links and files";
    Hide, "hide", menu('H'), &[Scope::Catalog, Scope::Thread], "hide / unhide the thread or post";
    Filter, "filter", menu('X'), &[Scope::Catalog, Scope::Thread], "hide or highlight posts like this one (a filter)";
    ImageSearch, "image_search", menu('L'), &[Scope::Thread, Scope::Viewer], "reverse image search";
    DownloadThread, "download_thread", menu('D'), &[Scope::Thread, Scope::Gallery], "save all the thread's files (asks first)";
    Export, "export", menu('E'), &[Scope::Thread, Scope::Gallery], "save the thread as a page, HTML and JSON (asks first)";
    Archive, "archive", menu('a'), &[Scope::Thread], "open a 404'd thread in the archive";
    ArchiveSearch, "archive_search", menu('A'), &[Scope::Catalog], "search the board's archive";
    BoardImages, "board_images", menu('B'), &[Scope::Lists, Scope::Catalog, Scope::Thread], "images on this board: on / off";
    AddSite, "add_site", menu('a'), &[Scope::Lists], "add a site from a link to any page of it";
    UpdateBoards, "update_boards", menu('U'), &[Scope::Lists], "read a vichan site's board list again from its pages";
    SearchSaved, "search_saved", menu('s'), &[Scope::Saved], "search inside the saved threads (also :saved WORDS)";
    Conversation, "conversation", menu('C'), &[Scope::Thread], "the post's conversation alone: what it replies to, and its replies";
    Poster, "poster", menu('I'), &[Scope::Thread], "the post's poster's posts alone (by poster ID)";
    Media, "media", menu('M'), &[Scope::Thread], "all posts, only those with files, or all with images hidden (in turn)";
    AllSpoilers, "all_spoilers", menu('S'), &[Scope::Thread], "show all spoilers";
    ShowHidden, "show_hidden", menu('Z'), &[Scope::Catalog, Scope::Thread], "show hidden threads and posts";
    Sort, "sort", menu('s'), &[Scope::Catalog], "cycle the sort order";
    Compact, "compact", menu('l'), &[Scope::Catalog], "layout: cards, compact, grid";
}

impl Action {
    pub fn name(self) -> &'static str {
        self.spec().0
    }

    pub fn default_key(self) -> Option<Key> {
        match self.spec().1 {
            Bind::Key(k) => Some(k),
            Bind::Menu(_) => None,
        }
    }

    /// Its letter in its menu, when it has no key of its own by default.
    fn letter(self) -> Option<Key> {
        match self.spec().1 {
            Bind::Key(_) => None,
            Bind::Menu(k) => Some(k),
        }
    }

    /// Whether it's in the `c` menu, of what the view shows, rather than the `.` menu.
    pub fn shows(self) -> bool {
        matches!(self, Action::Conversation | Action::Poster | Action::Media | Action::AllSpoilers | Action::ShowHidden | Action::Sort | Action::Compact)
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
    (EVERYWHERE, Nav::Open, &[Key::code(KeyCode::Enter)], "open (a post: its images, else its quote)"),
    (EVERYWHERE, Nav::Esc, &[Key::code(KeyCode::Esc)], "back the way you came, close; drops a count"),
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

/// The commands' keys, and what every key does in each place and in its menus: made together
/// by `new`, which refuses a key that would do two things in one place.
#[derive(Debug, Clone)]
pub struct KeyMap {
    keys: HashMap<Action, Vec<Key>>,
    commands: HashMap<(Scope, Key), Command>,
    menus: HashMap<(Scope, Key), Action>,
    /// The letters in use: those of actions without keys of their own, where no key is.
    letters: HashMap<Action, Key>,
}

impl Default for KeyMap {
    fn default() -> Self {
        // The defaults don't clash (a test checks).
        Self::new(HashMap::new()).unwrap_or_else(|_| Self { keys: HashMap::new(), commands: HashMap::new(), menus: HashMap::new(), letters: HashMap::new() })
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
        let name = |c| match c {
            Command::Nav(_) => "navigation",
            Command::Act(a) => Action::name(a),
        };
        let mut commands = HashMap::new();
        let mut menus = HashMap::new();
        for place in SCOPES.into_iter().filter(|&s| s != Scope::Global) {
            for (_, command, keys) in fixed.clone().chain(acts.clone()).filter(|(scopes, ..)| scopes.iter().any(|s| s.covers(place))) {
                for &key in keys {
                    if let Some(other) = commands.insert((place, key), command).filter(|&other| other != command) {
                        bail!("`{}` and `{}` both use '{key}' in the {} view", name(command), name(other), place.label());
                    }
                    if let Command::Act(a) = command {
                        menus.insert((place, key), a);
                    }
                }
            }
        }
        // In a menu, an action without a key of its own is on its letter, unless a key is
        // there in any place it applies (given in `[keys]`: that wins).
        let applies = |a: Action| SCOPES.into_iter().filter(move |&p| p != Scope::Global && a.scopes().iter().any(|s| s.covers(p)));
        let mut letters = HashMap::new();
        for &a in Action::ALL.iter().filter(|a| keys.get(a).is_none_or(Vec::is_empty)) {
            if let Some(letter) = a.letter().filter(|&l| applies(a).all(|p| !menus.contains_key(&(p, l)))) {
                menus.extend(applies(a).map(|p| ((p, letter), a)));
                letters.insert(a, letter);
            }
        }
        Ok(Self { keys, commands, menus, letters })
    }

    /// Every key that does something somewhere, in the tables' order.
    #[cfg(test)]
    pub fn every_key(&self) -> impl Iterator<Item = KeyEvent> {
        FIXED.iter().flat_map(|f| f.2).chain(Action::ALL.iter().flat_map(|&a| self.keys(a))).chain(self.letters.values()).map(|k| KeyEvent::new(k.code, k.mods))
    }

    /// What a key does in `place`.
    pub fn command(&self, place: Scope, key: impl Into<Key>) -> Option<Command> {
        self.commands.get(&(place, key.into())).copied()
    }

    /// The action a key runs in a menu in `place`: by its key, or its letter.
    pub fn in_menu(&self, place: Scope, key: impl Into<Key>) -> Option<Action> {
        self.menus.get(&(place, key.into())).copied()
    }

    /// The key that runs an action in its menu, as its row shows it; empty when it has none.
    pub fn menu_key(&self, action: Action) -> String {
        self.keys(action).first().copied().or_else(|| self.letter(action)).map(|k| k.to_string()).unwrap_or_default()
    }

    /// An action's letter, while it has no key of its own.
    fn letter(&self, action: Action) -> Option<Key> {
        self.letters.get(&action).copied()
    }

    /// The first key of an action, as shown in hints; empty when it has none.
    pub fn key(&self, action: Action) -> String {
        self.keys(action).first().map(Key::to_string).unwrap_or_default()
    }

    /// All of an action's keys, for help; empty when it has none.
    pub fn label(&self, action: Action) -> String {
        self.keys(action).iter().map(Key::to_string).collect::<Vec<_>>().join(", ")
    }

    /// How to get to an action, for messages and help: its key, its menu's key then its
    /// letter (`. h`), or its menu.
    pub fn how(&self, action: Action) -> String {
        let menu = self.keys(if action.shows() { Action::Show } else { Action::Menu }).first();
        match (self.keys(action).first(), menu, self.letter(action)) {
            (Some(k), ..) => k.to_string(),
            (None, Some(m), Some(l)) => format!("{m} {l}"),
            (None, Some(m), None) => format!("the {m} menu"),
            (None, None, _) => "the menu (right-click)".into(),
        }
    }

    /// An action's keys; empty when it has none.
    pub fn keys(&self, action: Action) -> &[Key] {
        self.keys.get(&action).map_or(&[], Vec::as_slice)
    }

    pub fn is_default(&self, action: Action) -> bool {
        self.keys(action) == action.default_key().as_slice()
    }

    /// Help's rows for `scope`, with its fixed keys first: what works there first of all, or
    /// with `all`, all that does but what works everywhere. Those in a menu come last, as
    /// their menu's key then their letter (`. h`).
    pub fn help(&self, scope: Scope, all: bool) -> Vec<(String, &'static str)> {
        let listed = |scopes: &[Scope]| scopes.first() == Some(&scope) || (all && scopes.first() != Some(&Scope::Global) && scopes.contains(&scope));
        let fixed = FIXED.iter().filter(|f| listed(f.0)).map(|&(_, _, keys, what)| match keys {
            [first, .., last] if keys.len() > 3 => (format!("{first}-{last}"), what),
            _ => (keys.iter().map(Key::to_string).collect::<Vec<_>>().join(", "), what),
        });
        let acts = Action::ALL.iter().filter(|a| listed(a.scopes()));
        let (direct, in_menus): (Vec<Action>, Vec<Action>) = acts.partition(|&&a| !self.keys(a).is_empty());
        let in_menus = in_menus.into_iter().filter(|&a| self.letter(a).is_some()).map(|a| (self.how(a), a.what()));
        fixed.chain(direct.into_iter().map(|a| (self.label(a), a.what()))).chain(in_menus).collect()
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
        assert_eq!(on(&m, Scope::Catalog, 'c'), Some(Command::Act(Action::Show)));
        assert_eq!(on(&m, Scope::Thread, 'r'), Some(Command::Act(Action::Reply)));
        assert_eq!(on(&m, Scope::Saved, 'q'), Some(Command::Act(Action::Quit)));
        // The less used are on letters in their menu, the same letter in different places.
        assert_eq!(on(&m, Scope::Thread, 's'), None);
        assert_eq!(m.in_menu(Scope::Catalog, KeyEvent::from(KeyCode::Char('s'))), Some(Action::Sort));
        assert_eq!(m.in_menu(Scope::Thread, KeyEvent::from(KeyCode::Char('s'))), Some(Action::Spoiler));
        assert_eq!(m.in_menu(Scope::Thread, KeyEvent::from(KeyCode::Char('w'))), Some(Action::Watch));
        assert_eq!((m.how(Action::Hide), m.how(Action::Sort), m.how(Action::Watch)), (". H".into(), "c s".into(), "w".into()));
        assert_eq!(on(&m, Scope::Lists, 'z'), None);
        assert_eq!(on(&m, Scope::Thread, 'z'), Some(Command::Nav(Nav::Z)));
        // Global keys don't apply in the image viewer and gallery, which have their own.
        assert_eq!(on(&m, Scope::Viewer, 'q'), Some(Command::Nav(Nav::Esc)));
        assert_eq!(on(&m, Scope::Gallery, 'W'), None);
        // Shift is part of the character.
        assert_eq!(m.command(Scope::Thread, KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT)), Some(Command::Act(Action::PrevMatch)));
    }

    #[test]
    fn every_default_letter_works() {
        // None gives way to a key or another letter, nor is one the menus move or close with.
        let m = KeyMap::default();
        for &a in Action::ALL {
            assert!(a.default_key().is_some() || m.letter(a).is_some(), "{a:?} has no letter");
            assert!(m.letter(a).is_none_or(|l| !"jkgGq".chars().any(|c| l == Key::char(c))), "{a:?}");
        }
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
        // A key given wins over a menu letter, which then isn't one.
        assert_eq!(m.in_menu(Scope::Thread, KeyEvent::from(KeyCode::Char('Q'))), Some(Action::Watch));
        assert_eq!(m.how(Action::Quote), "the . menu");

        let err = |pairs| map(pairs).unwrap_err().to_string();
        assert!(err(&[("nope", "z")]).contains("unknown action `nope`"));
        assert!(err(&[("watch", "ctrl-")]).contains("`watch`: `ctrl-` isn't a key"));
        assert!(err(&[("watch", "hyper-w")]).contains("isn't a key"));
        assert!(err(&[("watch", "ctrl-c")]).contains("ctrl-c always quits"));
        assert!(err(&[("sort", "c")]).contains("both use 'c' in the Catalog view"));
        // A global key can't shadow a view's key, nor ctrl-i tab (to a terminal they're one).
        assert!(err(&[("reload", "V")]).contains("'V'"));
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
        assert!(m.with(Action::Sort, Some(vec![Key::char('c')])).is_err());
        let m = m.with(Action::Watch, None).unwrap();
        assert!(m.is_default(Action::Watch));
    }

    #[test]
    fn actions_without_keys() {
        // Saving all the thread's files or the page has no key by default: it's in the menu.
        let m = KeyMap::default();
        assert!(m.keys(Action::Export).is_empty() && m.key(Action::Export).is_empty());
        assert_eq!(on(&m, Scope::Thread, 'E'), None);
        assert_eq!((m.how(Action::Export), m.menu_key(Action::Export)), (". E".into(), "E".into()));
        assert_eq!(m.how(Action::Download), "d");
        assert_eq!(m.binding(Action::Export), None);
        // `[]` takes a key away; it's kept as `[]`, and the key is free.
        let none: KeyMap = toml::from_str("download = []").unwrap();
        assert_eq!(on(&none, Scope::Thread, 'd'), None);
        assert_eq!(none.binding(Action::Download), Some([].as_slice()));
        assert!(map(&[("watch", "d")]).unwrap_err().to_string().contains("`download` and `watch` both use 'd' in the Thread view"));
        assert_eq!(on(&none.with(Action::Watch, Some(vec![Key::char('d')])).unwrap(), Scope::Thread, 'd'), Some(Command::Act(Action::Watch)));
        // Given a key, it works like any other.
        let m = map(&[("export", "E")]).unwrap();
        assert_eq!(on(&m, Scope::Thread, 'E'), Some(Command::Act(Action::Export)));
        let m = map(&[("menu", "alt-m")]).unwrap().with(Action::Menu, Some(vec![])).unwrap();
        assert_eq!(m.how(Action::Export), "the menu (right-click)");
    }
}
