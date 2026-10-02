//! The home screen (the Sites view): favorite boards at the top, opened with 1-9.

use std::collections::HashMap;

use super::{App, SiteRow, View};
use crate::route::Target;

/// A board on a site, as `site/board` in the config.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BoardRef {
    pub site: String,
    pub board: String,
}

impl BoardRef {
    pub fn parse(s: &str) -> Option<Self> {
        let (site, board) = s.split_once('/')?;
        let board = board.trim_matches('/');
        (!site.is_empty() && !board.is_empty()).then(|| Self { site: site.into(), board: board.into() })
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.site, self.board)
    }
}

impl App {
    /// Board titles for the home screen's rows, from loaded, configured or saved board lists.
    pub fn refresh_home_titles(&mut self) {
        let mut titles = HashMap::new();
        for s in &self.sites {
            let saved;
            let boards = match (&s.boards, &s.cfg.boards) {
                (Some(b), _) => b.clone(),
                (None, Some(cfg)) => cfg.iter().map(crate::backend::to_board).collect(),
                (None, None) => {
                    saved = self.store.load_boards(&s.cfg.name).map(|(b, _)| b).unwrap_or_default();
                    saved
                }
            };
            for b in boards {
                titles.insert(format!("{}/{}", s.cfg.name, b.uri), b.title);
            }
        }
        self.home_titles = titles;
    }

    pub fn board_title(&self, b: &BoardRef) -> &str {
        self.home_titles.get(&b.key()).map_or("", String::as_str)
    }

    /// Open a board's catalog from the home screen.
    pub fn open_board(&mut self, b: &BoardRef) {
        match self.sites.iter().position(|s| s.cfg.name == b.site) {
            Some(site) => self.go(Target { site, board: Some(b.board.clone()), thread: None, post: None }),
            None => self.status = Some((format!("No site named {} in the config", b.site), true)),
        }
    }

    /// The board a `*` applies to: the selected one in Boards, the open one in a catalog.
    fn current_board_ref(&self) -> Option<BoardRef> {
        let site = self.current_site().cfg.name.clone();
        let board = match self.view {
            View::Boards => self.selected_index().map(|i| self.boards()[i].uri.clone())?,
            View::Catalog | View::Thread => self.board.as_ref()?.uri.clone(),
            View::Sites => match self.selected_site_row()? {
                SiteRow::Favorite(i) => return self.favorites.get(i).cloned(),
                _ => return None,
            },
            _ => return None,
        };
        Some(BoardRef { site, board })
    }

    pub fn selected_site_row(&self) -> Option<SiteRow> {
        self.site_list.state.selected().and_then(|i| self.visible_sites().get(i).copied())
    }

    /// `*`: add the board to the favorites, or take it off. They're kept in the config.
    pub fn toggle_favorite(&mut self) {
        let Some(b) = self.current_board_ref() else { return };
        let added = match self.favorites.iter().position(|f| *f == b) {
            Some(i) => {
                self.favorites.remove(i);
                false
            }
            None => {
                self.favorites.push(b.clone());
                true
            }
        };
        self.save_favorites(&if added {
            format!("/{}/ as a favorite ({} on the home screen opens it)", b.board, self.favorites.len().min(9))
        } else {
            format!("/{}/ off the favorites", b.board)
        });
    }

    pub fn save_favorites(&mut self, what: &str) {
        let list: toml_edit::Array = self.favorites.iter().map(|f| f.key()).collect();
        self.save_config(what, |d| d["favorites"] = toml_edit::value(list));
        self.clamp_home();
    }

    /// 1-9 on the home screen: open that favorite.
    pub fn open_favorite(&mut self, n: usize) {
        match self.favorites.get(n) {
            Some(b) => {
                let b = b.clone();
                self.open_board(&b);
            }
            None if self.favorites.is_empty() => {
                let key = self.keys.key(crate::keys::Action::Favorite);
                self.status = Some((format!("No favorites yet: {key} on a board adds one"), false));
            }
            None => self.status = Some((format!("There are {} favorites", self.favorites.len()), false)),
        }
    }

    fn clamp_home(&mut self) {
        let len = self.visible_sites().len();
        self.site_list.clamp(len);
    }
}

#[cfg(test)]
mod tests {
    use super::BoardRef;

    #[test]
    fn board_refs() {
        assert_eq!(BoardRef::parse("4chan/g"), Some(BoardRef { site: "4chan".into(), board: "g".into() }));
        assert_eq!(BoardRef::parse("lainchan/λ/").unwrap().key(), "lainchan/λ");
        assert!(BoardRef::parse("4chan").is_none() && BoardRef::parse("/g/").is_none());
    }
}
