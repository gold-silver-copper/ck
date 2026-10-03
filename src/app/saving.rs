//! Saving to your computer. `d` saves the file in front of you: the focused one, the
//! viewer's, the gallery's. Saving a whole post's files, all the thread's, or the thread as
//! a page are in the `.` menu (they have no key unless given one), and the two big ones ask
//! first, saying what they'll write and where.

use super::*;

/// What a confirmation would save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saving {
    /// Every file of the thread.
    Files,
    /// thread.html and thread.json.
    Page,
}

/// A save waiting for `enter`.
pub struct Confirm {
    pub what: Saving,
    pub title: &'static str,
    /// What will be written, and where.
    pub lines: Vec<String>,
}

/// `1.2 MB`, `340 KB`.
fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 10 << 20 => format!("{} MB", b >> 20),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{} KB", (b >> 10).max(1)),
    }
}

impl App {
    /// `d`: the viewer's file, the gallery's (see `on_gallery_key`) or the focused one. On a
    /// post with nothing focused it says how, rather than saving all the post's files.
    pub(super) fn save_here(&mut self) {
        if self.tab.viewer.is_some() {
            return self.save_viewed();
        }
        if self.download_focused() {
            return;
        }
        let Some(p) = self.selected_post() else { return };
        let msg = match p.files.len() {
            0 => "Post has no file".to_string(),
            n => format!(
                "{} to a file, then {} saves it ({} saves {})",
                self.keys.how(Action::NextPart),
                self.keys.how(Action::Download),
                self.keys.how(Action::DownloadPost),
                if n == 1 { "the post's file" } else { "all the post's files" }
            ),
        };
        self.info(msg);
    }

    /// Save one file of a post into its thread's folder.
    fn save_file(&mut self, board: &str, thread: u64, post: &Post, url: &str) {
        let dir = download::dir(self.download_dir.as_deref(), &self.current_site().cfg.name, board, thread);
        let jobs = download::jobs(&[post], &dir).into_iter().filter(|(u, _)| u == url).collect();
        self.start_download(jobs, dir, "No file to save");
    }

    /// `d` in the image viewer: the file shown.
    fn save_viewed(&mut self) {
        let Some(v) = &self.tab.viewer else { return };
        let Some(url) = v.files.get(v.index).map(|f| f.url.clone()) else { return };
        let no = v.posts.get(v.index).copied();
        let from = if self.tab.view == View::Thread {
            self.tab.thread.as_ref().and_then(|t| {
                let p = match no {
                    Some(n) => t.posts.get(*t.index.get(&n)?)?,
                    None => t.current()?,
                };
                Some((t.board.clone(), t.no, p.clone()))
            })
        } else {
            // Over a catalog: the thread's OP.
            self.selected_post().and_then(|p| {
                let board = p.board.clone().or_else(|| Some(self.tab.catalog_board.clone()).filter(|b| !b.is_empty()));
                let board = board.or_else(|| self.tab.board.as_ref().map(|b| b.uri.clone()))?;
                Some((board, p.no, p.clone()))
            })
        };
        match from.filter(|(.., p)| p.files.iter().any(|f| f.url == url)) {
            Some((board, thread, p)) => self.save_file(&board, thread, &p, &url),
            None => self.info("Can't tell which thread this file is from; o opens it in the browser"),
        }
    }

    /// Saving all the thread's files, or the thread as a page: say what and where, and wait
    /// for `enter`.
    pub(super) fn ask_to_save(&mut self, what: Saving) {
        let Some(t) = &self.tab.thread else { return };
        let dir = download::dir(self.download_dir.as_deref(), &self.current_site().cfg.name, &t.board, t.no);
        let at = tilde(&dir.display().to_string());
        let (title, lines) = match what {
            Saving::Files => {
                let posts: Vec<&Post> = t.posts.iter().collect();
                let jobs = download::jobs(&posts, &dir);
                if jobs.is_empty() {
                    return self.info("Thread has no files");
                }
                let have = jobs.iter().filter(|(_, path)| path.exists()).count();
                let files: Vec<&Attachment> = t.posts.iter().flat_map(|p| &p.files).collect();
                let known: u64 = files.iter().filter_map(|f| f.size).sum();
                let mut first = format!("{} file{}", jobs.len() - have, if jobs.len() - have == 1 { "" } else { "s" });
                if known > 0 {
                    let about = if files.iter().all(|f| f.size.is_some()) { "" } else { "at least " };
                    first.push_str(&format!(" ({about}{} in all)", size(known)));
                }
                if have > 0 {
                    first.push_str(&format!(", {have} already there"));
                }
                if have == jobs.len() {
                    return self.info(format!("All {} of the thread's files are already in {at}", jobs.len()));
                }
                ("Save all the thread's files?", vec![first, format!("to {at}")])
            }
            Saving::Page => (
                "Save the thread as a page?",
                vec![format!("thread.html and thread.json ({} posts)", t.posts.len()), format!("to {at}"), "and a copy in Saved".into()],
            ),
        };
        self.confirm = Some(Confirm { what, title, lines });
    }

    /// Keys while a save asks: `enter` (or `y`) saves, anything else cancels.
    pub(super) fn on_confirm_key(&mut self, key: KeyEvent) {
        let Some(c) = self.confirm.take() else { return };
        if !matches!(key.code, KeyCode::Enter | KeyCode::Char('y')) {
            return self.info("Not saved");
        }
        match c.what {
            Saving::Files => self.download(true),
            Saving::Page => self.export_thread(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::size;

    #[test]
    fn sizes() {
        assert_eq!(size(100), "1 KB");
        assert_eq!(size(340 << 10), "340 KB");
        assert_eq!(size(3 << 19), "1.5 MB");
        assert_eq!(size(480 << 20), "480 MB");
        assert_eq!(size(5 << 29), "2.5 GB");
    }
}
