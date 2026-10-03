use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::ListState;

use crate::backend::{self, Backend};
pub use crate::config::Sort;
use crate::config::{CatalogLayout, ColorMode, Config, ImagesMode, SiteConfig};
use crate::disk_cache::DiskCache;
use crate::download;
use crate::filter::{Filters, Mark};
use crate::http;
use crate::images::Images;
use crate::keys::{Action, KeyMap, Scope};
use crate::model::{Attachment, Board, Link, Post};
use crate::store::{Store, ThreadKey};
use crate::theme::{self, ThemeDef, ThemeSetting};

mod filters;
mod focus;
mod gallery;
mod input;
mod generals;
mod goto;
mod home;
mod links;
mod saved;
mod saving;
mod board_images;
mod search;
mod session;
mod tabs;
mod settings;
mod sites;
pub use filters::{AddFilter, Candidate, EDIT_ROWS, EditRow, Reach, problem as filters_problem, with_text as filters_with_text};
pub use focus::{HintTarget, HintTo, Hints, Menu, MenuItem};
pub use gallery::Gallery;
pub use home::BoardRef;
pub use links::{ImageSearchPanel, LinkItem, LinksPanel};
pub use saving::{Confirm, Saving};
pub use search::{SavedSearch, Search};
pub use sites::{Adding, BoardsUpdate, MySites, origin as site_origin};
pub use tabs::{MAX_TABS, Offline, Tab};
pub use settings::{Popup as SettingsPopup, SECTIONS as SETTING_SECTIONS, key_rows, rows as setting_rows, tilde};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Sites,
    Boards,
    Catalog,
    Thread,
    Watched,
    History,
    /// Saved copies of threads (watched and exported ones), readable offline.
    Saved,
    Settings,
    /// Archive search results.
    Search,
}

/// Saved board lists older than this (seconds) are refreshed in the background.
const BOARDS_MAX_AGE: i64 = 24 * 3600;

/// Background changes to the data directory are written at most this often.
const SAVE_EVERY: Duration = Duration::from_secs(2);

/// Watched-thread refreshes running at once.
const MAX_REFRESHING: usize = 2;

/// A row of the home screen (the Sites view): Watched and History, favorite boards, then
/// the sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteRow {
    Watched,
    History,
    Saved,
    Favorite(usize),
    /// An index into `store.recent_boards`.
    Recent(usize),
    Site(usize),
    /// "N hidden sites": enter shows them (or hides them again).
    HiddenSites,
}

pub struct Site {
    pub cfg: SiteConfig,
    pub backend: Arc<dyn Backend>,
    pub boards: Option<Vec<Board>>,
}

/// A filterable list with a selection.
#[derive(Default)]
pub struct Picker {
    pub state: ListState,
    pub filter: String,
}

impl Picker {
    /// An empty filter, with the first row selected.
    fn top() -> Self {
        Self { state: ListState::default().with_selected(Some(0)), filter: String::new() }
    }

    fn move_by(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.state.select(None);
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        self.state.select(Some((cur + delta).clamp(0, len as isize - 1) as usize));
    }

    fn clamp(&mut self, len: usize) {
        self.move_by(0, len);
    }
}

/// One post as shown in a thread: at the top level, or inline under a post it replies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub post: usize,
    /// 0 at the top level; each `e` adds a level.
    pub depth: u8,
    /// Post numbers from the top-level post down to this one.
    pub path: Vec<u64>,
}

/// How deep replies can be expanded inline.
const MAX_DEPTH: u8 = 4;

/// The most posts a conversation shows.
pub const CONVERSATION_MAX: usize = 500;

/// One post's conversation (`c`): the post, what it quotes in the thread (and what those
/// quote, on up), and what quotes it (and what quotes those, on down).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    /// The post it's about.
    pub anchor: u64,
    /// Its posts (indices, so in thread order), each with its distance from the anchor:
    /// negative for what the anchor replies to, positive for replies.
    pub depth: std::collections::BTreeMap<usize, i32>,
    /// It had more than `CONVERSATION_MAX` posts; the nearest ones are shown.
    pub capped: bool,
    /// Where the whole thread was scrolled to, for coming back.
    pub back_scroll: usize,
}

/// The conversation of post `p`: up through what it quotes and down through what quotes
/// it, never sideways (other replies to what it quotes aren't in it). Everyone quotes the
/// OP, so it's never gone through, unless it's `p`. Quote loops are followed once.
pub fn conversation_of(posts: &[Post], index: &HashMap<u64, usize>, backlinks: &[Vec<u64>], p: usize) -> (std::collections::BTreeMap<usize, i32>, bool) {
    let mut depth = std::collections::BTreeMap::from([(p, 0)]);
    let mut capped = false;
    let through = |i: usize| i == p || i != 0;
    for up in [true, false] {
        let mut queue = std::collections::VecDeque::from([(p, 0)]);
        while let Some((i, d)) = queue.pop_front() {
            if !through(i) {
                continue;
            }
            let next: Vec<u64> = if up { posts[i].quotes.clone() } else { backlinks[i].clone() };
            let d = if up { d - 1 } else { d + 1 };
            for no in next {
                let Some(&j) = index.get(&no) else { continue };
                if depth.contains_key(&j) {
                    continue;
                }
                if depth.len() >= CONVERSATION_MAX {
                    capped = true;
                    break;
                }
                depth.insert(j, d);
                queue.push_back((j, d));
            }
        }
    }
    (depth, capped)
}

#[derive(Default)]
pub struct ThreadView {
    pub board: String,
    pub no: u64,
    pub posts: Vec<Post>,
    pub index: HashMap<u64, usize>,
    /// For each post, the posts that quote it.
    pub backlinks: Vec<Vec<u64>>,
    pub selected: usize,
    pub scroll: usize,
    jumps: Vec<usize>,
    /// Rendered layout, rebuilt by the UI when the width changes, from cached post lines.
    pub layout: Option<ThreadLayout>,
    pub cache: LineCache,
    pub viewport: usize,
    /// Posts numbered above this arrived since the previous visit (0: first visit, none are new).
    pub new_after: u64,
    /// After new posts arrive, keep this post (index, line offset into it) at the top of the view.
    pub anchor: Option<(usize, usize)>,
    /// Search query (as typed) and the posts matching it.
    pub search: String,
    pub matches: Vec<usize>,
    /// Posts whose spoilers are shown, or all of them.
    pub revealed: HashSet<usize>,
    pub reveal_all: bool,
    /// The posts as shown, with replies expanded inline (`e`) where asked.
    pub entries: Vec<Entry>,
    /// Entries (by path) whose replies are expanded.
    pub expanded: HashSet<Vec<u64>>,
    /// The selected entry; `selected` is its post.
    cursor: usize,
    /// What filters and hiding say about each post, and whether hidden ones are shown.
    pub marks: Vec<Mark>,
    pub show_hidden: bool,
    /// Posts marked as yours.
    pub mine: HashSet<u64>,
    /// The part of the selected post that has focus (`tab`); `None`: the post itself.
    pub focus: Option<Part>,
    /// The selection moved: scroll it into view at the next draw (once it's laid out), so.
    pub reveal: Option<Reveal>,
    /// How far from the top and bottom of the screen the selected post is kept while
    /// reading on: a fraction of the screen (`scroll_margin`).
    pub margin: f32,
    /// The width the line cache is for.
    pub cache_width: u16,
    /// Estimated heights of entries not laid out yet, by the same key as the line cache.
    pub estimates: HashMap<(u64, u16, bool), usize>,
    /// Each post's searchable text, once searched (spoilers hidden; revealed ones aren't kept).
    search_texts: Vec<Option<String>>,
    /// Showing one post's conversation instead of the whole thread.
    pub conversation: Option<Conversation>,
    /// Scroll the focused part into view at the next draw.
    pub follow_focus: bool,
}

/// How the selection comes into view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    /// Just into view, scrolling as little as possible (after laying out again).
    Visible,
    /// `j` / `k`: kept away from the screen's edges by the margin.
    Step,
    /// A jump (a quote, `n`, `U`, ...): its top at the margin from the top.
    Jump,
    /// `k` into a post taller than the screen: its last screenful.
    End,
}

/// A part of a post that can take focus, be clicked or get a hint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Part {
    File(usize),
    Link(crate::model::Target),
    /// The "Replies" label: shows them inline.
    Replies,
}

/// A post's parts in reading order: its files, the links in its text, then its replies.
pub fn parts(p: &Post, backlinks: &[u64]) -> Vec<Part> {
    let mut out: Vec<Part> = (0..p.files.len()).map(Part::File).collect();
    let mut add = |part: Part| {
        if !out.contains(&part) {
            out.push(part);
        }
    };
    for a in &p.anchors {
        add(Part::Link(a.to.clone()));
    }
    if !backlinks.is_empty() {
        add(Part::Replies);
        for &no in backlinks {
            add(Part::Link(crate::model::Target::Quote(Link { board: None, thread: None, post: Some(no) })));
        }
    }
    out
}

/// Where a part was drawn in an entry's block: the line, first column and width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spot {
    pub part: Part,
    pub line: usize,
    pub col: u16,
    pub width: u16,
}

/// A thread laid out: entries near the view exactly, the rest only by an estimate of their
/// height (blank lines), until they come near.
pub struct ThreadLayout {
    pub width: u16,
    /// Each entry's lines: padding, the post, padding, and the gap below it.
    pub blocks: Vec<Rc<[Line<'static>]>>,
    /// `starts[i]` is the first line of entry `i`; has one extra item for the end.
    pub starts: Vec<usize>,
    /// `(line, entry)` for each entry drawn with a thumbnail in the left column.
    pub thumbs: Vec<(usize, usize)>,
    /// Where each entry's parts are, in its block.
    pub spots: Vec<Rc<[Spot]>>,
    /// Which entries are laid out (the rest are estimates).
    pub exact: Vec<bool>,
    /// Which entries have a thumbnail.
    pub has_thumb: Vec<bool>,
    /// Whether thumbnails are shown at all, as laid out.
    pub thumbs_on: bool,
    /// The time (Unix seconds) it was laid out at: "3h ago" moves on.
    pub at: i64,
}

impl ThreadLayout {
    /// A layout of these blocks, all exact (tests).
    #[cfg(test)]
    pub fn of_blocks(width: u16, blocks: Vec<Rc<[Line<'static>]>>) -> Self {
        let n = blocks.len();
        let mut l = ThreadLayout {
            width,
            blocks,
            starts: Vec::new(),
            thumbs: Vec::new(),
            spots: vec![Rc::from([]); n],
            exact: vec![true; n],
            has_thumb: vec![false; n],
            thumbs_on: false,
            at: 0,
        };
        l.restart();
        l
    }

    /// Work out where each entry starts (and its thumbnail), after heights changed.
    pub fn restart(&mut self) {
        self.starts.clear();
        let mut len = 0;
        for b in &self.blocks {
            self.starts.push(len);
            len += b.len();
        }
        self.starts.push(len);
        self.thumbs = (0..self.blocks.len()).filter(|&e| self.has_thumb[e]).map(|e| (self.starts[e] + 1, e)).collect();
    }

    /// The entry line `i` is in.
    pub fn entry_at(&self, i: usize) -> usize {
        self.starts.partition_point(|&s| s <= i).saturating_sub(1).min(self.blocks.len().saturating_sub(1))
    }
    pub fn len(&self) -> usize {
        self.starts.last().copied().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Line `i`, and the entry it's in.
    pub fn line(&self, i: usize) -> Option<(usize, &Line<'static>)> {
        let e = self.starts.partition_point(|&s| s <= i).checked_sub(1)?;
        Some((e, self.blocks.get(e)?.get(i - self.starts.get(e)?)?))
    }
}

/// A post's lines as last laid out, by (post, text width, with a thumbnail), with a hash of
/// what else they show (time, marks, highlights, ...): reused while that's the same.
pub type LineCache = HashMap<(u64, u16, bool), (u64, Rc<[Line<'static>]>, Rc<[Spot]>)>;

impl ThreadView {
    pub fn new(board: String, no: u64, mut posts: Vec<Post>) -> Self {
        // A post number once: a site that repeats one (or a broken answer) keeps the first.
        let mut seen = HashSet::new();
        posts.retain(|p| seen.insert(p.no));
        let index: HashMap<u64, usize> = posts.iter().enumerate().map(|(i, p)| (p.no, i)).collect();
        let mut backlinks = vec![Vec::new(); posts.len()];
        for p in &posts {
            for q in &p.quotes {
                if let Some(&i) = index.get(q)
                    && !backlinks[i].contains(&p.no)
                {
                    backlinks[i].push(p.no);
                }
            }
        }
        let entries = (0..posts.len()).map(|i| Entry { post: i, depth: 0, path: vec![posts[i].no] }).collect();
        Self { entries, board, no, posts, index, backlinks, ..Default::default() }
    }

    /// The post is collapsed to a line: hidden, not shown anyway, and not the OP.
    pub fn is_collapsed(&self, i: usize) -> bool {
        i > 0 && !self.show_hidden && self.marks.get(i).is_some_and(|m| m.hidden.is_some())
    }

    /// The selected post.
    pub fn current(&self) -> Option<&Post> {
        self.posts.get(self.selected)
    }

    pub fn is_revealed(&self, i: usize) -> bool {
        self.reveal_all || self.revealed.contains(&i)
    }

    /// Recompute matches for the current query and re-render.
    pub fn set_search(&mut self, query: String) {
        let needle = query.to_lowercase();
        self.matches = if needle.is_empty() {
            Vec::new()
        } else {
            let mut texts = std::mem::take(&mut self.search_texts);
            texts.resize(self.posts.len(), None);
            let mut found = |i: usize| {
                if self.is_revealed(i) {
                    return self.post_text(i).contains(&needle);
                }
                texts[i].get_or_insert_with(|| self.post_text(i)).contains(&needle)
            };
            let m = (0..self.posts.len()).filter(|&i| self.in_view(i) && found(i)).collect();
            self.search_texts = texts;
            m
        };
        self.search = query;
        self.layout = None;
    }

    /// Lowercase searchable text of a post: name, subject, files and body (hidden spoilers excluded).
    fn post_text(&self, i: usize) -> String {
        let p = &self.posts[i];
        let files = p.files.iter().map(|f| f.filename.as_str());
        if self.is_revealed(i) {
            let mut text = String::new();
            for line in &p.body {
                text.extend(line.spans.iter().map(|s| s.content.as_ref()));
                text.push(' ');
            }
            crate::model::search_haystack(&p.name, p.subject.as_deref(), files, &text)
        } else {
            crate::model::search_haystack(&p.name, p.subject.as_deref(), files, p.plain_text())
        }
    }

    /// The next (or previous) matching post after the selection, wrapping around.
    fn next_match(&self, forward: bool) -> Option<usize> {
        if forward {
            self.matches.iter().find(|&&i| i > self.selected).or(self.matches.first()).copied()
        } else {
            self.matches.iter().rev().find(|&&i| i < self.selected).or(self.matches.last()).copied()
        }
    }

    /// Whether post `i` is shown: always, unless a conversation is, without it.
    pub fn in_view(&self, i: usize) -> bool {
        self.conversation.as_ref().is_none_or(|c| c.depth.contains_key(&i))
    }

    /// `c`: show the selected post's conversation alone.
    pub fn enter_conversation(&mut self) -> Result<usize, String> {
        let Some(p) = self.current() else { return Err("No posts".into()) };
        let no = p.no;
        let quotes = p.quotes.iter().any(|q| self.index.contains_key(q));
        if !quotes && self.backlinks[self.selected].is_empty() {
            return Err(format!("No.{no} isn't part of a conversation here: it quotes no post in the thread, and none quote it"));
        }
        self.conversation = Some(Conversation { anchor: no, depth: Default::default(), capped: false, back_scroll: self.scroll });
        self.focus = None;
        self.scroll = 0;
        self.rebuild_entries();
        self.set_search(self.search.clone());
        Ok(self.conversation.as_ref().map_or(0, |c| c.depth.len()))
    }

    /// Back to the whole thread, with the conversation's post selected and the thread
    /// scrolled where it was.
    pub fn leave_conversation(&mut self) {
        let Some(c) = self.conversation.take() else { return };
        self.focus = None;
        if let Some(&i) = self.index.get(&c.anchor) {
            self.selected = i;
        }
        self.rebuild_entries();
        self.scroll = c.back_scroll;
        self.set_search(self.search.clone());
    }

    pub fn is_new(&self, i: usize) -> bool {
        self.new_after > 0 && self.posts[i].no > self.new_after
    }

    /// The selected entry: the cursor if it's on the selected post, else the post's
    /// top-level entry (code that sets `selected` directly lands there).
    pub fn entry(&self) -> usize {
        match self.entries.get(self.cursor) {
            Some(e) if e.post == self.selected => self.cursor,
            _ => self.entries.iter().position(|e| e.path.len() == 1 && e.post == self.selected).unwrap_or(0),
        }
    }

    /// Select an entry (without scrolling).
    pub fn set_cursor(&mut self, e: usize) {
        if let Some(entry) = self.entries.get(e) {
            // Another entry: the focus was in the old one.
            if (e != self.cursor || entry.post != self.selected) && self.focus.take().is_some() {
                self.layout = None;
            }
            self.cursor = e;
            self.selected = entry.post;
        }
    }

    /// The parts of entry `e`'s post (none when it's collapsed).
    pub fn parts_of(&self, e: usize) -> Vec<Part> {
        let Some(&Entry { post, .. }) = self.entries.get(e) else { return Vec::new() };
        match (self.posts.get(post), self.backlinks.get(post)) {
            (Some(p), Some(b)) if !self.is_collapsed(post) => parts(p, b),
            _ => Vec::new(),
        }
    }

    /// Focus the next (or previous) part: on through the selected post's parts, then the
    /// next post's. The post itself comes before its parts. False at either end.
    pub fn step_part(&mut self, forward: bool) -> bool {
        let mut e = self.entry();
        let parts = self.parts_of(e);
        let at = self.focus.as_ref().and_then(|f| parts.iter().position(|p| p == f));
        let next = match (at, forward) {
            (None, true) => Some(0),
            (None, false) => None,
            (Some(i), true) => Some(i + 1),
            (Some(i), false) => i.checked_sub(1),
        };
        let found = match next.and_then(|i| parts.get(i)) {
            Some(part) => Some((e, part.clone())),
            // The post itself, from its first part.
            None if !forward && at.is_some() => {
                self.focus = None;
                self.layout = None;
                self.follow_focus = true;
                return true;
            }
            None => loop {
                e = if forward { e + 1 } else if let Some(p) = e.checked_sub(1) { p } else { break None };
                if e >= self.entries.len() {
                    break None;
                }
                let parts = self.parts_of(e);
                if let Some(part) = if forward { parts.first() } else { parts.last() } {
                    break Some((e, part.clone()));
                }
            },
        };
        let Some((e, part)) = found else { return false };
        self.set_cursor(e);
        self.focus = Some(part);
        self.layout = None;
        self.follow_focus = true;
        true
    }

    /// Rebuild `entries` from `expanded` (and the conversation, which is worked out again:
    /// a refresh may add to it), keeping the cursor on the same path if it's still there.
    fn rebuild_entries(&mut self) {
        let path = self.entries.get(self.entry()).map(|e| e.path.clone());
        if let Some(c) = &mut self.conversation {
            match self.index.get(&c.anchor) {
                Some(&p) => (c.depth, c.capped) = conversation_of(&self.posts, &self.index, &self.backlinks, p),
                None => self.conversation = None,
            }
        }
        let shown: Vec<(usize, i32)> = match &self.conversation {
            Some(c) => c.depth.iter().map(|(&i, &d)| (i, d)).collect(),
            None => (0..self.posts.len()).map(|i| (i, 0)).collect(),
        };
        let mut out = Vec::with_capacity(shown.len());
        for (i, d) in shown {
            let path = vec![self.posts[i].no];
            // Replies are indented by how far down from the conversation's post they are.
            let depth = d.clamp(0, MAX_DEPTH as i32 - 1) as u8;
            out.push(Entry { post: i, depth, path: path.clone() });
            self.push_replies(&mut out, i, path, depth + 1);
        }
        self.entries = out;
        self.layout = None;
        match path.and_then(|p| self.entries.iter().position(|e| e.path == p)) {
            Some(e) => self.set_cursor(e),
            // The selected post, if it's still shown.
            None => match self.entries.iter().position(|e| e.path.len() == 1 && e.post == self.selected) {
                Some(e) => self.set_cursor(e),
                None => {
                    self.cursor = 0;
                    self.selected = self.entries.first().map_or(0, |e| e.post);
                }
            },
        }
    }

    fn push_replies(&self, out: &mut Vec<Entry>, post: usize, path: Vec<u64>, depth: u8) {
        if depth > MAX_DEPTH || !self.expanded.contains(&path) {
            return;
        }
        for no in &self.backlinks[post] {
            // A post can't contain itself (quote loops).
            let Some(&j) = self.index.get(no).filter(|_| !path.contains(no)) else { continue };
            let mut p = path.clone();
            p.push(*no);
            out.push(Entry { post: j, depth, path: p.clone() });
            self.push_replies(out, j, p, depth + 1);
        }
    }

    /// `e`: show or hide the selected entry's replies under it. Returns what happened.
    fn toggle_expanded(&mut self) -> Result<bool, &'static str> {
        let Some(e) = self.entries.get(self.entry()) else { return Err("No posts") };
        if self.backlinks.get(e.post).is_none_or(Vec::is_empty) {
            return Err("No replies to this post");
        }
        if e.depth >= MAX_DEPTH {
            return Err("Replies are expanded as deep as they go here");
        }
        let path = e.path.clone();
        let open = !self.expanded.remove(&path);
        if open {
            self.expanded.insert(path);
        }
        self.rebuild_entries();
        Ok(open)
    }

    /// The entry at the top of the view and how many of its lines are scrolled past.
    /// Whether the end of the thread is being read: the last entry shown is selected, and
    /// the end is on screen.
    pub fn at_end(&self) -> bool {
        let Some(l) = &self.layout else { return false };
        // The last line is the gap after the last post.
        self.viewport > 0 && self.entry() + 1 == self.entries.len() && self.scroll + self.viewport + 1 >= l.len()
    }

    /// New posts whose entries start below the screen.
    pub fn new_below(&self) -> usize {
        let Some(l) = &self.layout else { return 0 };
        let bottom = self.scroll + self.viewport;
        self.entries.iter().enumerate().filter(|&(e, x)| l.starts.get(e).is_some_and(|&s| s >= bottom) && self.is_new(x.post)).count()
    }

    fn top_anchor(&self) -> Option<(usize, usize)> {
        let l = self.layout.as_ref()?;
        let top = l.starts.partition_point(|&s| s <= self.scroll).saturating_sub(1);
        Some((top, self.scroll - l.starts[top]))
    }

    /// Select a post's top-level entry. A post outside the conversation shown leaves it.
    pub fn select(&mut self, i: usize) {
        let i = i.min(self.posts.len().saturating_sub(1));
        if !self.in_view(i) {
            self.leave_conversation();
        }
        let e = self.entries.iter().position(|e| e.path.len() == 1 && e.post == i).unwrap_or(0);
        self.set_cursor(e.min(self.entries.len().saturating_sub(1)));
        self.scroll_to(Reveal::Jump);
        self.reveal = Some(Reveal::Jump);
    }

    /// `j` / `k`: within a post taller than the screen, on to its next (or previous)
    /// screenful, keeping two lines; past its end (or top), the next (or previous) post.
    pub fn step(&mut self, down: bool) {
        let e = self.entry();
        let view = self.viewport.max(1);
        if let Some(l) = self.layout.as_ref().filter(|l| l.exact.get(e) == Some(&true)) {
            let (start, end) = (l.starts[e], l.starts[e + 1]);
            // The last line is the gap before the next post: it's past the end that matters.
            let last = end.saturating_sub(1);
            let page = view.saturating_sub(2).max(1);
            let on_screen = start < self.scroll + view && last > self.scroll;
            if down && on_screen && last > self.scroll + view {
                self.scroll = (self.scroll + page).min(last.saturating_sub(view)).min(l.len().saturating_sub(view));
                return;
            }
            if !down && on_screen && start < self.scroll {
                self.scroll = self.scroll.saturating_sub(page).max(start);
                return;
            }
        }
        if down {
            self.select_entry(e + 1);
        } else if e > 0 {
            self.select_entry(e - 1);
            self.reveal = Some(Reveal::End);
        }
    }

    /// The selected post's place when it's taller than the screen: which screenful is
    /// shown, of how many; and whether it goes on above and below the screen.
    pub fn tall(&self) -> Option<((usize, usize), bool, bool)> {
        let l = self.layout.as_ref()?;
        let e = self.entry();
        let view = self.viewport.max(1);
        let (start, last) = (*l.starts.get(e)?, l.starts.get(e + 1)?.saturating_sub(1));
        if last - start <= view || !l.exact.get(e).copied().unwrap_or(false) {
            return None;
        }
        let page = view.saturating_sub(2).max(1);
        let pages = (last - start - view).div_ceil(page) + 1;
        let at = ((self.scroll.saturating_sub(start)).div_ceil(page) + 1).min(pages);
        Some(((at, pages), start < self.scroll, last > self.scroll + view))
    }

    fn select_entry(&mut self, e: usize) {
        self.set_cursor(e.min(self.entries.len().saturating_sub(1)));
        self.scroll_to(Reveal::Step);
        // Placed by estimates so far; exactly once it's laid out.
        self.reveal = Some(Reveal::Step);
    }

    /// The scroll margin in lines: at most what leaves a line between the two.
    fn margin_lines(&self) -> usize {
        let view = self.viewport;
        ((view as f32 * self.margin.clamp(0.0, 0.5)) as usize).min(view.saturating_sub(1) / 2)
    }

    /// Adjust scroll so the selected entry is visible (its top, if it's taller than the view).
    pub fn scroll_to_selected(&mut self) {
        self.scroll_to(Reveal::Visible);
    }

    /// Scroll the selected entry into view, `how`.
    pub fn scroll_to(&mut self, how: Reveal) {
        let e = self.entry();
        let Some((&start, &end, len)) = self.layout.as_ref().and_then(|l| Some((l.starts.get(e)?, l.starts.get(e + 1)?, l.len()))) else { return };
        let view = self.viewport;
        let m = if how == Reveal::Visible { 0 } else { self.margin_lines() };
        match how {
            // Come to from below, a post taller than the screen shows its end.
            Reveal::End if end - start > view => self.scroll = end.saturating_sub(1 + view),
            // A jump lands with the target's top at the margin, unless it's well in view.
            Reveal::Jump if m > 0 => {
                if start < self.scroll + m || end > self.scroll + view - m {
                    self.scroll = start.saturating_sub(m);
                }
            }
            // Up past the top margin: the post's bottom goes to the margin from the bottom
            // (its top, if it's too tall for that).
            _ if start < self.scroll + m => {
                self.scroll = if m == 0 { start } else { (end + m).saturating_sub(view).min(start) };
            }
            // Down past the bottom margin: the post's top goes to the margin from the top.
            _ if end + m > self.scroll + view => {
                self.scroll = if m == 0 { start.min(end.saturating_sub(view)) } else { start.saturating_sub(m) };
            }
            _ => {}
        }
        self.scroll = self.scroll.min(len.saturating_sub(view));
    }

    /// Scroll by lines, then select the entry at the top of the view.
    fn scroll_lines(&mut self, delta: isize) {
        let Some(l) = &self.layout else { return };
        let max = l.len().saturating_sub(self.viewport);
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
        let top = l.starts.partition_point(|&s| s <= self.scroll).saturating_sub(1);
        // Prefer an entry whose header is on screen.
        let e = if l.starts[top] < self.scroll && top + 1 < self.entries.len() && l.starts[top + 1] < self.scroll + self.viewport {
            top + 1
        } else {
            top
        };
        self.set_cursor(e);
    }

    pub(crate) fn jump_to(&mut self, no: u64) -> bool {
        let Some(&i) = self.index.get(&no) else { return false };
        self.jumps.push(self.selected);
        self.select(i);
        true
    }
}

/// Wall clock for timestamps and "3h ago", and the monotonic clock for deadlines. Tests fix
/// the first (and format in UTC) so snapshots don't depend on when or where they run; the
/// fuzzer runs both on its own time.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clock {
    pub fixed: Option<i64>,
    pub instant: Option<Instant>,
}

impl Clock {
    pub fn now(&self) -> i64 {
        self.fixed.unwrap_or_else(|| chrono::Utc::now().timestamp())
    }

    pub fn instant(&self) -> Instant {
        self.instant.unwrap_or_else(Instant::now)
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

enum DlEvent {
    Done,
    Skipped,
    Failed(String),
    Finished,
}

/// Where the list or thread was last drawn, for mouse clicks.
#[derive(Debug, Clone, Copy)]
pub enum Hit {
    /// A list: its area, first visible item, and rows per item.
    List { area: Rect, offset: usize, item_height: u16 },
    Thread { area: Rect },
    /// The catalog grid: its area, first visible item, columns, and cell size.
    Grid { area: Rect, offset: usize, cols: usize, cell: (u16, u16) },
    /// The settings screen, laid out as `settings::rows()` from `offset`.
    Settings { area: Rect, offset: usize },
}

/// A message in the footer, for a moment (errors a little longer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

impl Status {
    /// How long it replaces the key hints.
    fn ttl(&self) -> Duration {
        Duration::from_secs(if self.error { 5 } else { 2 })
    }
}

/// New posts in a watched thread, for a notification.
struct Note {
    key: ThreadKey,
    subject: String,
    new: usize,
    /// Of those, replies to your posts.
    replies: usize,
}

/// Popup with the posts the selected post quotes.
pub struct Preview {
    /// Indices of quoted posts in this thread.
    pub posts: Vec<usize>,
    /// Quoted post numbers that aren't in this thread.
    pub elsewhere: Vec<u64>,
    pub scroll: u16,
}

/// Full-screen viewer over a post's files, or a thread's.
pub struct Viewer {
    pub files: Vec<Attachment>,
    pub index: usize,
    /// Link to the post the files are from (one post's).
    pub link: Option<String>,
    /// The post each file is from, when they're a thread's (else empty).
    pub posts: Vec<u64>,
    /// How far it's zoomed in, and where.
    pub crop: crate::images::Crop,
    /// How much of the image is on screen (thousandths of its width and height), as last
    /// drawn: what moving around steps by.
    pub shown: Option<(u16, u16)>,
}

impl Viewer {
    pub fn new(files: Vec<Attachment>, index: usize, link: Option<String>) -> Self {
        Viewer { files, index, link, posts: Vec::new(), crop: crate::images::Crop::FIT, shown: None }
    }
}

enum Msg {
    /// The request was answered from the cache without hitting the network.
    Cached(u64, Duration),
    Boards(u64, usize, Result<Vec<Board>>),
    /// The pages of a board list loaded so far; more are coming.
    BoardsPartial(u64, usize, Vec<Board>),
    /// A saved board list refreshed in the background.
    BoardsRefreshed(usize, Result<Vec<Board>>),
    Catalog(u64, Result<Vec<Post>>),
    CatalogPartial(u64, Vec<Post>),
    Thread(u64, Result<Vec<Post>>),
    /// The last copy kept of the catalog or thread being fetched, and when it was fetched.
    CachedCatalog(u64, Vec<Post>, i64),
    CachedThread(u64, Vec<Post>, i64),
    /// A background refresh of a watched or open thread.
    Refreshed(ThreadKey, Result<Vec<Post>>),
    /// A page of archive search results.
    Search(u64, u32, Result<crate::backend::SearchPage>),
    /// The catalog of a followed general's board, to find its next thread.
    GeneralCatalog(ThreadKey, Result<Vec<Post>>),
    /// The thread a quoted post is in: (board, post, thread).
    Found(u64, Board, u64, Result<Option<u64>>),
    Download(DlEvent),
    /// What a site runs, or has (for adding it).
    Detected(u64, Result<sites::Detected>),
    /// What a search of the saved threads found in one more copy.
    SavedSearch(u64, search::SavedFound),
    Input(Event),
    /// Something else (a loaded image) needs a redraw.
    Wake,
}

impl Msg {
    /// The request a response is for.
    fn id(&self) -> Option<u64> {
        match self {
            Msg::Cached(id, _)
            | Msg::Boards(id, ..)
            | Msg::BoardsPartial(id, ..)
            | Msg::Catalog(id, _)
            | Msg::CatalogPartial(id, _)
            | Msg::CachedCatalog(id, ..)
            | Msg::CachedThread(id, ..)
            | Msg::Thread(id, _)
            | Msg::Search(id, ..)
            | Msg::Found(id, ..) => Some(*id),
            _ => None,
        }
    }
}

pub struct App {
    pub sites: Vec<Site>,
    pub site_list: Picker,
    /// Favorite boards (from the config), and board titles for the home screen.
    pub favorites: Vec<BoardRef>,
    pub home_titles: HashMap<String, String>,
    /// Sites left off the home screen (from the config), and whether they're shown anyway.
    pub hidden_sites: std::collections::BTreeSet<String>,
    pub show_hidden_sites: bool,
    /// The catalog layout for boards without their own (`catalog_layout` in the config).
    pub default_layout: CatalogLayout,
    /// Columns of the catalog grid as last drawn (0: not a grid).
    pub grid_cols: usize,
    /// The settings popup open, if any.
    pub settings_popup: Option<SettingsPopup>,
    /// The menu for what's selected (`.`), and link hints on screen (`f`).
    pub menu: Option<Menu>,
    pub hints: Option<Hints>,
    pub settings_list: Picker,
    /// The current theme's name, and the config's custom themes.
    pub theme_name: String,
    pub themes: BTreeMap<String, ThemeDef>,
    pub color_mode: ColorMode,
    /// Draw 24-bit colors (else the nearest of 256).
    pub truecolor: bool,
    pub images_mode: ImagesMode,
    pub watched_list: Picker,
    pub history_list: Picker,
    pub saved_list: Picker,
    /// The saved copy `x` was pressed on once: a second `x` removes it.
    pub saved_confirm: Option<ThreadKey>,
    /// A big save asking first (see `ask_to_save`).
    pub confirm: Option<saving::Confirm>,
    /// Adding a site (see `add_site_from`).
    pub adding: Option<sites::Adding>,
    /// The search of saved threads that's wanted; a running one stops when it changes.
    saved_search: Arc<std::sync::atomic::AtomicU64>,
    /// Sites taken out of the config in Settings: off the home screen until ck restarts.
    pub removed_sites: std::collections::BTreeSet<String>,
    pub store: Store,
    refresh_thread: Duration,
    refresh_watched: Duration,
    watched_checked: HashMap<ThreadKey, Instant>,
    /// Background refreshes in flight.
    pub refreshing: HashSet<ThreadKey>,
    /// The newest post seen in each watched thread by a refresh this session; notifications
    /// are for posts past it.
    notified_max: HashMap<ThreadKey, u64>,
    /// Followed generals: when each one's board was last searched, and searches running.
    generals_checked: HashMap<ThreadKey, Instant>,
    generals_searching: HashSet<ThreadKey>,
    /// When each (site, board) catalog was last fetched for them.
    general_boards: HashMap<(String, String), Instant>,
    /// Notifications waiting to be sent together, and since when.
    notes: Vec<Note>,
    notes_since: Option<Instant>,
    pub notify_mode: crate::notify::NotifyMode,
    pub notify_command: Option<Vec<String>>,
    /// Notifications sent (the last few), for the record.
    pub notified: Vec<String>,
    /// Sites whose saved board list is being refreshed in the background, and when each was
    /// last tried (a failed one isn't retried within `MIN_REFETCH`).
    boards_refreshing: HashSet<usize>,
    boards_tried: HashMap<usize, Instant>,
    pub filters: Filters,
    /// The last copies of catalogs and threads, to open them at once (none in tests).
    pub pages: Option<crate::pages::Pages>,
    /// `scroll_margin`: where the selected post sits while reading (a fraction of the screen).
    pub scroll_margin: f32,
    /// The config's `[[filter]]` tables, as last read or written (`filters` is made from them).
    pub filter_cfgs: Vec<crate::filter::FilterConfig>,
    /// `X`: a filter being made from the selected post.
    pub filter_add: Option<AddFilter>,
    /// The filter just added (and where): `u` as the next key takes it back.
    pub filter_undo: Option<(usize, crate::filter::FilterConfig)>,
    /// Show hidden threads and posts (dimmed) instead of leaving them out.
    pub show_hidden: bool,
    /// True while typing into the filter.
    pub filtering: bool,
    pub status: Option<Status>,
    /// The status message as last seen by `expire_status`, and when it appeared.
    status_since: Option<(String, Instant)>,
    pub show_help: bool,
    pub help_scroll: u16,
    pub images: Images,
    /// Reverse image search engines (`R`), and the panel choosing one.
    pub image_search: Vec<crate::config::ImageSearch>,
    pub image_search_panel: Option<ImageSearchPanel>,
    /// Save where you are and start there next time.
    pub restore_session: bool,
    /// Reading the end of a thread, new posts come into view (`follow_new_posts`).
    pub follow_new_posts: bool,
    /// Images on boards the site marks NSFW (`nsfw_images`).
    pub nsfw_images: crate::config::NsfwImages,
    /// NSFW boards of sites whose board list isn't loaded, from their saved lists; and the
    /// sites asked for theirs, once, to know.
    nsfw_saved: HashMap<usize, HashSet<String>>,
    nsfw_asked: HashSet<usize>,
    /// The data directory has changes to write, and when it was last written.
    save_pending: bool,
    saved_at: Instant,
    /// The session as last saved, and when that was checked.
    session_saved: (Option<crate::store::Session>, Instant),
    /// The archive search query being typed.
    pub search_input: Option<String>,
    /// True while typing a thread search.
    pub searching: bool,
    /// What's typed after `:`, while it's being typed.
    pub goto: Option<String>,
    pub keys: KeyMap,
    /// The last text copied to the clipboard, and the last URL opened.
    pub copied: Option<String>,
    pub opened: Option<String>,
    /// The config file that settings are saved to (tests point it elsewhere).
    pub config_path: Option<std::path::PathBuf>,
    pub clock: Clock,
    pub downloads: Downloads,
    pub(crate) download_dir: Option<String>,
    /// Set by the UI every frame.
    pub hit: Option<Hit>,
    /// Where each tab's chip was drawn, for clicks.
    pub tab_chips: Vec<(Rect, usize)>,
    /// Last left click: when, and the list index or thread post it hit.
    last_click: Option<(Instant, usize)>,
    pub tick: usize,
    pub quit: bool,
    /// The active tab's place; the other tabs (the active one's slot holds nothing useful),
    /// and which is active.
    pub tab: Tab,
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// The last request id given out (ids are unique across tabs).
    next_id: u64,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl App {
    pub fn new(cfg: Config, keys: KeyMap, picker: Option<ratatui_image::picker::Picker>, store: Store) -> Self {
        // ck 0.2's [theme] table of overrides becomes a theme of its own, "legacy".
        let mut themes = cfg.themes.clone();
        let theme_name = match &cfg.theme {
            None => theme::DEFAULT_THEME.to_string(),
            Some(ThemeSetting::Name(n)) => n.clone(),
            Some(ThemeSetting::Legacy(old)) => {
                let colors = old
                    .iter()
                    .map(|(k, v)| {
                        let role = theme::LEGACY_KEYS.iter().find(|(l, _)| l == k).map_or(k.as_str(), |(_, r)| r);
                        (role.to_string(), v.clone())
                    })
                    .collect();
                let def = ThemeDef { base: Some(theme::DEFAULT_THEME.into()), colors, ..Default::default() };
                themes.insert("legacy".into(), def);
                "legacy".into()
            }
        };
        let layout = cfg.layout();
        let refresh_thread = Duration::from_secs(cfg.refresh_thread_secs.max(10));
        let refresh_watched = Duration::from_secs(cfg.refresh_watched_secs.max(60));
        let mut store = store;
        store.saved_max = cfg.saved_max_mb.saturating_mul(1024 * 1024);
        let sites = cfg
            .sites
            .into_iter()
            .map(|cfg| Site { backend: backend::build(&cfg), cfg, boards: None })
            .collect();
        let (tx, rx) = channel();
        let mut app = Self {
            sites,
            site_list: Picker::top(),
            favorites: cfg.favorites.iter().filter_map(|f| BoardRef::parse(f)).collect(),
            home_titles: HashMap::new(),
            hidden_sites: cfg.hidden_sites.iter().cloned().collect(),
            show_hidden_sites: false,
            grid_cols: 0,
            default_layout: store.settings.catalog_layout.unwrap_or(match store.settings.compact_catalog {
                Some(true) => CatalogLayout::Compact,
                Some(false) => CatalogLayout::Cards,
                None => layout,
            }),
            settings_popup: None,
            menu: None,
            hints: None,
            settings_list: Picker::top(),
            theme_name,
            themes,
            color_mode: cfg.color,
            truecolor: cfg.color.truecolor(),
            images_mode: cfg.images,
            watched_list: Picker::top(),
            history_list: Picker::top(),
            saved_list: Picker::top(),
            saved_confirm: None,
            confirm: None,
            adding: None,
            saved_search: Arc::default(),
            removed_sites: Default::default(),
            store,
            refresh_thread,
            refresh_watched,
            watched_checked: HashMap::new(),
            refreshing: HashSet::new(),
            notified_max: HashMap::new(),
            generals_checked: HashMap::new(),
            generals_searching: HashSet::new(),
            general_boards: HashMap::new(),
            notes: Vec::new(),
            notes_since: None,
            notify_mode: cfg.notify,
            notify_command: cfg.notify_command.clone(),
            notified: Vec::new(),
            boards_refreshing: HashSet::new(),
            boards_tried: HashMap::new(),
            filters: Filters::new(&cfg.filters).unwrap_or_default(),
            filter_cfgs: cfg.filters.clone(),
            scroll_margin: if cfg.scroll_margin.is_finite() { cfg.scroll_margin.clamp(0.0, 0.5) } else { 0.3 },
            pages: crate::pages::Pages::default_dir()
                .filter(|_| !cfg!(test) && cfg.page_cache_mb > 0)
                .map(|d| crate::pages::Pages::new(d, cfg.page_cache_mb.saturating_mul(1024 * 1024))),
            filter_add: None,
            filter_undo: None,
            show_hidden: false,
            filtering: false,
            status: None,
            status_since: None,
            show_help: false,
            help_scroll: 0,
            images: Images::new(picker, {
                let tx = tx.clone();
                Arc::new(move || {
                    let _ = tx.send(Msg::Wake);
                })
            }, DiskCache::default_dir().map(|d| DiskCache::new(d, crate::disk_cache::BUDGET))),
            image_search: if cfg.image_search.is_empty() { crate::config::ImageSearch::defaults() } else { cfg.image_search.clone() },
            image_search_panel: None,
            restore_session: cfg.restore_session,
            follow_new_posts: cfg.follow_new_posts,
            nsfw_images: cfg.nsfw_images,
            nsfw_saved: HashMap::new(),
            nsfw_asked: HashSet::new(),
            save_pending: false,
            saved_at: Instant::now(),
            session_saved: (None, Instant::now()),
            search_input: None,
            searching: false,
            goto: None,
            keys,
            copied: None,
            opened: None,
            config_path: Config::path(),
            clock: Clock::default(),
            downloads: Downloads::default(),
            download_dir: cfg.download_dir.clone(),
            hit: None,
            tab_chips: Vec::new(),
            last_click: None,
            tick: 0,
            quit: false,
            tab: Tab::new(0, Instant::now()),
            tabs: vec![Tab::new(0, Instant::now())],
            active: 0,
            next_id: 0,
            tx,
            rx,
        };
        app.load_home_titles();
        app
    }

    // ----- visible (filtered) items -----

    pub fn visible_sites(&self) -> Vec<SiteRow> {
        let rows: Vec<SiteRow> = [SiteRow::Watched, SiteRow::History, SiteRow::Saved]
            .into_iter()
            .chain((0..self.favorites.len()).map(SiteRow::Favorite))
            .chain(self.recent_rows().into_iter().map(SiteRow::Recent))
            .chain(
                (0..self.sites.len())
                    .filter(|&i| !self.removed_sites.contains(&self.sites[i].cfg.name))
                    .filter(|&i| self.show_hidden_sites || !self.is_site_hidden(i))
                    .map(SiteRow::Site),
            )
            .chain((!self.hidden_sites.is_empty()).then_some(SiteRow::HiddenSites))
            .collect();
        let name = |k: usize| match &rows[k] {
            SiteRow::Watched => "Watched".to_string(),
            SiteRow::History => "History".to_string(),
            SiteRow::Saved => "Saved".to_string(),
            SiteRow::Favorite(i) => {
                let f = &self.favorites[*i];
                format!("{} /{}/ {}", f.site, f.board, self.board_title(f))
            }
            SiteRow::Recent(i) => {
                let r = self.recent_board(*i);
                r.map_or(String::new(), |r| format!("{} /{}/ {}", r.site, r.board, self.board_title(&r)))
            }
            SiteRow::Site(i) => self.sites[*i].cfg.name.clone(),
            SiteRow::HiddenSites => "hidden sites".to_string(),
        };
        filtered(&self.site_list.filter, rows.len(), name).into_iter().map(|i| rows[i]).collect()
    }

    pub fn visible_watched(&self) -> Vec<usize> {
        let w = &self.store.watched;
        filtered(&self.watched_list.filter, w.len(), |i| format!("{} {} {} {}", w[i].key.site, w[i].key.board, w[i].key.no, w[i].subject))
    }

    pub fn visible_saved(&self) -> Vec<usize> {
        let s = &self.store.saved;
        filtered(&self.saved_list.filter, s.len(), |i| format!("{} {} {} {}", s[i].key.site, s[i].key.board, s[i].key.no, s[i].subject))
    }

    pub fn visible_history(&self) -> Vec<usize> {
        let h = &self.store.history;
        filtered(&self.history_list.filter, h.len(), |i| format!("{} {} {} {}", h[i].key.site, h[i].key.board, h[i].key.no, h[i].subject))
    }

    pub fn boards(&self) -> &[Board] {
        self.sites[self.tab.site].boards.as_deref().unwrap_or(&[])
    }

    pub fn visible_boards(&self) -> Vec<usize> {
        let b = self.boards();
        filtered(&self.tab.board_list.filter, b.len(), |i| format!("{} {}", b[i].uri, b[i].title))
    }

    pub fn visible_catalog(&self) -> Vec<usize> {
        let needle = self.tab.catalog_list.filter.to_lowercase();
        let shown = |i: usize| self.show_hidden || self.tab.catalog_marks.get(i).is_none_or(|m| m.hidden.is_none());
        let mut v: Vec<usize> =
            (0..self.tab.catalog.len()).filter(|&i| shown(i) && self.tab.catalog[i].search_text().contains(&needle)).collect();
        let c = &self.tab.catalog;
        match self.tab.catalog_sort {
            Sort::Bump => {}
            Sort::Replies => v.sort_by_key(|&i| std::cmp::Reverse(c[i].replies.unwrap_or(0))),
            Sort::Newest => v.sort_by_key(|&i| std::cmp::Reverse((c[i].time, c[i].no))),
            Sort::Oldest => v.sort_by_key(|&i| (c[i].time, c[i].no)),
        }
        v
    }

    /// What's on screen, broadly: when it changes, the screen is painted whole.
    pub fn screen(&self) -> (View, usize, bool, bool) {
        (self.tab.view, self.active, self.tab.viewer.is_some(), self.tab.gallery.is_some())
    }

    /// Keep the current list's selection on a row that exists.
    fn clamp_list(&mut self) {
        if let Some((p, len)) = self.picker() {
            p.clamp(len);
        }
    }

    fn picker(&mut self) -> Option<(&mut Picker, usize)> {
        Some(match self.tab.view {
            View::Sites => (self.visible_sites().len(), &mut self.site_list),
            View::Boards => (self.visible_boards().len(), &mut self.tab.board_list),
            View::Catalog => (self.visible_catalog().len(), &mut self.tab.catalog_list),
            View::Watched => (self.visible_watched().len(), &mut self.watched_list),
            View::History => (self.visible_history().len(), &mut self.history_list),
            View::Saved => (self.visible_saved().len(), &mut self.saved_list),
            View::Settings => (settings::items().len(), &mut self.settings_list),
            View::Search => (self.tab.search.as_ref().map_or(0, |s| s.hits.len()), &mut self.tab.search_list),
            View::Thread => return None,
        })
        .map(|(len, p)| (p, len))
    }

    pub fn current_site(&self) -> &Site {
        &self.sites[self.tab.site]
    }

    // ----- background loading -----

    /// Block until something happens (input, a finished request, a loaded image) or
    /// `timeout` passes, then handle everything pending. Returns whether anything arrived.
    pub fn wait(&mut self, timeout: Duration) -> bool {
        let got = self.rx.recv_timeout(timeout).map(|msg| self.handle(msg)).is_ok();
        self.poll();
        got
    }

    /// Handle everything pending without blocking. Queued input is all handled before the
    /// next draw, so held-down keys don't build up a lag.
    pub fn poll(&mut self) {
        self.images.poll();
        for e in self.store.settle() {
            self.error(e);
        }
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg);
        }
        self.background();
        self.flush_notes(self.clock.instant());
        self.save_session(Some(self.clock.instant()));
        if self.save_pending && self.clock.instant().saturating_duration_since(self.saved_at) >= SAVE_EVERY {
            self.save_now();
        }
        self.expire_status(self.clock.instant());
    }

    /// Send waiting notifications together: once no refresh is running, or 3s after the
    /// first, so several threads' news makes one notification.
    fn flush_notes(&mut self, now: Instant) {
        let Some(since) = self.notes_since else { return };
        if !self.refreshing.is_empty() && now.duration_since(since) < Duration::from_secs(3) {
            return;
        }
        let notes = std::mem::take(&mut self.notes);
        self.notes_since = None;
        let method = crate::notify::method(self.notify_mode, self.notify_command.as_deref(), &|k| std::env::var(k).ok());
        let place = |n: &Note| format!("/{}/ {}", n.key.board, n.subject.chars().take(60).collect::<String>());
        let replies: Vec<&Note> = notes.iter().filter(|n| n.replies > 0).collect();
        let mut messages = Vec::new();
        match replies.as_slice() {
            [] => {}
            [n] if n.replies == 1 => messages.push(format!("New reply to your post in {}", place(n))),
            [n] => messages.push(format!("{} new replies to your posts in {}", n.replies, place(n))),
            many => {
                let total: usize = many.iter().map(|n| n.replies).sum();
                messages.push(format!("{total} new replies to your posts in {} threads", many.len()));
            }
        }
        let others: Vec<&Note> = notes.iter().filter(|n| n.new > n.replies).collect();
        match others.as_slice() {
            [] => {}
            [n] => {
                let k = n.new - n.replies;
                messages.push(format!("{k} new post{} in {}", if k == 1 { "" } else { "s" }, place(n)));
            }
            many => messages.push(format!("{} watched threads have new posts", many.len())),
        }
        for m in &messages {
            if let Err(e) = crate::notify::send(&method, "ck", m) {
                self.error(format!("Couldn't notify: {e:#}"));
            }
        }
        if let Some(m) = messages.first() {
            self.status.get_or_insert_with(|| Status { text: m.clone(), error: false });
        }
        self.notified.extend(messages);
        let len = self.notified.len();
        self.notified.drain(..len.saturating_sub(20));
    }

    /// How long the main loop may sleep: until the next animation frame, status expiry or
    /// due refresh, and at most a second (relative times like "5s ago" stay current).
    pub fn next_wake(&self, now: Instant) -> Duration {
        let mut wake = Duration::from_secs(1);
        let animating = self.tab.loading.is_some() || !self.refreshing.is_empty() || self.downloads.running > 0 || self.tab.viewer.is_some();
        if animating {
            wake = wake.min(Duration::from_millis(100));
        }
        let mut at = |t: Instant| wake = wake.min(t.saturating_duration_since(now));
        if let Some(since) = self.notes_since {
            at(since + Duration::from_secs(3));
        }
        if self.save_pending {
            at(self.saved_at + SAVE_EVERY);
        }
        // An animated GIF in the viewer: its next frame.
        if let Some(t) = self.images.next_frame() {
            at(t);
        }
        if let (Some(s), Some((_, since))) = (&self.status, &self.status_since) {
            at(*since + s.ttl());
        }
        if self.tab.view == View::Thread && self.tab.thread.is_some() && self.tab.loading.is_none() && self.tab.offline.is_none() {
            at(self.tab.thread_checked + self.refresh_thread);
        }
        // At capacity, a finished refresh wakes the loop anyway (and due ones mustn't spin it).
        if self.refreshing.len() < MAX_REFRESHING {
            for w in self.store.watched.iter().filter(|w| !w.dead && !self.refreshing.contains(&w.key)) {
                at(self.watched_checked.get(&w.key).map_or(now, |t| *t + self.refresh_watched));
            }
        }
        wake
    }

    /// Read terminal input on a thread, into the same channel as everything else. Start it
    /// only after image protocol detection, which reads stdin itself.
    pub fn listen_for_input(&self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = event::read() {
                crate::input_log::note(|| format!("read    {ev:?}"));
                if tx.send(Msg::Input(ev)).is_err() {
                    return;
                }
            }
        });
    }

    fn handle(&mut self, msg: Msg) {
        // A response to another request: another tab's is handled there, the rest are stale
        // (but a finished board list is worth keeping anyway).
        let other = msg.id().filter(|&id| id != self.tab.req);
        if let Some(i) = other.and_then(|id| self.tab_of(id)) {
            self.handle_in_tab(i, msg);
            return;
        }
        if other.is_some() && !matches!(msg, Msg::Boards(..)) {
            return;
        }
        if let Msg::Input(ev) = &msg {
            let acted = !matches!(ev, Event::Key(k) if k.kind != KeyEventKind::Press);
            crate::input_log::note(|| format!("handle  {ev:?}{}", if acted { "" } else { "  (not a press: ignored)" }));
        }
        match msg {
            Msg::Wake => {}
            Msg::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => self.on_key(key),
            Msg::Input(Event::Mouse(m)) => self.on_mouse(m, self.clock.instant()),
            Msg::Input(Event::Paste(text)) => self.paste(&text),
            Msg::Input(_) => {}
            Msg::Refreshed(key, res) => self.refreshed(key, res),
            Msg::GeneralCatalog(key, res) => self.general_catalog(key, res),
            Msg::Download(ev) => self.download_event(ev),
            Msg::Detected(id, res) => self.detected(id, res),
            Msg::SavedSearch(id, found) => self.saved_found(id, found),
            Msg::Found(_, board, post, res) => {
                self.tab.loading = None;
                match res {
                    Ok(Some(no)) => {
                        if let Some(t) = &self.tab.thread {
                            self.tab.trail.push((self.tab.site, self.tab.board.clone().unwrap_or(board.clone()), t.no, t.current().map_or(t.no, |p| p.no)));
                        }
                        let in_settings = self.tab.view == View::Settings;
                        self.open_thread_at(board, no, Some(post));
                        // Found while the settings were open: the thread is behind them.
                        if in_settings {
                            self.tab.settings_back = Some(View::Thread);
                            self.tab.view = View::Settings;
                        }
                    }
                    Ok(None) => {
                        self.error(format!("Post {post} isn't in this thread, and this site can't say which thread it's in"));
                    }
                    Err(e) => self.error(e),
                }
            }
            Msg::Search(_, page, res) => {
                self.tab.loading = None;
                self.search_results(page, res);
            }
            Msg::Cached(_, age) => {
                if self.status.is_none() {
                    self.info(format!("Up to date (checked {}s ago)", age.as_secs()));
                }
            }
            Msg::Boards(_, site, res) => {
                if other.is_none() {
                    self.tab.loading = None;
                }
                match res {
                    Ok(b) => self.set_boards(site, b, true),
                    Err(e) => self.error(e),
                }
            }
            Msg::BoardsPartial(_, site, b) => self.set_boards(site, b, false),
            Msg::BoardsRefreshed(site, res) => {
                self.boards_refreshing.remove(&site);
                // A failed background refresh keeps the saved list; there's nothing to say.
                if let Ok(b) = res {
                    self.set_boards(site, b, true);
                }
            }
            Msg::CachedCatalog(_, posts, fetched) => {
                // Only before anything fetched has arrived.
                if self.tab.catalog.is_empty() && self.tab.view == View::Catalog {
                    self.tab.catalog_cached = Some(tabs::Offline { saved: fetched, dead: false });
                    self.show_catalog(posts);
                    if let Some(no) = self.tab.pending_catalog
                        && let Some(i) = self.visible_catalog().iter().position(|&k| self.tab.catalog[k].no == no)
                    {
                        self.tab.catalog_list.state.select(Some(i));
                    }
                }
            }
            Msg::CachedThread(_, posts, fetched) => {
                if self.tab.thread.is_none() && self.tab.view == View::Thread && self.tab.offline.is_none() {
                    self.set_cached_thread(posts, fetched);
                }
            }
            Msg::CatalogPartial(_, posts) => {
                self.tab.catalog_cached = None;
                self.show_catalog(posts);
            }
            Msg::Catalog(_, res) => {
                self.tab.loading = None;
                match res {
                    Ok(posts) => {
                        self.tab.catalog_cached = None;
                        self.show_catalog(posts);
                        self.catalog_seen();
                        if let Some(no) = self.tab.pending_catalog.take()
                            && let Some(i) = self.visible_catalog().iter().position(|&k| self.tab.catalog[k].no == no)
                        {
                            self.tab.catalog_list.state.select(Some(i));
                        }
                        let len = self.visible_catalog().len();
                        self.tab.catalog_list.clamp(len);
                    }
                    Err(e) => self.error(e),
                }
            }
            Msg::Thread(_, res) => {
                self.tab.loading = None;
                self.tab.thread_checked = self.clock.instant();
                let restoring = std::mem::take(&mut self.tab.restoring);
                match res {
                    Ok(posts) => self.set_thread(posts),
                    // Last session's thread is gone: its catalog instead.
                    Err(e) if restoring && http::is_not_found(&e) => {
                        self.tab.view = View::Catalog;
                        self.load_catalog();
                        self.info("The thread you had open last time is gone (archived or deleted)");
                    }
                    Err(e) if http::is_not_found(&e) => {
                        let Some(key) = self.tab.board.as_ref().map(|b| self.key(&b.uri, self.tab.pending_thread)) else {
                            return;
                        };
                        if let Some(w) = self.store.watched_mut(&key) {
                            w.dead = true;
                            self.save();
                        }
                        self.store.saved_dead(&key);
                        self.thread_gone(&key);
                    }
                    Err(e) => self.error(e),
                }
            }
        }
    }

    /// Status messages replace the footer's key hints only briefly: info for 2 seconds,
    /// errors for 5. A new or changed message restarts the timer.
    fn expire_status(&mut self, now: Instant) {
        let Some(status) = &self.status else {
            self.status_since = None;
            return;
        };
        match &self.status_since {
            Some((seen, since)) if *seen == status.text => {
                if now.duration_since(*since) >= status.ttl() {
                    self.status = None;
                    self.status_since = None;
                }
            }
            _ => self.status_since = Some((status.text.clone(), now)),
        }
    }

    /// Say something in the footer for a moment.
    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: false });
    }

    /// Say something went wrong (shown a little longer).
    pub fn error(&mut self, e: impl std::fmt::Display) {
        self.status = Some(Status { text: format!("{e:#}"), error: true });
    }

    /// Run `job` on a thread, as the current request (shown as `label`). The job gets the
    /// request id and a sender for partial results.
    fn spawn<T: Send + 'static>(
        &mut self,
        label: String,
        job: impl FnOnce(&dyn Backend, u64, &Sender<Msg>) -> Result<T> + Send + 'static,
        wrap: impl FnOnce(u64, Result<T>) -> Msg + Send + 'static,
    ) {
        self.next_id += 1;
        self.tab.req = self.next_id;
        let id = self.tab.req;
        let backend = self.current_site().backend.clone();
        let tx = self.tx.clone();
        self.tab.loading = Some(label);
        self.status = None;
        std::thread::spawn(move || {
            let res = job(&*backend, id, &tx);
            let cached = http::take_cached_age();
            let _ = tx.send(wrap(id, res));
            if let Some(age) = cached {
                let _ = tx.send(Msg::Cached(id, age));
            }
        });
    }

    /// Show a site's boards; a complete list is also saved for next time.
    fn set_boards(&mut self, site: usize, boards: Vec<Board>, complete: bool) {
        if complete && self.sites[site].cfg.boards.is_none() {
            let name = self.sites[site].cfg.name.clone();
            if let Err(e) = self.store.save_boards(&name, &boards, self.clock.now()) {
                self.error(format!("Couldn't save the board list: {e:#}"));
            }
        }
        self.sites[site].boards = Some(boards);
        if site == self.tab.site {
            let len = self.visible_boards().len();
            self.tab.board_list.clamp(len);
        }
        if complete {
            self.note_titles(site);
        }
    }

    /// Refresh a saved board list at background priority, without a spinner.
    fn refresh_boards_in_background(&mut self, site: usize) {
        let now = self.clock.instant();
        if self.boards_tried.get(&site).is_some_and(|t| now.saturating_duration_since(*t) < http::MIN_REFETCH)
            || !self.boards_refreshing.insert(site)
        {
            return;
        }
        self.boards_tried.insert(site, now);
        let backend = self.sites[site].backend.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = http::background(|| backend.boards(&|_| {}));
            let _ = tx.send(Msg::BoardsRefreshed(site, res));
        });
    }

    fn load_boards(&mut self) {
        let site = self.tab.site;
        let label = format!("Loading boards for {}", self.current_site().cfg.name);
        self.spawn(
            label,
            move |b, id, tx| {
                b.boards(&|so_far| {
                    let _ = tx.send(Msg::BoardsPartial(id, site, so_far.to_vec()));
                })
            },
            move |id, r| Msg::Boards(id, site, r),
        );
    }

    fn load_catalog(&mut self) {
        let Some(board) = self.tab.board.clone() else { return };
        self.tab.catalog_board = board.uri.clone();
        self.tab.catalog_site = self.tab.site;
        self.tab.catalog_of = Some(board.clone());
        self.apply_board_sort();
        // Opening it (nothing shown yet): its last copy shows while it loads.
        let pages = self.pages.clone();
        let read = pages.clone().filter(|_| self.tab.catalog.is_empty());
        let (site, now) = (self.current_site().cfg.name.clone(), self.clock.now());
        let job = move |b: &dyn Backend, id, tx: &Sender<Msg>| {
            let kept = read.and_then(|p| p.read(&site, &board.uri, None));
            if let Some((copies, fetched)) = &kept {
                http::seed(copies);
                if let Ok(posts) = http::from_copies(copies, || b.catalog(&board.uri, &|_| {}))
                    && !posts.is_empty()
                {
                    let _ = tx.send(Msg::CachedCatalog(id, posts, *fetched));
                }
            }
            let (res, copies) = http::recording(|| {
                b.catalog(&board.uri, &|so_far| {
                    let _ = tx.send(Msg::CatalogPartial(id, so_far.to_vec()));
                })
            });
            if let (Ok(posts), Some(p)) = (&res, &pages)
                && !posts.is_empty()
            {
                p.write(&site, &board.uri, None, &copies, now);
            }
            res
        };
        self.spawn(format!("Loading /{}/", self.tab.catalog_board), job, Msg::Catalog);
    }

    fn load_thread(&mut self, no: u64) {
        let Some(board) = self.tab.board.clone() else { return };
        // Opening it (not reloading what's shown): its last copy shows while it loads.
        let opening = self.tab.thread.as_ref().is_none_or(|t| t.no != no || t.board != board.uri);
        self.tab.pending_thread = no;
        self.tab.archive_offer = None;
        self.tab.saved_offer = None;
        self.tab.offline = None;
        if opening {
            self.tab.cached = None;
        }
        self.tab.thread_checked = self.clock.instant();
        let key = self.key(&board.uri, no);
        // A watched thread's saved copy is its copy (kept once, not in the page cache too).
        let saved = self.store.saved(&key).map(|m| m.saved);
        let watched = self.store.watched(&key).is_some();
        if opening
            && watched
            && let Some(at) = saved
            && let Ok(copy) = self.store.load_saved(&key)
        {
            let posts: Vec<Post> = copy.posts.into_iter().map(Post::from).collect();
            if !posts.is_empty() {
                self.set_cached_thread(posts, at);
            }
        }
        let pages = self.pages.clone().filter(|_| !watched);
        let read = pages.clone().filter(|_| opening && self.tab.thread.is_none());
        let (site, now) = (key.site.clone(), self.clock.now());
        let job = move |b: &dyn Backend, id, tx: &Sender<Msg>| {
            let kept = read.and_then(|p| p.read(&site, &board.uri, Some(no)));
            if let Some((copies, fetched)) = &kept {
                http::seed(copies);
                if let Ok(posts) = http::from_copies(copies, || b.thread(&board.uri, no))
                    && !posts.is_empty()
                {
                    let _ = tx.send(Msg::CachedThread(id, posts, *fetched));
                }
            }
            let (res, copies) = http::recording(|| b.thread(&board.uri, no));
            if let (Ok(posts), Some(p)) = (&res, &pages)
                && !posts.is_empty()
            {
                p.write(&site, &board.uri, Some(no), &copies, now);
            }
            res
        };
        self.spawn(format!("Loading thread {no}"), job, Msg::Thread);
    }

    /// Show catalog threads, keeping the selected thread selected (by number).
    fn show_catalog(&mut self, posts: Vec<Post>) {
        let selected = self.selected_index().filter(|_| self.tab.view == View::Catalog).and_then(|i| self.tab.catalog.get(i)).map(|p| p.no);
        self.tab.catalog = posts;
        self.remark_catalog();
        if let Some(i) = selected.and_then(|no| self.visible_catalog().iter().position(|&k| self.tab.catalog[k].no == no)) {
            self.tab.catalog_list.state.select(Some(i));
        }
        let len = self.visible_catalog().len();
        self.tab.catalog_list.clamp(len);
    }

    fn key(&self, board: &str, no: u64) -> ThreadKey {
        ThreadKey { site: self.current_site().cfg.name.clone(), board: board.to_string(), no }
    }

    /// Save soon: background changes (refreshes, visits) are written together, every few
    /// seconds and on quit.
    fn save(&mut self) {
        self.save_pending = true;
    }

    /// Finish the background writes (on quit), within a few seconds.
    pub fn flush_writes(&mut self) {
        for e in self.store.flush(Duration::from_secs(5)) {
            self.error(e);
        }
    }

    /// Save now (what the user just did).
    pub fn save_now(&mut self) {
        self.save_pending = false;
        self.saved_at = self.clock.instant();
        if let Err(e) = self.store.save() {
            self.error(format!("Couldn't save watched threads: {e:#}"));
        }
    }

    /// Show a thread's posts as fetched (replacing a copy shown meanwhile).
    fn set_thread(&mut self, posts: Vec<Post>) {
        self.show_thread(posts, true);
    }

    /// Show the last copy kept of a thread being fetched, fetched at `at`. It's not a visit.
    fn set_cached_thread(&mut self, posts: Vec<Post>, at: i64) {
        self.tab.cached = None;
        self.show_thread(posts, false);
        if self.tab.thread.is_some() {
            self.tab.cached = Some(tabs::Offline { saved: at, dead: false });
        }
    }

    fn show_thread(&mut self, posts: Vec<Post>, live: bool) {
        // Fetched posts replacing the copy shown while they loaded: a first open, as far as
        // visits go (the copy wasn't one).
        let replacing = live && self.tab.cached.take().is_some();
        let Some(board) = self.tab.board.as_ref().map(|b| b.uri.clone()) else { return };
        if posts.is_empty() {
            self.error("The site sent the thread without any posts");
            return;
        }
        let no = posts.first().map(|p| p.no).unwrap_or(0);
        let key = self.key(&board, no);
        let mut tv = ThreadView::new(board, no, posts);
        tv.margin = self.scroll_margin;
        // On a refresh, the newest post already shown.
        let mut shown_max = None;
        // Reading the end: the last entry shown, to go on from once the posts are marked.
        let mut follow = None;
        match self.tab.thread.take().filter(|t| t.no == no && t.board == tv.board) {
            // On refresh, keep the selected post and what's at the top of the view.
            Some(old) => {
                shown_max = old.posts.iter().map(|p| p.no).max().filter(|_| !replacing);
                // Reading the end: posts this brings come into view.
                if self.follow_new_posts && !replacing && old.at_end() {
                    follow = old.entries.last().map(|e| e.path.clone());
                }
                tv.selected = old.current().and_then(|p| tv.index.get(&p.no)).copied().unwrap_or(0);
                // Expanded replies, the selected entry and the one at the top stay put.
                tv.expanded = old.expanded.clone();
                // A conversation stays, with any new replies that belong in it.
                tv.conversation = old.conversation.clone();
                let cursor_path = old.entries.get(old.entry()).map(|e| e.path.clone());
                tv.rebuild_entries();
                if let Some(e) = cursor_path.and_then(|p| tv.entries.iter().position(|e| e.path == p)) {
                    tv.set_cursor(e);
                }
                let at = |p: &Vec<u64>| tv.entries.iter().position(|e| e.path == *p);
                tv.anchor = old.top_anchor().and_then(|(i, off)| Some((at(&old.entries.get(i)?.path)?, off)));
                tv.scroll = old.scroll;
                tv.viewport = old.viewport;
                tv.cache = old.cache;
                tv.cache_width = old.cache_width;
                tv.estimates = old.estimates;
                tv.jumps = old.jumps;
                tv.new_after = old.new_after;
                // Revealed spoilers by post number, since indices can shift.
                tv.revealed = old.revealed.iter().filter_map(|&i| tv.index.get(&old.posts[i].no).copied()).collect();
                tv.reveal_all = old.reveal_all;
                tv.set_search(old.search);
                // The focused part, if the post still has it.
                tv.focus = old.focus.filter(|f| tv.parts_of(tv.entry()).contains(f));
                // On a first visit nothing was new; what this brings is.
                if tv.new_after == 0
                    && let Some(m) = shown_max.filter(|&m| tv.posts.iter().any(|p| p.no > m))
                {
                    tv.new_after = m;
                }
            }
            None => {
                tv.new_after = self.store.last_seen(&key);
                if let Some(i) = self.tab.pending_post.take().and_then(|no| tv.index.get(&no).copied()) {
                    tv.selected = i;
                }
                // A conversation that was open last time.
                if let Some(&i) = self.tab.pending_conversation.take().and_then(|no| tv.index.get(&no)) {
                    let selected = tv.selected;
                    tv.selected = i;
                    if tv.enter_conversation().is_ok() && tv.in_view(selected) {
                        tv.select(selected);
                    }
                }
            }
        }
        // A saved or cached copy isn't a visit, and isn't saved again.
        if live && self.tab.offline.is_none() {
            self.fetched_thread(&key, &tv.posts, shown_max);
        }
        self.tab.thread = Some(tv);
        self.remark_thread();
        // Following: the first new entry that isn't hidden, revealed as `j` would.
        if let Some(last) = follow
            && let Some(t) = &mut self.tab.thread
            && let Some(at) = t.entries.iter().position(|e| e.path == last)
            && let Some(next) = (at + 1..t.entries.len()).find(|&e| !t.is_collapsed(t.entries[e].post))
        {
            t.set_cursor(next);
            t.reveal = Some(Reveal::Step);
        }
    }

    /// A thread's posts arrived: note the visit, and keep a copy if it's watched.
    fn fetched_thread(&mut self, key: &ThreadKey, posts: &[Post], shown_max: Option<u64>) {
        // Just fetched: a watched thread's next background refresh counts from now.
        self.watched_checked.insert(key.clone(), self.clock.instant());
        let max_no = posts.iter().map(|p| p.no).max().unwrap_or(0);
        let subject = thread_subject(posts);
        // A visit is an open, or a refresh that brought new posts.
        if shown_max.is_none_or(|m| max_no > m) {
            self.store.visit(key, &subject, posts.len(), max_no, self.clock.now());
        }
        self.store.opened(&key.site, &key.board, key.no, posts.len().saturating_sub(1) as u32, self.clock.now());
        if let Some(w) = self.store.watched_mut(key) {
            generals::note_limit(w, posts);
        }
        self.keep_copy(key, posts, false);
        self.save();
    }

    // ----- filters and hiding -----

    /// What filters and hiding by hand say about posts (on the board `board_of` gives each).
    fn marks(&self, posts: &[Post], board_of: impl Fn(&Post) -> String) -> Vec<Mark> {
        let site = &self.current_site().cfg.name;
        let mut hidden: HashMap<String, HashSet<u64>> = HashMap::new();
        let mark = |p: &Post| {
            let board = board_of(p);
            let mut m = self.filters.check(site, &board, p);
            let by_hand = hidden.entry(board).or_insert_with_key(|b| self.store.hidden_on(site, b));
            if m.hidden.is_none() && by_hand.contains(&p.no) {
                m.hidden = Some(String::new());
            }
            m
        };
        posts.iter().map(mark).collect()
    }

    /// Note which catalog threads are new since the last visit (and remember them all).
    fn catalog_seen(&mut self) {
        let nos: Vec<u64> = self.tab.catalog.iter().map(|p| p.no).collect();
        let site = self.current_site().cfg.name.clone();
        let board = self.tab.catalog_board.clone();
        self.tab.catalog_new = self.store.catalog_seen(&site, &board, &nos, self.clock.now());
        self.store.board_opened(&site, &board);
        self.note_titles(self.tab.site);
        self.save();
    }

    /// Replies a catalog thread has gained since it was last opened.
    pub fn new_replies(&self, p: &Post) -> Option<u32> {
        let seen = self.store.replies_seen(&self.current_site().cfg.name, &self.board_of(p), p.no)?;
        p.replies.filter(|&r| r > seen).map(|r| r - seen)
    }

    pub fn remark_catalog(&mut self) {
        let marks = self.marks(&self.tab.catalog, |p| self.board_of(p));
        self.tab.catalog_marks = marks;
    }

    /// The board a catalog thread is on (overboards mix boards).
    pub fn board_of(&self, p: &Post) -> String {
        let loaded = Some(self.tab.catalog_board.clone()).filter(|b| !b.is_empty());
        p.board.clone().or(loaded).or_else(|| self.tab.board.as_ref().map(|b| b.uri.clone())).unwrap_or_default()
    }

    pub fn remark_thread(&mut self) {
        let Some(t) = &self.tab.thread else { return };
        let marks = self.marks(&t.posts, |_| t.board.clone());
        let mine = self.store.watched(&self.key(&t.board, t.no)).map(|w| w.mine.iter().copied().collect()).unwrap_or_default();
        let show = self.show_hidden;
        if let Some(t) = &mut self.tab.thread {
            t.marks = marks;
            t.mine = mine;
            t.show_hidden = show;
            t.layout = None;
            // A post just collapsed (hidden) has no parts to focus.
            if t.focus.as_ref().is_some_and(|f| !t.parts_of(t.entry()).contains(f)) {
                t.focus = None;
            }
        }
    }

    /// `H`: hide or unhide the selected thread (catalog) or post (thread) by hand.
    fn toggle_hidden(&mut self) {
        let (board, no, what, mark) = match self.tab.view {
            View::Catalog => {
                let Some(i) = self.selected_index() else { return };
                let p = &self.tab.catalog[i];
                (self.board_of(p), p.no, "thread", self.tab.catalog_marks.get(i).cloned())
            }
            View::Thread => {
                let Some(t) = &self.tab.thread else { return };
                let what = if t.selected == 0 { "thread" } else { "post" };
                (t.board.clone(), t.current().map_or(t.no, |p| p.no), what, t.marks.get(t.selected).cloned())
            }
            _ => return,
        };
        if let Some(label) = mark.and_then(|m| m.hidden).filter(|l| !l.is_empty()) {
            self.info(format!("Hidden by the filter \"{label}\"; Settings › Filters changes it"));
            return;
        }
        let site = self.current_site().cfg.name.clone();
        let hidden = self.store.toggle_hidden(&site, &board, no);
        self.save_now();
        let show = self.keys.key(Action::ShowHidden);
        self.info(if hidden { format!("Hid {what} {no} ({show} shows hidden ones)") } else { format!("Unhid {what} {no}") });
        self.remark_catalog();
        self.remark_thread();
        self.clamp_list();
    }

    /// `Z`: show hidden threads and posts (dimmed), or leave them out again.
    fn toggle_show_hidden(&mut self) {
        // Keep the same thread selected in the catalog.
        let keep = self.selected_index().filter(|_| self.tab.view == View::Catalog);
        self.show_hidden = !self.show_hidden;
        self.remark_thread();
        if let Some(i) = keep
            && let Some(pos) = self.visible_catalog().iter().position(|&v| v == i)
        {
            self.tab.catalog_list.state.select(Some(pos));
        }
        self.clamp_list();
        self.info(if self.show_hidden { "Showing hidden threads and posts" } else { "Leaving out hidden threads and posts" });
    }

    // ----- watched threads and auto-refresh -----

    /// Start background refreshes that are due: the open thread every `refresh_thread`, and
    /// each watched thread every `refresh_watched`, a couple at a time.
    fn background(&mut self) {
        self.know_nsfw(self.tab.site);
        // A saved copy open isn't refreshed (a watched thread still is, below, unless it's dead).
        let open = self.tab.thread.as_ref().filter(|_| self.tab.view == View::Thread && self.tab.offline.is_none()).map(|t| self.key(&t.board, t.no));
        let now = self.clock.instant();
        // Fetched for any tab (or as a watched thread) counts too.
        let fetched_since = |key: &ThreadKey, every: Duration| {
            self.watched_checked.get(key).is_none_or(|t| now.saturating_duration_since(*t) >= every)
        };
        if let Some(key) = &open
            && self.tab.loading.is_none()
            && now.saturating_duration_since(self.tab.thread_checked) >= self.refresh_thread
            && fetched_since(key, self.refresh_thread)
            && !self.refreshing.contains(key)
        {
            self.tab.thread_checked = self.clock.instant();
            // A watched thread refreshed as the open one needn't be again when it's left.
            self.watched_checked.insert(key.clone(), self.tab.thread_checked);
            self.refresh_in_background(key.clone());
        }
        if self.refreshing.len() >= MAX_REFRESHING {
            return;
        }
        let due = self.store.watched.iter().find(|w| {
            !w.dead
                && Some(&w.key) != open.as_ref()
                && !self.refreshing.contains(&w.key)
                && self.watched_checked.get(&w.key).is_none_or(|t| now.saturating_duration_since(*t) >= self.refresh_watched)
        });
        if let Some(key) = due.map(|w| w.key.clone()) {
            self.watched_checked.insert(key.clone(), self.clock.instant());
            self.refresh_in_background(key);
        }
        self.check_generals(self.clock.instant());
    }

    fn refresh_in_background(&mut self, key: ThreadKey) {
        let Some(site) = self.sites.iter().find(|s| s.cfg.name == key.site) else { return };
        let backend = site.backend.clone();
        let tx = self.tx.clone();
        self.refreshing.insert(key.clone());
        // The open thread's copy is kept up to date too (a watched one's is its saved copy).
        let pages = self.pages.clone().filter(|_| self.store.watched(&key).is_none());
        let now = self.clock.now();
        std::thread::spawn(move || {
            let (res, copies) = http::recording(|| http::background(|| backend.thread(&key.board, key.no)));
            if let (Ok(posts), Some(p)) = (&res, &pages)
                && !posts.is_empty()
            {
                p.write(&key.site, &key.board, Some(key.no), &copies, now);
            }
            let _ = tx.send(Msg::Refreshed(key, res));
        });
    }

    fn refreshed(&mut self, key: ThreadKey, res: Result<Vec<Post>>) {
        self.refreshing.remove(&key);
        let is_open = self.tab.view == View::Thread
            && self.tab.offline.is_none()
            && self.tab.thread.as_ref().is_some_and(|t| self.key(&t.board, t.no) == key)
            && self.current_site().cfg.name == key.site;
        if is_open {
            // Count the interval from the response, so the next refresh is past the HTTP cache window.
            self.tab.thread_checked = self.clock.instant();
        }
        match res {
            Ok(posts) if is_open => self.set_thread(posts),
            Ok(posts) => {
                let subject = thread_subject(&posts);
                let prev = self.notified_max.get(&key).copied();
                let Some(w) = self.store.watched_mut(&key) else { return };
                let max_no = posts.iter().map(|p| p.no).max().unwrap_or(0);
                if w.last_seen == 0 {
                    w.last_seen = max_no;
                }
                let mine = w.mine.clone();
                let to_you = |p: &Post| p.quotes.iter().any(|q| mine.contains(q));
                let unread: Vec<&Post> = posts.iter().filter(|p| p.no > w.last_seen).collect();
                w.unread = unread.len();
                w.replies = unread.iter().filter(|p| to_you(p)).count();
                // Tell about posts newer than this session's last refresh (not on the first
                // one, which may find posts from long ago).
                let fresh: Vec<&&Post> = unread.iter().filter(|p| prev.is_some_and(|m| p.no > m)).collect();
                let note = Note {
                    key: key.clone(),
                    subject: if w.subject.is_empty() { subject.clone() } else { w.subject.clone() },
                    new: fresh.len(),
                    replies: fresh.iter().filter(|p| to_you(p)).count(),
                };
                w.posts = posts.len();
                w.dead = false;
                generals::note_limit(w, &posts);
                if w.subject.is_empty() {
                    w.subject = subject;
                }
                self.keep_copy(&key, &posts, false);
                self.notified_max.insert(key, max_no);
                if note.new > 0 {
                    self.notes_since.get_or_insert_with(Instant::now);
                    self.notes.push(note);
                }
                self.save();
            }
            Err(e) if http::is_not_found(&e) => {
                if let Some(w) = self.store.watched_mut(&key) {
                    w.dead = true;
                    self.save();
                }
                self.store.saved_dead(&key);
                if is_open {
                    self.thread_gone(&key);
                }
            }
            // Other failures (network, rate limits) just wait for the next round.
            Err(e) if is_open => self.error(e),
            Err(_) => {}
        }
    }

    // ----- downloads and settings -----

    /// Save the selected post's files, or the whole thread's.
    fn download(&mut self, whole_thread: bool) {
        let Some(t) = &self.tab.thread else { return };
        let posts: Vec<&Post> = if whole_thread { t.posts.iter().collect() } else { t.current().into_iter().collect() };
        let dir = download::dir(self.download_dir.as_deref(), &self.current_site().cfg.name, &t.board, t.no);
        let jobs = download::jobs(&posts, &dir);
        self.start_download(jobs, dir, if whole_thread { "Thread has no files" } else { "Post has no file" });
    }

    /// Save the thread as thread.html and thread.json in its download folder.
    fn export_thread(&mut self) {
        let (Some(t), Some(b)) = (&self.tab.thread, &self.tab.board) else { return };
        let site = self.current_site();
        let dir = download::dir(self.download_dir.as_deref(), &site.cfg.name, &t.board, t.no);
        let url = site.backend.thread_url(&b.uri, t.no);
        let about = crate::export::About { site: &site.cfg.name, board: &t.board, thread: t.no, url: &url, saved: self.clock.now() };
        let key = self.key(&t.board, t.no);
        let posts = t.posts.clone();
        match crate::export::save(&posts, &about, &theme::theme(), &dir) {
            Ok(()) => {
                // Also kept as a saved copy, to read in ck (the Saved view).
                self.keep_copy(&key, &posts, true);
                self.save_now();
                self.info(format!("Saved thread.html and thread.json in {} (and in Saved)", tilde(&dir.display().to_string())));
            }
            Err(e) => self.error(format!("Couldn't save the thread: {e:#}")),
        }
    }

    /// Fetch `(url, path)` jobs into `dir` in the background.
    fn start_download(&mut self, jobs: Vec<(String, std::path::PathBuf)>, dir: std::path::PathBuf, none: &str) {
        if jobs.is_empty() {
            self.info(none);
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.error(format!("Couldn't create {}: {e}", dir.display()));
            return;
        }
        let d = &mut self.downloads;
        if d.running == 0 {
            *d = Downloads::default();
        }
        d.total += jobs.len();
        d.running += 1;
        d.dir = Some(dir);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            for (url, path) in jobs {
                let ev = if path.exists() {
                    DlEvent::Skipped
                } else {
                    match http::download_to(&url, &path) {
                        Ok(()) => DlEvent::Done,
                        Err(e) => DlEvent::Failed(format!("{e:#}")),
                    }
                };
                if tx.send(Msg::Download(ev)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::Download(DlEvent::Finished));
        });
    }

    fn download_event(&mut self, ev: DlEvent) {
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
                        msg.push_str(&format!(", {} already there", d.skipped));
                    }
                    if d.failed > 0 {
                        msg.push_str(&format!(", {} failed ({})", d.failed, d.last_error.as_deref().unwrap_or("")));
                    }
                    self.status = Some(Status { text: msg, error: d.failed > 0 });
                }
            }
        }
    }

    /// The open catalog's board, as `site/board` (for its own sort and layout).
    fn board_key(&self) -> String {
        let board = Some(self.tab.catalog_board.clone()).filter(|b| !b.is_empty()).or_else(|| self.tab.board.as_ref().map(|b| b.uri.clone()));
        format!("{}/{}", self.current_site().cfg.name, board.unwrap_or_default())
    }

    /// The catalog layout here: the board's own, or the default.
    pub fn layout(&self) -> CatalogLayout {
        self.store.board_prefs.get(&self.board_key()).and_then(|p| p.layout).unwrap_or(self.default_layout)
    }

    /// `c` in a catalog: cycle this board's layout (cards, compact, grid), remembered for it.
    fn cycle_layout(&mut self) {
        let layout = self.layout().next();
        let key = self.board_key();
        self.store.board_prefs.entry(key).or_default().layout = Some(layout);
        self.save_now();
        let board = self.tab.board.as_ref().map_or(String::new(), |b| format!(" for /{}/", b.uri));
        self.info(format!("Layout{board}: {} (the default is in Settings)", layout.as_str()));
    }

    /// The board's own sort, when its catalog opens.
    fn apply_board_sort(&mut self) {
        self.tab.catalog_sort = self.store.board_prefs.get(&self.board_key()).and_then(|p| p.sort).unwrap_or_default();
    }

    /// The default catalog layout (Settings): saved in config.toml (keeping its comments), or
    /// in the data directory if the config can't be edited.
    pub fn cycle_default_layout(&mut self) {
        self.default_layout = self.default_layout.next();
        let name = self.default_layout.as_str();
        match self.edit_config(|d| {
            d["catalog_layout"] = toml_edit::value(name);
            d.remove("compact_catalog");
        }) {
            Ok(path) => {
                self.store.settings.compact_catalog = None;
                self.store.settings.catalog_layout = None;
                self.info(format!("Default catalog layout: {name} (saved in {path})"));
            }
            Err(e) => {
                self.store.settings.compact_catalog = None;
                self.store.settings.catalog_layout = Some(self.default_layout);
                self.save_now();
                self.info(format!("Default catalog layout: {name} (kept in the data directory: {e:#})"));
            }
        }
    }

    /// A thread 404'd: say so, and offer the site's archive if it has one.
    ///
    /// With a saved copy, that comes first: a thread still on screen becomes its saved copy,
    /// and otherwise `enter` opens it.
    pub(crate) fn thread_gone(&mut self, key: &ThreadKey) {
        let archive = self.sites.iter().find(|s| s.cfg.name == key.site).and_then(|s| s.cfg.archive.clone());
        self.tab.archive_offer = archive.filter(|a| self.sites.iter().any(|s| s.cfg.name == *a)).map(|a| ThreadKey { site: a, board: key.board.clone(), no: key.no });
        let in_archive = self.tab.archive_offer.as_ref().map(|a| format!("{} opens it in {}", self.keys.key(Action::Archive), a.site));
        let saved = self.store.saved(key).map(|m| m.saved);
        let shown = self.tab.thread.as_ref().is_some_and(|t| t.no == key.no && t.board == key.board);
        let text = match saved {
            Some(at) if shown => {
                self.tab.cached = None;
                self.tab.offline = Some(tabs::Offline { saved: at, dead: true });
                format!("Thread was deleted or archived: this is its saved copy{}", in_archive.map(|a| format!(" ({a})")).unwrap_or_default())
            }
            Some(at) => {
                self.tab.saved_offer = Some(key.clone());
                let ago = crate::ui::ago(at, self.clock);
                format!("Thread was deleted or archived. A saved copy from {ago}: enter opens it{}", in_archive.map(|a| format!(", {a}")).unwrap_or_default())
            }
            None => {
                // A copy shown while it loaded stays, marked as gone.
                if let Some(c) = self.tab.cached.as_mut().filter(|_| shown) {
                    c.dead = true;
                }
                format!("Thread was deleted or archived{}", in_archive.map(|a| format!(". Press {a}")).unwrap_or_default())
            }
        };
        self.error(text);
    }

    fn toggle_watch(&mut self) {
        let (board, no, subject, posts, last_seen) = match (self.tab.view, &self.tab.board) {
            (View::Thread, _) => {
                let Some(t) = &self.tab.thread else { return };
                let max_no = t.posts.iter().map(|p| p.no).max().unwrap_or(0);
                (t.board.clone(), t.no, thread_subject(&t.posts), t.posts.len(), max_no)
            }
            (View::Catalog, Some(b)) => {
                let Some(i) = self.selected_index() else { return };
                let op = &self.tab.catalog[i];
                let posts = op.replies.map_or(1, |r| r as usize + 1);
                let board = op.board.clone().unwrap_or_else(|| b.uri.clone());
                // Unknown until the first refresh, which then counts nothing as unread.
                (board, op.no, thread_subject(std::slice::from_ref(op)), posts, 0)
            }
            _ => return,
        };
        let key = self.key(&board, no);
        let watching = self.store.toggle_watch(key.clone(), subject, posts, last_seen);
        if watching {
            self.keep_open_thread(&key);
        }
        // Refreshed soon, to learn where it's at, unless it was just fetched.
        let now = self.clock.instant();
        if self.watched_checked.get(&key).is_none_or(|t| now.saturating_duration_since(*t) >= http::MIN_REFETCH) {
            self.watched_checked.remove(&key);
        }
        self.info(if watching { format!("Watching thread {no}") } else { format!("Stopped watching thread {no}") });
        self.save_now();
    }

    /// `c`: the selected post's conversation alone, or the whole thread again.
    fn toggle_conversation(&mut self) {
        let Some(t) = self.tab.thread.as_mut().filter(|_| self.tab.view == View::Thread && self.tab.gallery.is_none()) else { return };
        if t.conversation.is_some() {
            t.leave_conversation();
            return;
        }
        match t.enter_conversation() {
            Ok(n) => {
                let capped = t.conversation.as_ref().is_some_and(|c| c.capped);
                let esc = if capped { format!(" (the nearest {n}; there are more)") } else { String::new() };
                self.info(format!("{}{esc}; esc or {} shows the whole thread", plural_posts(n), self.keys.key(Action::Conversation)));
            }
            Err(e) => self.info(e),
        }
    }

    /// Keep a copy of a thread's posts in the data directory (a watched thread's, or with
    /// `always`, any). Unchanged posts aren't written again.
    fn keep_copy(&mut self, key: &ThreadKey, posts: &[Post], always: bool) {
        if !always && self.store.watched(key).is_none() {
            return;
        }
        let url = self.thread_link(key, None).unwrap_or_default();
        if self.store.keep_thread(key, &thread_subject(posts), &url, posts, self.clock.now()) {
            self.save();
        }
    }

    /// A thread just watched: if it's the one open (and loaded), it's saved at once. A saved
    /// copy open is kept as it is (a copy saved under another number, when the site answered
    /// with another thread, is kept under this one too).
    fn keep_open_thread(&mut self, key: &ThreadKey) {
        if self.tab.view != View::Thread || (self.tab.offline.is_some() && self.store.saved(key).is_some()) {
            return;
        }
        let Some(t) = self.tab.thread.as_ref().filter(|t| self.key(&t.board, t.no) == *key) else { return };
        let posts = t.posts.clone();
        self.keep_copy(key, &posts, false);
        if self.tab.offline.is_some_and(|o| o.dead) {
            self.store.saved_dead(key);
        }
    }

    /// `m`: mark the selected post as yours (or not), to hear about replies to it. The
    /// thread is watched if it isn't.
    fn toggle_mine(&mut self) {
        let Some(t) = &self.tab.thread else { return };
        let key = self.key(&t.board, t.no);
        let Some(no) = t.current().map(|p| p.no) else { return };
        if self.store.watched(&key).is_none() {
            let max_no = t.posts.iter().map(|p| p.no).max().unwrap_or(0);
            self.store.toggle_watch(key.clone(), thread_subject(&t.posts), t.posts.len(), max_no);
            self.keep_open_thread(&key);
        }
        let Some(w) = self.store.watched_mut(&key) else { return };
        let mine = !w.mine.contains(&no);
        if mine {
            w.mine.push(no);
        } else {
            w.mine.retain(|&n| n != no);
        }
        self.info(if mine { format!("Marked No.{no} as yours; replies to it will be counted and notified") } else { format!("No.{no} isn't marked as yours any more") });
        self.save_now();
        self.remark_thread();
    }

    /// Open a thread from Watched or History, switching site and board as needed.
    fn open_key(&mut self, key: ThreadKey) {
        let Some(site) = self.sites.iter().position(|s| s.cfg.name == key.site) else {
            self.error(format!("No site named {} in the config", key.site));
            return;
        };
        self.switch_site(site);
        let board = self.boards().iter().find(|b| b.uri == key.board).cloned();
        self.tab.board = Some(board.unwrap_or(Board { uri: key.board, title: String::new(), nsfw: None }));
        self.tab.return_to = Some(self.tab.view);
        self.tab.from_catalog = false;
        self.tab.gallery = None;
        self.tab.thread = None;
        self.tab.view = View::Thread;
        self.load_thread(key.no);
    }

    /// Make `site` the current one, with its board list if it's saved (else fetched in the
    /// background), so going back to Boards shows it.
    fn switch_site(&mut self, site: usize) {
        if site == self.tab.site {
            return;
        }
        self.tab.board_list = Picker::top();
        self.tab.site = site;
        if self.sites[site].boards.is_none() {
            match self.known_boards(site) {
                Some(boards) => self.sites[site].boards = Some(boards),
                None => self.refresh_boards_in_background(site),
            }
        }
    }

    /// A site's boards as far as they're known without a request: loaded, from the config,
    /// or saved last time.
    fn known_boards(&self, site: usize) -> Option<Vec<Board>> {
        let s = self.sites.get(site)?;
        s.boards
            .clone()
            .or_else(|| s.cfg.boards.as_ref().map(|b| b.iter().map(backend::to_board).collect()))
            .or_else(|| self.store.load_boards(&s.cfg.name).map(|(b, _)| b))
    }

    /// Follow the selected post's first link that leads out of this thread, preferring links
    /// to posts over links to boards.
    fn follow_link(&mut self) {
        match self.outgoing_link() {
            Some(link) => self.follow(link),
            None => self.info("Post quotes nothing in this thread"),
        }
    }

    /// The selected post's first link out of this thread (to a post rather than a board, if
    /// it has both).
    fn outgoing_link(&self) -> Option<Link> {
        let (Some(t), Some(board)) = (&self.tab.thread, &self.tab.board) else { return None };
        let here = |l: &Link| l.board.as_ref().is_none_or(|b| *b == board.uri);
        let leaves = |l: &&Link| {
            let in_thread = here(l)
                && (l.thread == Some(t.no) || (l.thread.is_none() && l.post.is_some_and(|p| t.index.contains_key(&p))));
            !in_thread
        };
        let links = &t.current()?.links;
        links.iter().filter(leaves).find(|l| l.post.is_some()).or_else(|| links.iter().find(leaves)).cloned()
    }

    /// Go where a quote link leads: a thread (remembered for `u`), a board, or a post whose
    /// thread the engine is asked for.
    fn follow(&mut self, link: Link) {
        let Some(board) = self.tab.board.clone() else { return };
        let target = match &link.board {
            Some(uri) if *uri != board.uri => self.find_board(uri),
            _ => board.clone(),
        };
        match (link.thread, link.post) {
            (Some(no), post) => {
                if let Some(t) = self.tab.thread.as_ref().filter(|_| self.tab.view == View::Thread) {
                    self.tab.trail.push((self.tab.site, board, t.no, t.current().map_or(t.no, |p| p.no)));
                }
                self.open_thread_at(target, no, post);
            }
            (None, None) => {
                // A board link: open its catalog.
                self.tab.return_to = None;
                self.open_catalog(target);
            }
            (None, Some(post)) => {
                // Ask the engine which thread the post is in (only some can).
                let uri = target.uri.clone();
                let label = format!("Looking up post {post}");
                self.spawn(label, move |b, _, _| b.find_thread(&uri, post), move |id, r| Msg::Found(id, target, post, r));
            }
        }
    }

    /// A board by URI from the site's board list, or a bare one if the list isn't loaded.
    fn find_board(&self, uri: &str) -> Board {
        let known = self.boards().iter().find(|b| b.uri == uri).cloned();
        known.unwrap_or(Board { uri: uri.to_string(), title: String::new(), nsfw: None })
    }

    /// Open a thread on the current site, selecting `post` when it arrives.
    fn open_thread_at(&mut self, board: Board, no: u64, post: Option<u64>) {
        self.tab.from_catalog = false;
        self.tab.gallery = None;
        self.tab.board = Some(board);
        self.tab.pending_post = post;
        self.tab.thread = None;
        self.tab.view = View::Thread;
        self.load_thread(no);
    }

    /// The post whose files `v`, `i`, `d` act on: the selected catalog entry or thread post.
    fn selected_post(&self) -> Option<&Post> {
        match self.tab.view {
            View::Catalog => self.selected_index().map(|i| &self.tab.catalog[i]),
            View::Thread => self.tab.thread.as_ref().and_then(ThreadView::current),
            _ => None,
        }
    }

    fn open_viewer(&mut self) {
        if !self.images.enabled() {
            self.info("Images are off (images = \"off\" in the config); i opens the file");
            return;
        }
        if self.images_off_here() {
            return;
        }
        let link = self.selected_link();
        match self.selected_post().map(|p| p.files.clone()) {
            Some(files) if !files.is_empty() => {
                if !self.thread_viewer(0) {
                    self.tab.viewer = Some(Viewer::new(files, 0, link));
                }
            }
            _ => self.info("Post has no file"),
        }
    }

    /// Open a file externally: videos in mpv when it's installed, everything else in the default opener.
    pub fn open_file(&mut self, f: &Attachment) {
        if f.is_video() && on_path("mpv") && !crate::sandboxed() {
            let mut cmd = std::process::Command::new("mpv");
            cmd.arg(&f.url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
            match cmd.spawn() {
                Ok(_) => self.info(format!("Playing {} in mpv", f.filename)),
                Err(e) => self.error(format!("Couldn't start mpv: {e}")),
            }
        } else {
            self.open_url(&f.url);
        }
    }

    fn remove_entry(&mut self) {
        if self.tab.view == View::Sites {
            match self.selected_site_row() {
                Some(SiteRow::Favorite(i)) => {
                    let f = self.favorites.remove(i);
                    self.save_favorites(&format!("/{}/ off the favorites", f.board));
                }
                Some(SiteRow::Recent(i)) => {
                    self.store.recent_boards.remove(i);
                    self.save_now();
                    self.clamp_home();
                }
                Some(SiteRow::Site(i)) => self.toggle_site_hidden(i),
                _ => {}
            }
            return;
        }
        let Some(i) = self.selected_index() else { return };
        match self.tab.view {
            View::Watched => {
                let w = self.store.watched.remove(i);
                self.info(format!("Stopped watching thread {}", w.key.no));
            }
            View::History => {
                self.store.history.remove(i);
            }
            View::Saved => {
                let key = self.store.saved[i].key.clone();
                // Asked first: a copy can't be fetched again once the thread is gone.
                if self.saved_confirm.take().as_ref() != Some(&key) {
                    let x = self.keys.key(Action::Remove);
                    self.info(format!("Press {x} again to remove the saved copy of thread {}", key.no));
                    self.saved_confirm = Some(key);
                    return;
                }
                self.store.forget_saved(&key);
                self.info(format!("Removed the saved copy of thread {}", key.no));
            }
            _ => return,
        }
        self.save_now();
        self.clamp_list();
    }

    /// Index of the selected item in the current list's underlying data (not for Sites).
    fn selected_index(&self) -> Option<usize> {
        match self.tab.view {
            View::Sites | View::Thread | View::Settings | View::Search => None,
            View::Boards => self.tab.board_list.state.selected().and_then(|i| self.visible_boards().get(i).copied()),
            View::Catalog => self.tab.catalog_list.state.selected().and_then(|i| self.visible_catalog().get(i).copied()),
            View::Watched => self.watched_list.state.selected().and_then(|i| self.visible_watched().get(i).copied()),
            View::History => self.history_list.state.selected().and_then(|i| self.visible_history().get(i).copied()),
            View::Saved => self.saved_list.state.selected().and_then(|i| self.visible_saved().get(i).copied()),
        }
    }

    fn enter(&mut self) {
        match (self.tab.view, self.selected_index()) {
            (View::Settings, _) => self.activate_setting(),
            (View::Search, _) => self.open_search_hit(),
            (View::Sites, _) => match self.selected_site_row() {
                Some(SiteRow::Watched) => self.tab.view = View::Watched,
                Some(SiteRow::History) => self.tab.view = View::History,
                Some(SiteRow::Saved) => self.tab.view = View::Saved,
                Some(SiteRow::Favorite(i)) => self.open_favorite(i),
                Some(SiteRow::Recent(i)) => {
                    if let Some(b) = self.recent_board(i) {
                        self.open_board(&b);
                    }
                }
                Some(SiteRow::Site(i)) => self.enter_site(i),
                Some(SiteRow::HiddenSites) => {
                    self.show_hidden_sites = !self.show_hidden_sites;
                    self.clamp_home();
                }
                None => {}
            },
            (View::Watched, Some(i)) => self.open_key(self.store.watched[i].key.clone()),
            (View::History, Some(i)) => self.open_key(self.store.history[i].key.clone()),
            (View::Saved, Some(i)) => self.open_saved(self.store.saved[i].key.clone()),
            (View::Boards, Some(i)) => self.open_catalog(self.boards()[i].clone()),
            (View::Catalog, Some(i)) => {
                let no = self.tab.catalog[i].no;
                // On an overboard the thread lives on its own board.
                if let Some(uri) = self.tab.catalog[i].board.clone().filter(|b| *b != self.tab.catalog_board) {
                    self.tab.board = Some(self.find_board(&uri));
                }
                self.tab.thread = None;
                self.tab.return_to = None;
                self.tab.from_catalog = true;
                self.tab.view = View::Thread;
                self.load_thread(no);
            }
            _ => {}
        }
    }

    /// Show `board`'s catalog from the top, and load it.
    fn open_catalog(&mut self, board: Board) {
        self.tab.board = Some(board);
        self.tab.catalog.clear();
        self.tab.catalog_cached = None;
        self.tab.catalog_list = Picker::top();
        self.tab.view = View::Catalog;
        self.load_catalog();
    }

    fn enter_site(&mut self, i: usize) {
        if i != self.tab.site {
            self.tab.board_list = Picker::default();
        }
        self.tab.site = i;
        self.tab.view = View::Boards;
        if self.tab.board_list.state.selected().is_none() {
            self.tab.board_list.state.select(Some(0));
        }
        if self.current_site().boards.is_some() {
            return;
        }
        // Board lists from the config need no request; fetched ones are saved, shown at
        // once next time, and refreshed in the background once a day.
        let cfg = &self.current_site().cfg;
        let saved = if cfg.boards.is_none() { self.store.load_boards(&cfg.name) } else { None };
        match saved {
            Some((boards, fetched)) => {
                self.sites[i].boards = Some(boards);
                if self.clock.now().saturating_sub(fetched) > BOARDS_MAX_AGE {
                    self.refresh_boards_in_background(i);
                }
            }
            None => self.load_boards(),
        }
    }

    fn back(&mut self) {
        self.tab.gallery = None;
        self.tab.view = match self.tab.view {
            View::Sites | View::Boards | View::Watched | View::History | View::Saved => View::Sites,
            View::Settings => self.tab.settings_back.take().unwrap_or(View::Sites),
            View::Search => self.close_search(),
            View::Catalog => View::Boards,
            View::Thread => self.tab.return_to.take().unwrap_or(View::Catalog),
        };
        self.tab.trail.clear();
        // Back to the catalog the thread was opened from (an overboard's, maybe).
        if self.tab.view == View::Catalog && std::mem::take(&mut self.tab.from_catalog) {
            self.tab.board = self.tab.catalog_of.clone();
        }
        // After following links to another board (or a board of the same name on another
        // site), the loaded catalog is for the old one.
        if self.tab.view == View::Catalog
            && let Some(board) = self.tab.board.clone().filter(|b| b.uri != self.tab.catalog_board || self.tab.site != self.tab.catalog_site)
        {
            self.open_catalog(board);
            return;
        }
        // Navigating away cancels any in-flight request (its response will be ignored).
        if self.tab.loading.is_some() {
            self.tab.req = 0;
            self.tab.loading = None;
        }
    }

    fn refresh(&mut self) {
        match self.tab.view {
            View::Sites | View::Watched | View::History | View::Saved | View::Settings => {}
            View::Search => {
                if let Some(s) = &mut self.tab.search {
                    s.hits.clear();
                    s.pages = 0;
                }
                self.load_search_page();
            }
            View::Boards => self.load_boards(),
            View::Catalog => self.load_catalog(),
            View::Thread if self.tab.offline.is_some() => self.refresh_saved(),
            View::Thread => {
                if let Some(no) = self.tab.thread.as_ref().map(|t| t.no) {
                    self.load_thread(no);
                }
            }
        }
    }

    /// Copy the selected thing's text (or file URL in the viewer), or with `link` its URL.
    fn copy(&mut self, link: bool) {
        let what = if let Some(v) = &self.tab.viewer {
            let post_link = v.link.clone().or_else(|| self.viewer_post_link()).or_else(|| self.gallery_link(v.index));
            if link { post_link.map(|l| ("link", l)) } else { Some(("file URL", v.files[v.index].url.clone())) }
        } else if link {
            self.selected_link().map(|l| ("link", l))
        } else {
            let saved = |key: &ThreadKey, subject: &str| {
                let url = self.thread_link(key, None).unwrap_or_default();
                format!("{subject}\n{url}").trim().to_string()
            };
            match self.tab.view {
                View::Thread => self.tab.thread.as_ref().and_then(ThreadView::current).map(|p| ("text", copy_text(p, false))),
                View::Catalog => self.selected_post().map(|p| ("text", copy_text(p, true))),
                View::Watched => self.selected_index().map(|i| ("text", saved(&self.store.watched[i].key, &self.store.watched[i].subject))),
                View::History => self.selected_index().map(|i| ("text", saved(&self.store.history[i].key, &self.store.history[i].subject))),
                View::Saved => self.selected_index().map(|i| ("text", saved(&self.store.saved[i].key, &self.store.saved[i].subject))),
                _ => None,
            }
        };
        let Some((what, text)) = what else { return };
        if text.is_empty() {
            self.info("Nothing to copy: the post has no text");
            return;
        }
        self.copy_text(what, text);
    }

    pub fn copy_text(&mut self, what: &str, text: String) {
        match crate::clipboard::copy(&text) {
            Ok(()) if what == "text" => self.info(format!("Copied {} characters", text.chars().count())),
            Ok(()) => self.info(format!("Copied {what}: {text}")),
            Err(e) => self.error(format!("Couldn't copy: {e:#}")),
        }
        self.copied = Some(text);
    }

    /// A link to a thread (and post) on its site.
    fn thread_link(&self, key: &ThreadKey, post: Option<u64>) -> Option<String> {
        let site = self.sites.iter().find(|s| s.cfg.name == key.site)?;
        Some(match post {
            Some(p) if p != key.no => site.backend.post_url(&key.board, key.no, p),
            _ => site.backend.thread_url(&key.board, key.no),
        })
    }

    /// The link to what's selected: a post in a thread, a catalog thread, a saved thread, a board.
    fn selected_link(&self) -> Option<String> {
        let backend = &self.current_site().backend;
        match (self.tab.view, &self.tab.board) {
            (View::Watched, _) => self.selected_index().and_then(|i| self.thread_link(&self.store.watched[i].key, None)),
            (View::History, _) => self.selected_index().and_then(|i| self.thread_link(&self.store.history[i].key, None)),
            (View::Saved, _) => self.selected_index().and_then(|i| self.thread_link(&self.store.saved[i].key, None)),
            (View::Boards, _) => self.selected_index().map(|i| backend.board_url(&self.boards()[i].uri)),
            (View::Catalog, Some(b)) => self.selected_index().map(|i| {
                let p = &self.tab.catalog[i];
                backend.thread_url(p.board.as_deref().unwrap_or(&b.uri), p.no)
            }),
            (View::Thread, Some(b)) => self.tab.thread.as_ref().and_then(|t| self.thread_link(&self.key(&b.uri, t.no), Some(t.current()?.no))),
            _ => None,
        }
    }

    fn open_in_browser(&mut self) {
        if let Some(url) = self.selected_link() {
            self.open_url(&url);
        }
    }

    pub fn open_url(&mut self, url: &str) {
        self.opened = Some(url.to_string());
        if crate::sandboxed() {
            return;
        }
        match open::that_detached(url) {
            Ok(()) => self.info(format!("Opened {url}")),
            Err(e) => self.error(format!("Couldn't open {url}: {e}")),
        }
    }
}

/// A post's text as plain text, line by line (spoilers included); with `subject`, the
/// subject first.
pub fn copy_text(p: &Post, subject: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(s) = p.subject.as_ref().filter(|_| subject) {
        lines.push(s.clone());
    }
    lines.extend(p.body.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>().trim_end().to_string()));
    lines.join("\n").trim().to_string()
}

fn plural_posts(n: usize) -> String {
    if n == 1 { "1 post".into() } else { format!("{n} posts") }
}

/// A thread's subject for lists: its subject, or the start of the OP's text.
pub fn thread_subject(posts: &[Post]) -> String {
    let Some(op) = posts.first() else { return String::new() };
    op.subject.clone().unwrap_or_else(|| op.plain_text().chars().take(80).collect())
}

pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// The indices of `n` items whose `text` contains the filter (any case). The text is only
/// made while there's a filter.
/// Where j/k (or the arrows) move one row and g/G (or home/end) jump to the ends of a list
/// of `len` rows, from row `cur`; `None` for other keys.
fn list_move(code: KeyCode, cur: usize, len: usize) -> Option<usize> {
    let last = len.saturating_sub(1);
    match code {
        KeyCode::Char('j') | KeyCode::Down => Some((cur + 1).min(last)),
        KeyCode::Char('k') | KeyCode::Up => Some(cur.saturating_sub(1)),
        KeyCode::Char('g') | KeyCode::Home => Some(0),
        KeyCode::Char('G') | KeyCode::End => Some(last),
        _ => None,
    }
}

/// Backspace and typed characters in a text field; other keys do nothing.
fn edit_text(text: &mut String, code: KeyCode) {
    match code {
        KeyCode::Backspace => {
            text.pop();
        }
        KeyCode::Char(c) => text.push(c),
        _ => {}
    }
}

fn filtered(filter: &str, n: usize, text: impl Fn(usize) -> String) -> Vec<usize> {
    let needle = filter.to_lowercase();
    (0..n).filter(|&i| needle.is_empty() || text(i).to_lowercase().contains(&needle)).collect()
}

#[cfg(test)]
mod fuzz;
#[cfg(test)]
pub mod tests;
