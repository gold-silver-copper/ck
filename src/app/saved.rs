//! Saved threads (see `crate::saved`): reading a copy offline, and offering it when the
//! thread 404s.

use std::path::PathBuf;

use super::tabs::{Offline, ThreadCopy};
use super::{App, Opening, View};
use crate::images::Kind;
use crate::model::{Attachment, Thread};
use crate::store::ThreadKey;

impl App {
    /// Open a thread's saved copy. It's read offline: nothing in it is fetched.
    pub fn open_saved(&mut self, key: &ThreadKey, open: Opening) {
        if self.site_index(&key.site).is_none() {
            self.error(format!("No site named {} in the config", key.site));
            return;
        }
        let copy = match self.store.load_saved(key) {
            Ok(t) => t,
            Err(e) => {
                self.error(e.context("Couldn't open the saved copy"));
                self.save();
                return;
            }
        };
        let (dead, saved) = (copy.dead || self.store.saved(key).is_some_and(|m| m.dead), copy.saved);
        let Some(t) = Thread::saved(copy) else {
            self.error("The saved copy has no posts");
            return;
        };
        if self.tab.place_view() != View::Thread {
            self.tab.return_to = Some(self.tab.place_view());
        }
        self.tab.archive_offer = None;
        self.tab.saved_offer = None;
        // The live thread, if that's what's open, keeps its place in the copy.
        let same = self.tab.thread.as_ref().is_some_and(|t| t.key() == key);
        if !same {
            self.tab.thread = None;
        }
        self.tab.navigate(View::Thread);
        self.tab.from_catalog = false;
        self.show_thread(key.clone(), t, Some(ThreadCopy::Saved(Offline { saved, dead })), open);
    }

    /// `r` on a saved copy: the live thread, unless it's known to be gone.
    pub fn refresh_saved(&mut self) {
        let (Some(off), Some(t)) = (self.tab.saved(), &self.tab.thread) else { return };
        if off.dead {
            let x = self.keys.how(crate::keys::Action::Archive);
            let archive = if self.archive_of(t.key()).is_some() { format!(" ({x} looks in the archive)") } else { String::new() };
            self.info(format!("This is a saved copy from {}; the thread is gone{archive}", crate::ui::ago(off.saved, self.clock)));
            return;
        }
        let key = t.key().clone();
        // The copy stays on screen until the live thread arrives, which keeps its place.
        self.tab.copy = None;
        self.load_thread(key, Opening::default());
    }

    /// Thread `key` on its site's configured archive, if it has one.
    pub(super) fn archive_of(&self, key: &ThreadKey) -> Option<ThreadKey> {
        let archive = self.site_named(&key.site)?.cfg.archive.clone()?;
        let i = self.site_index(&archive)?;
        // Not when the archive's boards are known and this isn't one of them.
        if self.known_boards(i).is_some_and(|boards| !boards.iter().any(|b| b.uri == key.board)) {
            return None;
        }
        Some(ThreadKey { site: archive, ..key.clone() })
    }

    /// What the viewer shows for a file: the image itself, or the thumbnail for other files.
    /// A saved copy's images come from the download folder, else their cached thumbnail.
    pub fn viewer_source(&self, file: &Attachment) -> Option<(String, Kind)> {
        let full = if self.tab.saved().is_some() && self.tab.view() == View::Thread {
            self.downloaded(file).filter(|_| file.is_image()).map(|path| format!("file://{}", path.display()))
        } else {
            file.image().map(Into::into)
        };
        full.map(|u| (u, Kind::Full)).or_else(|| file.thumb.clone().map(|u| (u, Kind::Thumb)))
    }

    /// Where `d` saved the open thread's file, if it did.
    pub fn downloaded(&self, file: &Attachment) -> Option<PathBuf> {
        let t = self.tab.thread.as_ref()?;
        let p = t.posts.iter().find(|p| p.files.iter().any(|f| f.url.is_some() && f.url == file.url))?;
        let dir = crate::download::dir(self.download_dir.as_deref(), t.key());
        let path = crate::download::job(p, file, &dir).into_iter().next()?.1;
        path.is_file().then_some(path)
    }

    /// `enter` where a thread 404'd and has a saved copy: open it.
    pub(super) fn take_saved_offer(&mut self) -> bool {
        match self.tab.saved_offer.take() {
            Some(key) if self.tab.thread.is_none() => {
                self.open_saved(&key, Opening::default());
                true
            }
            _ => false,
        }
    }
}
