//! The home screen (the Sites view): favorite boards at the top, opened with 1-9.

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

/// Recent boards shown on the home screen.
const RECENT_SHOWN: usize = 5;

impl App {
    /// Remember a site's board titles for the home screen, from its loaded, configured or
    /// saved board list.
    pub fn note_titles(&mut self, site: usize) {
        let Some(name) = self.sites.get(site).map(|s| s.cfg.name.clone()) else { return };
        let boards = self.known_boards(site).unwrap_or_default();
        self.home_titles.extend(boards.into_iter().map(|b| (format!("{name}/{}", b.uri), b.title)));
    }

    /// At the start: titles for the sites the favorites and recent boards are on.
    pub fn load_home_titles(&mut self) {
        let named: Vec<String> = self.favorites.iter().map(|f| f.site.clone()).chain(self.store.recent_boards.iter().filter_map(|r| BoardRef::parse(r).map(|b| b.site))).collect();
        for i in 0..self.sites.len() {
            if named.contains(&self.sites[i].cfg.name) {
                self.note_titles(i);
            }
        }
    }

    pub fn board_title(&self, b: &BoardRef) -> &str {
        self.home_titles.get(&b.key()).map_or("", String::as_str)
    }

    /// Open a board's catalog from the home screen.
    pub fn open_board(&mut self, b: &BoardRef) {
        match self.sites.iter().position(|s| s.cfg.name == b.site) {
            Some(site) => self.go(Target { site, board: Some(b.board.clone()), thread: None, post: None }),
            None => self.error(format!("No site named {} in the config", b.site)),
        }
    }

    /// The board a `*` applies to: the selected one in Boards, the open one in a catalog.
    fn current_board_ref(&self) -> Option<BoardRef> {
        let site = self.current_site().cfg.name.clone();
        let board = match self.tab.view {
            View::Boards => self.selected_index().map(|i| self.boards()[i].uri.clone())?,
            View::Catalog | View::Thread => self.tab.board.as_ref()?.uri.clone(),
            View::Sites => match self.selected_site_row()? {
                SiteRow::Favorite(i) => return self.favorites.get(i).cloned(),
                SiteRow::Recent(i) => return self.recent_board(i),
                _ => return None,
            },
            _ => return None,
        };
        Some(BoardRef { site, board })
    }

    pub fn recent_board(&self, i: usize) -> Option<BoardRef> {
        BoardRef::parse(self.store.recent_boards.get(i)?)
    }

    /// Recently opened boards for the home screen (indices into `store.recent_boards`): the
    /// last few that aren't favorites.
    pub fn recent_rows(&self) -> Vec<usize> {
        let fav: Vec<String> = self.favorites.iter().map(BoardRef::key).collect();
        let recent = self.store.recent_boards.iter().enumerate();
        recent.filter(|(_, b)| !fav.contains(b)).map(|(i, _)| i).take(RECENT_SHOWN).collect()
    }

    pub fn selected_site_row(&self) -> Option<SiteRow> {
        self.site_list.state.selected().and_then(|i| self.visible_sites().get(i).copied())
    }

    /// `*`: add the board to the favorites, or take it off. They're kept in the config.
    pub fn toggle_favorite(&mut self) {
        let Some(b) = self.current_board_ref() else { return };
        self.note_titles(self.tab.site);
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
                self.info(format!("No favorites yet: {key} on a board adds one"));
            }
            None => self.info(format!("There are {} favorites", self.favorites.len())),
        }
    }

    pub fn is_site_hidden(&self, i: usize) -> bool {
        self.hidden_sites.contains(&self.sites[i].cfg.name)
    }

    /// `x` on a site: leave it off the home screen (or, when hidden ones are shown, bring
    /// it back). Kept in the config as `hidden_sites`.
    pub fn toggle_site_hidden(&mut self, i: usize) {
        let name = self.sites[i].cfg.name.clone();
        let hidden = !self.hidden_sites.remove(&name);
        if hidden {
            self.hidden_sites.insert(name.clone());
        }
        let list: toml_edit::Array = self.hidden_sites.iter().map(String::as_str).collect();
        let what = if hidden { format!("{name} as hidden (the last row shows hidden sites)") } else { format!("{name} as shown") };
        self.save_config(&what, |d| d["hidden_sites"] = toml_edit::value(list));
        self.clamp_home();
    }

    pub(super) fn clamp_home(&mut self) {
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
