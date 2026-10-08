//! `V`: every file of the thread (or the conversation shown) as a grid of thumbnails.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;

use super::{App, TabPopup, View, Viewer};
use crate::download;
use crate::keys::{Action, Scope};
use crate::model::Attachment;

pub struct Gallery {
    /// Each file with its post's number.
    pub files: Vec<(u64, Attachment)>,
    pub state: ListState,
}

impl App {
    pub fn open_gallery(&mut self) {
        let Some(t) = &self.tab.thread else { return };
        // In a conversation, its files; not hidden posts' (unless shown).
        // Starting at the selected post's first file, or the next one after it.
        let (mut files, mut start) = (Vec::new(), None);
        for (i, p) in t.shown_posts() {
            if i >= t.selected && !p.files.is_empty() {
                start = start.or(Some(files.len()));
            }
            files.extend(p.files.iter().map(|f| (p.no, f.clone())));
        }
        if files.is_empty() {
            let msg = self.no_files(if t.conversation.is_some() { "The conversation has no files" } else { "Thread has no files" }, true, |_| true);
            return self.info(msg);
        }
        let state = ListState::default().with_selected(Some(start.unwrap_or(0)));
        self.tab.gallery = Some(Gallery { files, state });
    }

    /// `none`, or that only hidden posts (of those in view, `in_view`) have files that are `wanted`.
    pub(super) fn no_files(&self, none: &str, in_view: bool, wanted: fn(&Attachment) -> bool) -> String {
        let hidden = self.tab.thread.as_ref().is_some_and(|t| t.posts.iter().enumerate().any(|(i, p)| p.files.iter().any(wanted) && (!in_view || t.in_view(i))));
        if hidden { format!("Only hidden posts have files ({} shows them)", self.keys.key(Action::ShowHidden)) } else { none.to_string() }
    }

    pub fn on_gallery_key(&mut self, key: KeyEvent) {
        // Before a changed gallery is drawn, it steps one card, with no edge to leave by.
        let (cols, edges) = self.drawn_cols().map_or((1, false), |c| (c.unwrap_or(1), true));
        let Some(g) = &mut self.tab.gallery else { return };
        let n = g.files.len();
        let cur = g.state.selected().unwrap_or(0);
        let action = self.keys.action(Scope::Thread, &key);
        let to = match key.code {
            KeyCode::Char('j') | KeyCode::Down => Some((cur + cols).min(n - 1)),
            KeyCode::Char('k') | KeyCode::Up => Some(cur.saturating_sub(cols)),
            KeyCode::Char('l') | KeyCode::Right => Some((cur + 1).min(n - 1)),
            KeyCode::Char('h') | KeyCode::Left if !edges || cur % cols != 0 => Some(cur.saturating_sub(1)),
            KeyCode::Char('g') | KeyCode::Home => Some(0),
            KeyCode::Char('G') | KeyCode::End => Some(n - 1),
            KeyCode::PageDown => Some((cur + cols * 3).min(n - 1)),
            KeyCode::PageUp => Some(cur.saturating_sub(cols * 3)),
            _ => None,
        };
        if let Some(to) = to {
            g.state.select(Some(to));
            return;
        }
        match (key.code, action) {
            (KeyCode::Enter, _) => self.view_from_gallery(cur),
            (KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left | KeyCode::Backspace, _) => self.close_gallery(),
            (_, Some(action)) => self.gallery_action(action),
            _ => {}
        }
    }

    /// A command in the gallery, on the selected file.
    pub(super) fn gallery_action(&mut self, action: Action) {
        let Some(g) = &self.tab.gallery else { return };
        let cur = g.state.selected().unwrap_or(0);
        match action {
            Action::View => self.view_from_gallery(cur),
            Action::Download => self.download_file(cur),
            Action::DownloadThread => self.ask_to_save(super::Saving::Files),
            Action::Export => self.ask_to_save(super::Saving::Page),
            Action::Menu => self.open_menu(),
            Action::Copy => {
                if let Some((_, f)) = g.files.get(cur) {
                    match f.link() {
                        Some((what, url)) => self.copy_text(what, url.to_string()),
                        None => self.info(super::NEITHER),
                    }
                }
            }
            Action::CopyLink => {
                if let Some(link) = self.gallery_link(cur) {
                    self.copy_text("link", link);
                }
            }
            Action::Gallery => self.close_gallery(),
            _ => {}
        }
    }

    /// Back to the thread, with the selected file's post selected.
    pub fn close_gallery(&mut self) {
        let Some(g) = self.tab.gallery.take() else { return };
        let post = g.state.selected().and_then(|k| g.files.get(k)).map(|&(no, _)| no);
        if let (Some(no), Some(t)) = (post, &mut self.tab.thread) {
            t.select_post(no);
        }
    }

    pub fn view_from_gallery(&mut self, k: usize) {
        if self.images_off_here() {
            return;
        }
        let Some(g) = &self.tab.gallery else { return };
        if !self.images.enabled() {
            let Some((_, f)) = g.files.get(k).cloned() else { return };
            self.open_file(&f);
            return;
        }
        let files = g.files.iter().map(|(_, f)| f.clone()).collect();
        let posts = g.files.iter().map(|&(no, _)| no).collect();
        self.tab.popup = Some(TabPopup::Viewer(Viewer { posts, ..Viewer::new(files, k, None) }));
    }

    /// `v` in a thread: the viewer over every file in it (or in the conversation shown,
    /// leaving out hidden posts), from the selected post's file `k`. False if that file
    /// isn't among them.
    pub(super) fn thread_viewer(&mut self, k: usize) -> bool {
        let Some(t) = self.tab.thread.as_ref().filter(|_| self.tab.view() == View::Thread) else { return false };
        let (mut files, mut posts, mut start) = (Vec::new(), Vec::new(), None);
        for (i, p) in t.shown_and_selected() {
            for (j, f) in p.files.iter().enumerate() {
                if i == t.selected && j == k {
                    start = Some(files.len());
                }
                files.push(f.clone());
                posts.push(p.no);
            }
        }
        let Some(start) = start else { return false };
        self.tab.popup = Some(TabPopup::Viewer(Viewer { posts, ..Viewer::new(files, start, None) }));
        true
    }

    /// The link to the post the viewer's file is from, when it knows.
    pub(super) fn viewer_post_link(&self) -> Option<String> {
        let (v, t, b) = (self.tab.viewer()?, self.tab.thread.as_ref()?, self.tab.board.as_ref()?);
        let no = *v.posts.get(v.index)?;
        self.thread_link(&self.key(&b.uri, t.no), Some(no))
    }

    /// The link to the post a gallery file is from.
    pub fn gallery_link(&self, k: usize) -> Option<String> {
        let (g, t, b) = (self.tab.gallery.as_ref()?, self.tab.thread.as_ref()?, self.tab.board.as_ref()?);
        self.thread_link(&self.key(&b.uri, t.no), Some(g.files.get(k)?.0))
    }

    fn download_file(&mut self, k: usize) {
        let (Some(g), Some(t)) = (&self.tab.gallery, &self.tab.thread) else { return };
        // A refresh may have taken the post away since the gallery opened.
        let Some(((_, file), p)) = g.files.get(k).and_then(|f| Some((f, t.posts.get(*t.index.get(&f.0)?)?))) else { return };
        let dir = download::dir(self.download_dir.as_deref(), &self.current_site().cfg.name, &t.board, t.no);
        let (jobs, none) = (download::job(p, file, &dir), super::saving::nothing_to_save(file));
        self.start_download(jobs, dir, none);
    }
}
