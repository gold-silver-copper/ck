//! Remappable single-key commands, configured in the `[keys]` section of config.toml.
//! Navigation (j/k/g/G/h/l/J/K, arrows, enter, esc, space, ctrl-d/u) is fixed.

use std::collections::HashMap;

use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Help,
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
}

/// Where a key applies. Global keys work in every view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    /// Sites and Boards.
    Lists,
    Catalog,
    Thread,
    /// Watched and History.
    Saved,
}

const VIEWS: [Scope; 4] = [Scope::Lists, Scope::Catalog, Scope::Thread, Scope::Saved];

/// Every action: config name, default key, and the scopes it's bound in.
pub const ACTIONS: &[(Action, &str, char, &[Scope])] = &[
    (Action::Quit, "quit", 'q', &[Scope::Global]),
    (Action::Help, "help", '?', &[Scope::Global]),
    (Action::Search, "search", '/', &[Scope::Global]),
    (Action::Reload, "reload", 'r', &[Scope::Global]),
    (Action::Browser, "browser", 'o', &[Scope::Global]),
    (Action::View, "view", 'v', &[Scope::Catalog, Scope::Thread]),
    (Action::Watch, "watch", 'w', &[Scope::Catalog, Scope::Thread]),
    (Action::Sort, "sort", 's', &[Scope::Catalog]),
    (Action::Compact, "compact", 'c', &[Scope::Catalog]),
    (Action::OpenFile, "open_file", 'i', &[Scope::Thread]),
    (Action::Replies, "replies", 'b', &[Scope::Thread]),
    (Action::JumpBack, "jump_back", 'u', &[Scope::Thread]),
    (Action::Unread, "unread", 'U', &[Scope::Thread]),
    (Action::Preview, "preview", 'p', &[Scope::Thread]),
    (Action::NextMatch, "next_match", 'n', &[Scope::Thread]),
    (Action::PrevMatch, "prev_match", 'N', &[Scope::Thread]),
    (Action::Spoiler, "spoiler", 's', &[Scope::Thread]),
    (Action::AllSpoilers, "all_spoilers", 'S', &[Scope::Thread]),
    (Action::Download, "download", 'd', &[Scope::Thread]),
    (Action::DownloadThread, "download_thread", 'D', &[Scope::Thread]),
    (Action::Archive, "archive", 'a', &[Scope::Thread]),
    (Action::Remove, "remove", 'x', &[Scope::Saved]),
];

/// Fixed navigation keys per view, which remapped keys must not collide with.
fn fixed(scope: Scope) -> &'static [char] {
    match scope {
        Scope::Thread => &['j', 'k', 'g', 'G', 'h', 'l', 'J', 'K', ' '],
        _ => &['j', 'k', 'g', 'G', 'h', 'l'],
    }
}

#[derive(Debug, Clone)]
pub struct KeyMap {
    keys: HashMap<Action, char>,
}

impl Default for KeyMap {
    fn default() -> Self {
        Self { keys: ACTIONS.iter().map(|&(a, _, k, _)| (a, k)).collect() }
    }
}

impl KeyMap {
    /// Apply `[keys]` overrides (`action = "key"`), rejecting unknown actions, keys that
    /// aren't a single character, and two commands on one key in the same view.
    pub fn new(overrides: &HashMap<String, String>) -> Result<Self> {
        let mut map = Self::default();
        let mut names: Vec<_> = overrides.keys().collect();
        names.sort();
        for name in names {
            let Some(&(action, ..)) = ACTIONS.iter().find(|(_, n, ..)| n == name) else {
                let known: Vec<_> = ACTIONS.iter().map(|(_, n, ..)| *n).collect();
                bail!("[keys]: unknown action `{name}` (known: {})", known.join(", "));
            };
            let value = &overrides[name];
            let mut chars = value.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                bail!("[keys]: `{name} = \"{value}\"` must be a single character");
            };
            map.keys.insert(action, c);
        }
        for view in VIEWS {
            let mut seen: HashMap<char, &str> = fixed(view).iter().map(|&c| (c, "navigation")).collect();
            for &(action, name, _, scopes) in ACTIONS {
                if !scopes.iter().any(|&s| s == view || s == Scope::Global) {
                    continue;
                }
                let key = map.keys[&action];
                if let Some(other) = seen.insert(key, name) {
                    bail!("[keys]: `{name}` and `{other}` both use '{key}' in the {view:?} view");
                }
            }
        }
        Ok(map)
    }

    pub fn key(&self, action: Action) -> char {
        self.keys[&action]
    }

    /// The action bound to `c` in `scope`, or a global one.
    pub fn action(&self, scope: Scope, c: char) -> Option<Action> {
        ACTIONS
            .iter()
            .find(|&&(a, _, _, scopes)| self.keys[&a] == c && scopes.iter().any(|&s| s == scope || s == Scope::Global))
            .map(|&(a, ..)| a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> Result<KeyMap> {
        KeyMap::new(&pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect())
    }

    #[test]
    fn defaults_are_consistent() {
        let m = map(&[]).unwrap();
        assert_eq!(m.action(Scope::Catalog, 's'), Some(Action::Sort));
        assert_eq!(m.action(Scope::Thread, 's'), Some(Action::Spoiler));
        assert_eq!(m.action(Scope::Saved, 'q'), Some(Action::Quit));
        assert_eq!(m.action(Scope::Lists, 'x'), None);
    }

    #[test]
    fn overrides_and_errors() {
        let m = map(&[("watch", "W")]).unwrap();
        assert_eq!(m.action(Scope::Thread, 'W'), Some(Action::Watch));
        assert_eq!(m.action(Scope::Thread, 'w'), None);

        let err = |pairs| map(pairs).unwrap_err().to_string();
        assert!(err(&[("nope", "z")]).contains("unknown action `nope`"));
        assert!(err(&[("watch", "ctrl-w")]).contains("single character"));
        assert!(err(&[("sort", "c")]).contains("both use 'c' in the Catalog view"));
        assert!(err(&[("preview", "j")]).contains("navigation"));
        // A global key can't shadow a view's key.
        assert!(err(&[("reload", "v")]).contains("'v'"));
    }
}
