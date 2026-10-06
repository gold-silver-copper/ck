//! Images on or off per board: a board's own setting (kept with its sort and layout), or
//! for boards the site marks NSFW, `nsfw_images`. With images off on a board, ck asks for
//! none of its images and shows "image off" where its thumbnails would be.

use super::*;
use crate::config::NsfwImages;

impl App {
    /// The key of a board's preferences: `site/board`.
    pub(super) fn prefs_key(&self, site: usize, board: &str) -> String {
        crate::store::board_key(self.sites.get(site).map_or("", |s| s.cfg.name.as_str()), board)
    }

    /// Whether the site marks a board NSFW, as far as its board list says (`None`: it doesn't
    /// say, or the list isn't known).
    fn nsfw(&self, site: usize, board: &str) -> Option<bool> {
        match self.sites.get(site)?.boards.as_ref() {
            Some(boards) => boards.iter().find(|b| b.uri == board)?.nsfw,
            None => self.nsfw_saved.get(&site).map(|n| n.contains(board)),
        }
    }

    /// What images on a board are without its own setting.
    fn images_by_default(&self, site: usize, board: &str) -> bool {
        self.nsfw_images == NsfwImages::Show || self.nsfw(site, board) != Some(true)
    }

    /// Whether a board's images are shown: its own setting, else the default.
    pub fn images_on(&self, site: usize, board: &str) -> bool {
        match self.store.board_prefs.get(&self.prefs_key(site, board)).and_then(|p| p.images) {
            Some(on) => on,
            None => self.images_by_default(site, board),
        }
    }

    /// Images on a catalog's thread: the catalog's own setting if it has one (an overboard
    /// as a whole), else the thread's board's.
    pub fn catalog_images_on(&self, p: &Post) -> bool {
        let (site, catalog) = (self.tab.catalog_site, self.tab.catalog_board.as_deref().unwrap_or_default());
        if let Some(on) = self.store.board_prefs.get(&self.prefs_key(site, catalog)).and_then(|p| p.images) {
            return on;
        }
        self.images_on(site, p.board.as_deref().unwrap_or(catalog))
    }

    /// With `nsfw_images = "off"`, know which of a site's boards are NSFW: from its saved
    /// board list, or (once) by loading it in the background.
    pub(super) fn know_nsfw(&mut self, site: usize) {
        if self.nsfw_images == NsfwImages::Show || self.nsfw_saved.contains_key(&site) {
            return;
        }
        let Some(name) = self.sites.get(site).filter(|s| s.boards.is_none()).map(|s| s.cfg.name.clone()) else { return };
        match self.store.load_boards(&name) {
            Some((boards, _)) => {
                self.nsfw_saved.insert(site, boards.into_iter().filter(|b| b.nsfw == Some(true)).map(|b| b.uri).collect());
            }
            None if self.nsfw_asked.insert(site) => self.refresh_boards_in_background(site),
            None => {}
        }
    }

    /// The board the `.` menu's image switch is for: the selected board in the Boards list,
    /// the catalog's, or the thread's.
    pub(super) fn images_target(&self) -> Option<(usize, String)> {
        match self.tab.view {
            View::Boards => self.selected_index().and_then(|i| self.boards().get(i).map(|b| (self.tab.site, b.uri.clone()))),
            View::Catalog => self.tab.catalog_board.clone().map(|b| (self.tab.catalog_site, b)),
            View::Thread => self.tab.thread.as_ref().map(|t| (self.tab.site, t.board.clone())),
            _ => None,
        }
    }

    /// The `.` menu's row for the image switch, when there's a board to switch.
    pub(super) fn board_images_row(&self) -> Option<String> {
        let (site, board) = self.images_target()?;
        Some(if self.images_on(site, &board) { "images on this board: on → off" } else { "images on this board: off → on" }.into())
    }

    /// The boards with their own image setting, `site/board` and on or off, in order.
    pub fn boards_with_images_set(&self) -> Vec<(String, bool)> {
        self.store.board_prefs.iter().filter_map(|(k, p)| Some((k.clone(), p.images?))).collect()
    }

    /// Settings: take a board's own image setting away (back to the default).
    pub(super) fn reset_board_images(&mut self, key: &str) {
        if let Some(p) = self.store.board_prefs.get_mut(key) {
            p.images = None;
            self.save_now();
            self.info(format!("/{}/ is back to the default for images", key.split_once('/').map_or(key, |(_, b)| b)));
        }
    }

    /// The `.` menu's "images on this board": on, off. Back to the default, the board's own
    /// setting goes.
    pub(super) fn toggle_board_images(&mut self) {
        let Some((site, board)) = self.images_target() else { return };
        let on = !self.images_on(site, board.as_str());
        let key = self.prefs_key(site, &board);
        let own = (on != self.images_by_default(site, &board)).then_some(on);
        self.store.board_prefs.entry(key).or_default().images = own;
        self.save_now();
        self.info(if on { format!("Images on /{board}/") } else { format!("Images off on /{board}/: none are asked for") });
    }

    /// Opening an image (the viewer) where the post's board has images off: say so instead.
    pub(super) fn images_off_here(&mut self) -> bool {
        let (on, board) = match self.tab.view {
            View::Catalog => {
                let Some(p) = self.selected_post() else { return false };
                (self.catalog_images_on(p), p.board.clone().or_else(|| self.tab.catalog_board.clone()).unwrap_or_default())
            }
            View::Thread => {
                let Some(t) = &self.tab.thread else { return false };
                let on = self.images_on(self.tab.site, &t.board);
                if on && !self.thread_images_on() {
                    let key = self.keys.key(Action::Media);
                    self.info(format!("Images are hidden in this thread ({key} shows them); i opens the file"));
                    return true;
                }
                (on, t.board.clone())
            }
            _ => return false,
        };
        if on {
            return false;
        }
        let menu = self.keys.how(Action::Menu);
        self.info(format!("Images are off on /{board}/ ({menu} turns them on); i opens the file"));
        true
    }
}
