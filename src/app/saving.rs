//! Saving to your computer. `d` saves the file in front of you: the focused one, the
//! viewer's, the gallery's. Saving a whole post's files, all the thread's, or the thread as
//! a page are in the `.` menu (they have no key unless given one), and the two big ones ask
//! first, saying what they'll write and where.

use std::fmt::Write as _;

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
        if self.tab.viewer().is_some() {
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
        let Some(v) = self.tab.viewer() else { return };
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
                let board = p.board.clone().or_else(|| self.tab.catalog_board.clone());
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
                // Not hidden posts' (unless shown).
                let posts: Vec<&Post> = t.unhidden_posts().map(|(_, p)| p).collect();
                let jobs = download::jobs(&posts, &dir);
                if jobs.is_empty() {
                    return self.info(self.no_files("Thread has no files", false));
                }
                let have = jobs.iter().filter(|(_, path)| path.exists()).count();
                let files: Vec<&Attachment> = posts.iter().flat_map(|p| &p.files).collect();
                let known = files.iter().filter_map(|f| f.size).fold(0u64, u64::saturating_add);
                let mut first = format!("{} file{}", jobs.len() - have, if jobs.len() - have == 1 { "" } else { "s" });
                if known > 0 {
                    let about = if files.iter().all(|f| f.size.is_some()) { "" } else { "at least " };
                    let _ = write!(first, " ({about}{} in all)", size(known));
                }
                if have > 0 {
                    let _ = write!(first, ", {have} already there");
                }
                if have == jobs.len() {
                    return self.info(format!("All {} of the thread's files are already in {at}", jobs.len()));
                }
                ("Save all the thread's files?", vec![first, format!("to {at}")])
            }
            Saving::Page => (
                "Save the thread as a page?",
                vec![format!("thread.html and thread.json ({} posts)", t.live_posts().len()), format!("to {at}"), "and a copy in Saved".into()],
            ),
        };
        self.popup = Some(Popup::Confirm(Confirm { what, title, lines }));
    }

    /// Keys while a save asks: `enter` (or `y`) saves, anything else cancels.
    pub(super) fn on_confirm_key(&mut self, key: KeyEvent) {
        let Some(c) = take_popup!(self, Confirm) else { return };
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

/// Progress of the files being saved.
#[derive(Default)]
pub struct Downloads {
    pub total: usize,
    pub done: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Download jobs still running.
    pub running: usize,
    pub dir: Option<std::path::PathBuf>,
    pub last_error: Option<String>,
}

pub(super) enum DlEvent {
    Done,
    Skipped,
    Failed(String),
    Finished,
}

impl App {
    /// Save the selected post's files, or the whole thread's.
    pub(super) fn download(&mut self, whole_thread: bool) {
        let Some(t) = &self.tab.thread else { return };
        let posts: Vec<&Post> = if whole_thread { t.unhidden_posts().map(|(_, p)| p).collect() } else { t.current().into_iter().collect() };
        let dir = download::dir(self.download_dir.as_deref(), &self.current_site().cfg.name, &t.board, t.no);
        let jobs = download::jobs(&posts, &dir);
        let none = if whole_thread { self.no_files("Thread has no files", false) } else { "Post has no file".into() };
        self.start_download(jobs, dir, &none);
    }

    /// Save the thread as thread.html and thread.json in its download folder.
    fn export_thread(&mut self) {
        let (Some(t), Some(b)) = (&self.tab.thread, &self.tab.board) else { return };
        let site = self.current_site();
        let dir = download::dir(self.download_dir.as_deref(), &site.cfg.name, &t.board, t.no);
        let url = site.backend.thread_url(&b.uri, t.no);
        let about = crate::export::About { site: &site.cfg.name, board: &t.board, thread: t.no, url: &url, saved: self.clock.now() };
        let key = self.key(&t.board, t.no);
        // As the site has it, like the saved copy it's also kept as.
        let posts = t.live_posts().into_owned();
        match crate::export::save(&posts, &about, &theme::theme(), &dir) {
            Ok(()) => {
                // Also kept as a saved copy, to read in ck (the Saved view).
                self.keep_copy(&key, &posts, true);
                self.save_now();
                self.info(format!("Saved thread.html and thread.json in {} (and in Saved)", tilde(&dir.display().to_string())));
            }
            Err(e) => self.error(e.context("Couldn't save the thread")),
        }
    }

    /// Fetch `(url, path)` jobs into `dir` in the background.
    pub(super) fn start_download(&mut self, jobs: Vec<(String, std::path::PathBuf)>, dir: std::path::PathBuf, none: &str) {
        if jobs.is_empty() {
            self.info(none);
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.error(anyhow::Error::from(e).context(format!("Couldn't create {}", dir.display())));
            return;
        }
        let d = &mut self.downloads;
        if d.running == 0 {
            *d = Downloads::default();
        }
        d.total += jobs.len();
        d.running += 1;
        d.dir = Some(dir);
        let later = self.later();
        std::thread::spawn(move || {
            for (url, path) in jobs {
                let ev = if path.exists() {
                    DlEvent::Skipped
                } else {
                    match crate::guard::result(|| http::download_to(&url, &path)) {
                        Ok(()) => DlEvent::Done,
                        Err(e) => DlEvent::Failed(http::plain(&e)),
                    }
                };
                if !later.run(move |app| app.download_event(ev)) {
                    return;
                }
            }
            later.run(|app| app.download_event(DlEvent::Finished));
        });
    }

    pub(super) fn download_event(&mut self, ev: DlEvent) {
        let d = &mut self.downloads;
        match ev {
            DlEvent::Done => d.done += 1,
            DlEvent::Skipped => d.skipped += 1,
            DlEvent::Failed(e) => {
                d.failed += 1;
                d.last_error = Some(e);
            }
            DlEvent::Finished => {
                d.running -= 1;
                if d.running == 0 {
                    let dir = d.dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                    let mut msg = format!("Downloaded {} file{} to {dir}", d.done, if d.done == 1 { "" } else { "s" });
                    if d.skipped > 0 {
                        let _ = write!(msg, ", {} already there", d.skipped);
                    }
                    if d.failed > 0 {
                        let _ = write!(msg, ", {} failed ({})", d.failed, d.last_error.as_deref().unwrap_or(""));
                    }
                    if d.failed > 0 { self.error(msg) } else { self.info(msg) }
                }
            }
        }
    }
}
