//! `O`: a panel of the selected post's links (quotes leading elsewhere, web links, files).

use std::time::Instant;

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;

use super::{App, Popup, TabPopup, View};
use crate::model::{Attachment, Link};

pub enum LinkItem {
    /// A quote of a post in another thread or board, and how it reads (`>>>/g/123`).
    Quote(Link, String),
    Url(String),
    File(Attachment),
}

pub struct LinksPanel {
    pub items: Vec<LinkItem>,
    pub list: ListState,
    /// Where the rows were drawn, for clicks (set by the UI).
    pub area: Rect,
    last_click: Option<(Instant, usize)>,
}

impl App {
    pub fn open_links(&mut self) {
        let Some(post) = self.selected_post() else { return };
        let here = self.tab.board.as_ref().map(|b| b.uri.clone()).unwrap_or_default();
        let thread = self.tab.thread.as_ref().filter(|_| self.tab.view == View::Thread);
        let mut items: Vec<LinkItem> = Vec::new();
        for l in &post.links {
            let board = l.board.clone().unwrap_or_else(|| here.clone());
            // Quotes within the open thread are what enter and p are for.
            let in_thread = thread.is_some_and(|t| {
                board == t.board && (l.thread == Some(t.no) || (l.thread.is_none() && l.post.is_some_and(|p| t.index.contains_key(&p))))
            });
            if in_thread {
                continue;
            }
            let label = match (l.thread, l.post) {
                (_, None) => format!(">>>/{board}/"),
                (Some(t), Some(p)) if t != p => format!(">>>/{board}/{p}  (thread {t})"),
                (_, Some(p)) => format!(">>>/{board}/{p}"),
            };
            items.push(LinkItem::Quote(l.clone(), label));
        }
        items.extend(post.urls.iter().cloned().map(LinkItem::Url));
        items.extend(post.files.iter().filter(|f| f.link().is_some()).cloned().map(LinkItem::File));
        if items.is_empty() {
            self.info("Post has no links");
            return;
        }
        let list = ListState::default().with_selected(Some(0));
        self.tab.popup = Some(TabPopup::Links(LinksPanel { items, list, area: Rect::default(), last_click: None }));
    }

    pub fn on_links_key(&mut self, code: KeyCode) {
        let Some(TabPopup::Links(p)) = &mut self.tab.popup else { return };
        let n = p.items.len();
        let cur = p.list.selected().unwrap_or(0);
        match code {
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.open_link_item(cur),
            KeyCode::Char('y') => {
                if let Some(text) = self.link_item_url(cur) {
                    self.copy_text("link", text);
                }
            }
            code => match super::list_move(code, cur, n) {
                Some(to) => p.list.select(Some(to)),
                None => self.tab.popup = None,
            },
        }
    }

    /// A click in the panel selects a row; a second quick click opens it. Clicks outside close it.
    pub fn on_links_click(&mut self, col: u16, row: u16, now: Instant) {
        let Some(TabPopup::Links(p)) = &mut self.tab.popup else { return };
        let pos = ratatui::layout::Position::new(col, row);
        let first = p.list.offset();
        let i = first + row.saturating_sub(p.area.y) as usize;
        if !p.area.contains(pos) || i >= p.items.len() {
            self.tab.popup = None;
            return;
        }
        let double = p.last_click.is_some_and(|(t, k)| k == i && now.duration_since(t).as_millis() < 400);
        p.last_click = if double { None } else { Some((now, i)) };
        p.list.select(Some(i));
        if double {
            self.open_link_item(i);
        }
    }

    fn open_link_item(&mut self, i: usize) {
        let Some(TabPopup::Links(p)) = self.tab.popup.take() else { return };
        match p.items.into_iter().nth(i) {
            Some(LinkItem::Quote(link, _)) => self.follow(&link),
            Some(LinkItem::Url(url)) => self.open_url(&url),
            Some(LinkItem::File(f)) => self.open_file(&f),
            None => {}
        }
    }

    fn link_item_url(&self, i: usize) -> Option<String> {
        let Some(TabPopup::Links(p)) = &self.tab.popup else { return None };
        Some(match p.items.get(i)? {
            LinkItem::Url(u) => u.clone(),
            LinkItem::File(f) => f.link()?.1.to_string(),
            LinkItem::Quote(l, _) => {
                let backend = &self.current_site().backend;
                let board = l.board.clone().or_else(|| self.tab.board.as_ref().map(|b| b.uri.clone()))?;
                match (l.thread, l.post) {
                    (Some(t), Some(p)) => backend.post_url(&board, t, p),
                    (Some(t), None) => backend.thread_url(&board, t),
                    (None, Some(p)) => backend.thread_url(&board, p),
                    (None, None) => backend.board_url(&board),
                }
            }
        })
    }
}

/// `R`: reverse image search engines for the post's files (or the viewer's file).
pub struct ImageSearchPanel {
    /// `Err(file name)` heads each file's engines when there are several files;
    /// `Ok((file URL, engine))` rows open a search.
    pub rows: Vec<Result<(String, usize), String>>,
    pub list: ListState,
    pub area: Rect,
}

impl App {
    pub fn open_image_search(&mut self) {
        let files: Vec<Attachment> = match self.tab.viewer() {
            Some(v) => v.files.get(v.index).cloned().into_iter().collect(),
            None => self.selected_post().map(|p| p.files.clone()).unwrap_or_default(),
        };
        // Videos and others: their thumbnail is what can be searched.
        let files: Vec<(String, String)> =
            files.iter().filter_map(|f| Some((f.filename.clone(), f.image().or(f.thumb.as_deref())?.to_string()))).collect();
        if files.is_empty() {
            self.info("Post has no image to search for");
            return;
        }
        let mut rows = Vec::new();
        for (name, url) in &files {
            if files.len() > 1 {
                rows.push(Err(name.clone()));
            }
            rows.extend((0..self.image_search.len()).map(|e| Ok((url.clone(), e))));
        }
        let list = ListState::default().with_selected(rows.iter().position(Result::is_ok));
        self.popup = Some(Popup::ImageSearch(ImageSearchPanel { rows, list, area: Rect::default() }));
    }

    pub fn on_image_search_key(&mut self, code: KeyCode) {
        let Some(Popup::ImageSearch(p)) = &mut self.popup else { return };
        let ok: Vec<usize> = p.rows.iter().enumerate().filter(|(_, r)| r.is_ok()).map(|(i, _)| i).collect();
        let cur = ok.iter().position(|&r| Some(r) == p.list.selected()).unwrap_or(0);
        if let Some(&to) = super::list_move(code, cur, ok.len()).and_then(|to| ok.get(to)) {
            p.list.select(Some(to));
            return;
        }
        let row = p.list.selected().and_then(|r| p.rows.get(r)).and_then(|r| r.as_ref().ok()).cloned();
        let link = row.and_then(|(url, e)| self.image_search.get(e).map(|s| s.link(&url)));
        match code {
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                self.popup = None;
                if let Some(link) = link {
                    self.open_url(&link);
                }
            }
            KeyCode::Char('y') => {
                if let Some(link) = link {
                    self.copy_text("link", link);
                }
            }
            _ => self.popup = None,
        }
    }

    pub fn on_image_search_click(&mut self, col: u16, row: u16) {
        let Some(Popup::ImageSearch(p)) = &mut self.popup else { return };
        let r = p.list.offset() + row.saturating_sub(p.area.y) as usize;
        match p.rows.get(r) {
            Some(Ok(_)) if p.area.contains(ratatui::layout::Position::new(col, row)) => {
                p.list.select(Some(r));
                self.on_image_search_key(KeyCode::Enter);
            }
            Some(Err(_)) if p.area.contains(ratatui::layout::Position::new(col, row)) => {}
            _ => self.popup = None,
        }
    }
}
