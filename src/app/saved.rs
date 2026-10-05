//! Saved threads (see `crate::saved`): reading a copy offline, and offering it when the
//! thread 404s.

use std::path::PathBuf;

use super::tabs::{Offline, ThreadCopy};
use super::{App, View};
use crate::images::Kind;
use crate::model::{Attachment, Post};
use crate::store::ThreadKey;

impl App {
    /// Open a thread's saved copy. It's read offline: nothing in it is fetched.
    pub fn open_saved(&mut self, key: ThreadKey) {
        let Some(site) = self.sites.iter().position(|s| s.cfg.name == key.site) else {
            self.error(format!("No site named {} in the config", key.site));
            return;
        };
        let copy = match self.store.load_saved(&key) {
            Ok(t) => t,
            Err(e) => {
                self.error(format!("Couldn't open the saved copy: {e:#}"));
                self.clamp_list();
                self.save();
                return;
            }
        };
        let dead = copy.dead || self.store.saved(&key).is_some_and(|m| m.dead);
        let posts: Vec<Post> = copy.posts.into_iter().map(Post::from).collect();
        if posts.is_empty() {
            self.error("The saved copy has no posts");
            return;
        }
        if self.tab.view != View::Thread {
            self.tab.return_to = Some(self.tab.view);
        }
        self.switch_site(site);
        self.tab.board = Some(self.find_board(&key.board));
        // A request in flight in this tab is dropped (its answer will be ignored).
        self.tab.req = None;
        self.tab.loading = None;
        self.tab.from_catalog = false;
        self.tab.gallery = None;
        self.tab.archive_offer = None;
        self.tab.saved_offer = None;
        // The live thread, if that's what's open, keeps its place in the copy.
        let same = self.tab.thread.as_ref().is_some_and(|t| t.no == key.no && t.board == key.board);
        if !same {
            self.tab.thread = None;
        }
        self.tab.view = View::Thread;
        self.show_thread(posts, Some(ThreadCopy::Saved(Offline { saved: copy.saved, dead })));
    }

    /// `r` on a saved copy: the live thread, unless it's known to be gone.
    pub fn refresh_saved(&mut self) {
        let (Some(off), Some(t)) = (self.tab.saved(), &self.tab.thread) else { return };
        if off.dead {
            let x = self.keys.key(crate::keys::Action::Archive);
            let archive = if self.archive_of(&t.board, t.no).is_some() { format!(" ({x} looks in the archive)") } else { String::new() };
            self.info(format!("This is a saved copy from {}; the thread is gone{archive}", crate::ui::ago(off.saved, self.clock)));
            return;
        }
        let no = t.no;
        // The copy stays on screen until the live thread arrives, which keeps its place.
        self.tab.copy = None;
        self.load_thread(no);
    }

    /// The thread a site's configured archive would have, if it has one.
    pub(super) fn archive_of(&self, board: &str, no: u64) -> Option<ThreadKey> {
        let site = self.current_site();
        let archive = site.cfg.archive.clone().filter(|a| self.sites.iter().any(|s| s.cfg.name == *a))?;
        Some(ThreadKey { site: archive, board: board.to_string(), no })
    }

    /// What the viewer shows for a file: the image itself, or the thumbnail for other files.
    /// A saved copy's images come from the download folder, else their cached thumbnail.
    pub fn viewer_source(&self, file: &Attachment) -> Option<(String, Kind)> {
        let thumb = || file.thumb.clone().map(|u| (u, Kind::Thumb));
        if self.tab.saved().is_none() || self.tab.view != View::Thread {
            return if file.is_image() { Some((file.url.clone(), Kind::Full)) } else { thumb() };
        }
        match self.downloaded(file).filter(|_| file.is_image()) {
            Some(path) => Some((format!("file://{}", path.display()), Kind::Full)),
            None => thumb(),
        }
    }

    /// Where `d` saved the open thread's file, if it did.
    pub fn downloaded(&self, file: &Attachment) -> Option<PathBuf> {
        let t = self.tab.thread.as_ref()?;
        let p = t.posts.iter().find(|p| p.files.iter().any(|f| f.url == file.url))?;
        let dir = crate::download::dir(self.download_dir.as_deref(), &self.current_site().cfg.name, &t.board, t.no);
        let path = crate::download::jobs(&[p], &dir).into_iter().find(|(u, _)| *u == file.url)?.1;
        path.is_file().then_some(path)
    }

    /// `enter` where a thread 404'd and has a saved copy: open it.
    pub(super) fn take_saved_offer(&mut self) -> bool {
        match self.tab.saved_offer.take() {
            Some(key) if self.tab.thread.is_none() => {
                self.open_saved(key);
                true
            }
            _ => false,
        }
    }
}
