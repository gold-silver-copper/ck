//! `O`: a panel of the selected post's links (quotes leading elsewhere, web links, files).

use std::time::Instant;

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;

use super::{App, View};
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
        let here = self.board.as_ref().map(|b| b.uri.clone()).unwrap_or_default();
        let thread = self.thread.as_ref().filter(|_| self.view == View::Thread);
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
        items.extend(post.files.iter().cloned().map(LinkItem::File));
        if items.is_empty() {
            self.status = Some(("Post has no links".into(), false));
            return;
        }
        let mut list = ListState::default();
        list.select(Some(0));
        self.links = Some(LinksPanel { items, list, area: Rect::default(), last_click: None });
    }

    pub fn on_links_key(&mut self, code: KeyCode) {
        let Some(p) = &mut self.links else { return };
        let n = p.items.len();
        let cur = p.list.selected().unwrap_or(0);
        match code {
            KeyCode::Char('j') | KeyCode::Down => p.list.select(Some((cur + 1).min(n - 1))),
            KeyCode::Char('k') | KeyCode::Up => p.list.select(Some(cur.saturating_sub(1))),
            KeyCode::Char('g') | KeyCode::Home => p.list.select(Some(0)),
            KeyCode::Char('G') | KeyCode::End => p.list.select(Some(n - 1)),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.open_link_item(cur),
            KeyCode::Char('y') => {
                if let Some(text) = self.link_item_url(cur) {
                    self.copy_text("link", text);
                }
            }
            _ => self.links = None,
        }
    }

    /// A click in the panel selects a row; a second quick click opens it. Clicks outside close it.
    pub fn on_links_click(&mut self, col: u16, row: u16, now: Instant) {
        let Some(p) = &mut self.links else { return };
        let pos = ratatui::layout::Position::new(col, row);
        let first = p.list.offset();
        let i = first + row.saturating_sub(p.area.y) as usize;
        if !p.area.contains(pos) || i >= p.items.len() {
            self.links = None;
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
        let Some(p) = self.links.take() else { return };
        match p.items.into_iter().nth(i) {
            Some(LinkItem::Quote(link, _)) => self.follow(link),
            Some(LinkItem::Url(url)) => self.open_url(&url),
            Some(LinkItem::File(f)) => self.open_file(&f),
            None => {}
        }
    }

    fn link_item_url(&self, i: usize) -> Option<String> {
        let p = self.links.as_ref()?;
        Some(match p.items.get(i)? {
            LinkItem::Url(u) => u.clone(),
            LinkItem::File(f) => f.url.clone(),
            LinkItem::Quote(l, _) => {
                let backend = &self.current_site().backend;
                let board = l.board.clone().or_else(|| self.board.as_ref().map(|b| b.uri.clone()))?;
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
