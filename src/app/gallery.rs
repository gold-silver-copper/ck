//! `V`: every file of the thread (or the conversation shown) as a grid of thumbnails.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;

use super::{App, Viewer};
use crate::download;
use crate::keys::{Action, Scope};
use crate::model::Attachment;

pub struct Gallery {
    /// Each file with the index of its post.
    pub files: Vec<(usize, Attachment)>,
    pub state: ListState,
    /// Columns as last drawn.
    pub cols: usize,
}

impl App {
    pub fn open_gallery(&mut self) {
        let Some(t) = &self.tab.thread else { return };
        // In a conversation, its files.
        let files: Vec<(usize, Attachment)> =
            t.posts.iter().enumerate().filter(|&(i, _)| t.in_view(i)).flat_map(|(i, p)| p.files.iter().map(move |f| (i, f.clone()))).collect();
        if files.is_empty() {
            self.info(if t.conversation.is_some() { "The conversation has no files" } else { "Thread has no files" });
            return;
        }
        // Start at the selected post's first file, or the next one after it.
        let state = ListState::default().with_selected(Some(files.iter().position(|(i, _)| *i >= t.selected).unwrap_or(0)));
        self.tab.gallery = Some(Gallery { files, state, cols: 1 });
    }

    pub fn on_gallery_key(&mut self, key: KeyEvent) {
        let Some(g) = &mut self.tab.gallery else { return };
        let (n, cols) = (g.files.len(), g.cols.max(1));
        let cur = g.state.selected().unwrap_or(0);
        let action = self.keys.action(Scope::Thread, &key);
        let to = match key.code {
            KeyCode::Char('j') | KeyCode::Down => Some((cur + cols).min(n - 1)),
            KeyCode::Char('k') | KeyCode::Up => Some(cur.saturating_sub(cols)),
            KeyCode::Char('l') | KeyCode::Right => Some((cur + 1).min(n - 1)),
            KeyCode::Char('h') | KeyCode::Left if cur % cols != 0 => Some(cur - 1),
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
            (KeyCode::Enter, _) | (_, Some(Action::View)) => self.view_from_gallery(cur),
            (_, Some(Action::Download | Action::DownloadThread)) => self.download_file(cur),
            (_, Some(Action::Menu)) => self.open_menu(),
            (_, Some(Action::Copy)) => {
                if let Some(url) = g.files.get(cur).map(|(_, f)| f.url.clone()) {
                    self.copy_text("file URL", url);
                }
            }
            (_, Some(Action::CopyLink)) => {
                if let Some(link) = self.gallery_link(cur) {
                    self.copy_text("link", link);
                }
            }
            (KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left | KeyCode::Backspace, _) | (_, Some(Action::Gallery)) => {
                self.close_gallery()
            }
            _ => {}
        }
    }

    /// Back to the thread, with the selected file's post selected.
    pub fn close_gallery(&mut self) {
        let Some(g) = self.tab.gallery.take() else { return };
        let post = g.state.selected().and_then(|k| g.files.get(k)).map(|(i, _)| *i);
        if let (Some(i), Some(t)) = (post, &mut self.tab.thread) {
            t.select(i);
        }
    }

    pub fn view_from_gallery(&mut self, k: usize) {
        let Some(g) = &self.tab.gallery else { return };
        if !self.images.enabled() {
            let Some((_, f)) = g.files.get(k).cloned() else { return };
            self.open_file(&f);
            return;
        }
        let files = g.files.iter().map(|(_, f)| f.clone()).collect();
        self.tab.viewer = Some(Viewer { files, index: k, link: None });
    }

    /// The link to the post a gallery file is from.
    pub fn gallery_link(&self, k: usize) -> Option<String> {
        let (g, t, b) = (self.tab.gallery.as_ref()?, self.tab.thread.as_ref()?, self.tab.board.as_ref()?);
        let post = t.posts.get(g.files.get(k)?.0)?.no;
        self.thread_link(&self.key(&b.uri, t.no), Some(post))
    }

    fn download_file(&mut self, k: usize) {
        let (Some(g), Some(t)) = (&self.tab.gallery, &self.tab.thread) else { return };
        // A refresh may have taken the post away since the gallery opened.
        let Some(((_, file), p)) = g.files.get(k).and_then(|f| Some((f, t.posts.get(f.0)?))) else { return };
        let dir = download::dir(self.download_dir.as_deref(), &self.current_site().cfg.name, &t.board, t.no);
        let jobs: Vec<_> = download::jobs(&[p], &dir).into_iter().filter(|(url, _)| *url == file.url).collect();
        self.start_download(jobs, dir, "No file to save");
    }
}
