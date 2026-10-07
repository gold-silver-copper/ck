//! The thread as shown: its entries (posts, and replies shown under them), the selection and
//! scrolling, search, conversations, and the layout the UI draws from.

use super::*;

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
/// quote, on up), and what quotes it (and what quotes those, on down). Or, with `poster`,
/// every post by that post's poster (`I`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    /// The post it's about.
    pub anchor: u64,
    /// Showing the posts with this poster ID instead.
    pub poster: Option<String>,
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
            let next = if up { posts.get(i).map(|p| &p.quotes) } else { backlinks.get(i) };
            let d = if up { d - 1 } else { d + 1 };
            for no in next.into_iter().flatten() {
                let Some(&j) = index.get(no) else { continue };
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

/// Which posts a thread shows by their files (`M`, in turn): all of them, only those with
/// files (and the OP, which is the thread), or all of them with their images hidden.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Media {
    #[default]
    All,
    Files,
    NoImages,
}

impl Media {
    pub fn next(self) -> Self {
        match self {
            Media::All => Media::Files,
            Media::Files => Media::NoImages,
            Media::NoImages => Media::All,
        }
    }

    /// The top bar's chip.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Media::All => None,
            Media::Files => Some("with files"),
            Media::NoImages => Some("images hidden"),
        }
    }
}

/// A refresh with fewer posts than 1/`SHRUNK` of those shown is more likely a broken answer
/// (cut short, or a page the site sent while in trouble) than moderators deleting most of
/// the thread: it's shown as it came, and the posts it leaves out aren't kept as deleted.
pub const SHRUNK: usize = 2;

/// A refresh of `old`'s thread, with the posts shown before that it leaves out put back
/// where they were (by number), and their numbers: deleted on the site. Posts still deleted
/// stay; one back again isn't any more. True when it came back too small to trust
/// (`SHRUNK`): then it's as it came.
pub fn keep_deleted(old: &ThreadView, fetched: Vec<Post>) -> (Vec<Post>, HashSet<u64>, bool) {
    let live = old.posts.len().saturating_sub(old.deleted.len());
    if fetched.len().saturating_mul(SHRUNK) < live {
        return (fetched, HashSet::new(), true);
    }
    let have: HashSet<u64> = fetched.iter().map(|p| p.no).collect();
    // (The OP is the thread: a refresh without it is another thread.)
    let mut gone: Vec<&Post> = old.posts.iter().skip(1).filter(|p| !have.contains(&p.no)).collect();
    if gone.is_empty() {
        return (fetched, HashSet::new(), false);
    }
    gone.sort_by_key(|p| p.no);
    let deleted = gone.iter().map(|p| p.no).collect();
    // Merged in, the fetched posts keep their order (the OP first).
    let mut out = Vec::with_capacity(fetched.len() + gone.len());
    let mut gone = gone.into_iter().peekable();
    let mut fetched = fetched.into_iter();
    out.extend(fetched.next());
    for p in fetched {
        while let Some(g) = gone.next_if(|g| g.no < p.no) {
            out.push(g.clone());
        }
        out.push(p);
    }
    out.extend(gone.cloned());
    (out, deleted, false)
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
    pub(super) jumps: Vec<usize>,
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
    /// Posts (by number) shown before that a refresh left out: deleted on the site, kept
    /// here as they were. In memory only.
    pub deleted: HashSet<u64>,
    /// Refreshes that came back too small to keep what they left out (`SHRUNK`).
    pub shrinks: u32,
    /// How many posts each poster ID has in the thread.
    ids: HashMap<String, usize>,
    /// Only the posts with files, or images hidden (`M`).
    pub media: Media,
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
    /// `G`: the very end, the post's bottom at the screen's bottom (whatever the margin).
    Bottom,
}

/// A part of a post that can take focus, be clicked or get a hint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Part {
    /// The poster ID in the header: shows that poster's posts alone.
    Poster,
    File(usize),
    Link(crate::model::Target),
    /// The "Replies" label: shows them inline.
    Replies,
}

/// A post's parts in reading order: its poster ID, its files, the links in its text, then
/// its replies.
pub fn parts(p: &Post, backlinks: &[u64]) -> Vec<Part> {
    let mut out: Vec<Part> = p.id.iter().map(|_| Part::Poster).collect();
    out.extend((0..p.files.len()).map(Part::File));
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
        let thumbs = self.has_thumb.iter().zip(&self.starts).take(self.blocks.len()).enumerate();
        self.thumbs = thumbs.filter(|(_, (has, _))| **has).map(|(e, (_, start))| (start + 1, e)).collect();
    }

    /// The entry line `i` is in.
    pub fn entry_at(&self, i: usize) -> usize {
        self.starts.partition_point(|&s| s <= i).saturating_sub(1).min(self.blocks.len().saturating_sub(1))
    }
    pub fn len(&self) -> usize {
        self.starts.last().copied().unwrap_or(0)
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
                    && let Some(b) = backlinks.get_mut(i)
                    && !b.contains(&p.no)
                {
                    b.push(p.no);
                }
            }
        }
        let entries = posts.iter().enumerate().map(|(i, p)| Entry { post: i, depth: 0, path: vec![p.no] }).collect();
        let mut ids: HashMap<String, usize> = HashMap::new();
        for id in posts.iter().filter_map(|p| p.id.as_ref()) {
            *ids.entry(id.clone()).or_default() += 1;
        }
        Self { entries, board, no, posts, index, backlinks, ids, ..Default::default() }
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
            let found = |i: usize, text: &mut Option<String>| {
                if self.is_revealed(i) {
                    return self.post_text(i).contains(&needle);
                }
                text.get_or_insert_with(|| self.post_text(i)).contains(&needle)
            };
            // Not in hidden posts (unless shown): a match would say what they hide.
            let m = texts.iter_mut().enumerate().filter_map(|(i, text)| (self.in_view(i) && !self.is_collapsed(i) && found(i, text)).then_some(i)).collect();
            self.search_texts = texts;
            m
        };
        self.search = query;
        self.layout = None;
    }

    /// Lowercase searchable text of a post: name, subject, files and body (hidden spoilers excluded).
    fn post_text(&self, i: usize) -> String {
        let Some(p) = self.posts.get(i) else { return String::new() };
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
    pub(super) fn next_match(&self, forward: bool) -> Option<usize> {
        if forward {
            self.matches.iter().find(|&&i| i > self.selected).or_else(|| self.matches.first()).copied()
        } else {
            self.matches.iter().rev().find(|&&i| i < self.selected).or_else(|| self.matches.last()).copied()
        }
    }

    /// Whether post `i` is shown: a conversation's posts while one is (all of them, files or
    /// not); else all, or with `Media::Files` the OP and the posts with files.
    pub fn in_view(&self, i: usize) -> bool {
        match &self.conversation {
            Some(c) => c.depth.contains_key(&i),
            None => self.media != Media::Files || i == 0 || self.posts.get(i).is_some_and(|p| !p.files.is_empty()),
        }
    }

    /// Show the posts `media` says, keeping the selected post if it's still shown (else the
    /// next one that is). How many posts are shown.
    pub fn set_media(&mut self, media: Media) -> usize {
        self.media = media;
        self.focus = None;
        self.rebuild_entries();
        self.set_search(self.search.clone());
        self.reveal = Some(Reveal::Jump);
        self.entries.iter().filter(|e| e.path.len() == 1).count()
    }

    /// `c`: show the selected post's conversation alone.
    pub fn enter_conversation(&mut self) -> Result<usize, String> {
        let Some(p) = self.current() else { return Err("No posts".into()) };
        let no = p.no;
        let quotes = p.quotes.iter().any(|q| self.index.contains_key(q));
        if !quotes && self.backlinks.get(self.selected).is_none_or(Vec::is_empty) {
            return Err(format!("No.{no} isn't part of a conversation here: it quotes no post in the thread, and none quote it"));
        }
        Ok(self.show_only(no, None))
    }

    /// `I`: only the posts by the selected post's poster (by its poster ID).
    pub fn enter_poster(&mut self) -> Result<usize, String> {
        let Some(p) = self.current() else { return Err("No posts".into()) };
        let Some(id) = p.id.clone() else { return Err(format!("No.{} has no poster ID (the board doesn't give them)", p.no)) };
        Ok(self.show_only(p.no, Some(id)))
    }

    /// Show a conversation (or a poster's posts) instead of the whole thread; how many posts.
    fn show_only(&mut self, anchor: u64, poster: Option<String>) -> usize {
        // From another conversation, back is still to where the whole thread was.
        let back_scroll = self.conversation.as_ref().map_or(self.scroll, |c| c.back_scroll);
        self.conversation = Some(Conversation { anchor, poster, depth: Default::default(), capped: false, back_scroll });
        self.focus = None;
        self.scroll = 0;
        self.rebuild_entries();
        self.set_search(self.search.clone());
        self.conversation.as_ref().map_or(0, |c| c.depth.len())
    }

    /// The posts with poster ID `id`, as a conversation's posts (all at the top level).
    pub fn posts_by(&self, id: &str) -> std::collections::BTreeMap<usize, i32> {
        self.posts.iter().enumerate().filter(|(_, p)| p.id.as_deref() == Some(id)).map(|(i, _)| (i, 0)).collect()
    }

    /// How many posts of the thread have poster ID `id`.
    pub fn id_count(&self, id: &str) -> usize {
        self.ids.get(id).copied().unwrap_or(0)
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

    /// Whether post `i` arrived since the last visit. A deleted one never counts: it was
    /// shown before.
    pub fn is_new(&self, i: usize) -> bool {
        self.new_after > 0 && self.posts.get(i).is_some_and(|p| p.no > self.new_after) && !self.is_deleted(i)
    }

    /// Whether post `i` is kept after the site deleted it.
    pub fn is_deleted(&self, i: usize) -> bool {
        !self.deleted.is_empty() && self.posts.get(i).is_some_and(|p| self.deleted.contains(&p.no))
    }

    /// The posts as the site has them, without the deleted ones kept: what's counted,
    /// watched and saved.
    pub fn live_posts(&self) -> std::borrow::Cow<'_, [Post]> {
        if self.deleted.is_empty() {
            std::borrow::Cow::Borrowed(&self.posts)
        } else {
            std::borrow::Cow::Owned(self.posts.iter().filter(|p| !self.deleted.contains(&p.no)).cloned().collect())
        }
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
    pub(super) fn rebuild_entries(&mut self) {
        let path = self.entries.get(self.entry()).map(|e| e.path.clone());
        if let Some(mut c) = self.conversation.take() {
            match (self.index.get(&c.anchor), &c.poster) {
                (Some(_), Some(id)) => c.depth = self.posts_by(id),
                (Some(&p), None) => (c.depth, c.capped) = conversation_of(&self.posts, &self.index, &self.backlinks, p),
                (None, _) => {}
            }
            self.conversation = Some(c).filter(|c| self.index.contains_key(&c.anchor));
        }
        let shown: Vec<(usize, i32)> = match &self.conversation {
            Some(c) => c.depth.iter().map(|(&i, &d)| (i, d)).collect(),
            None => (0..self.posts.len()).filter(|&i| self.in_view(i)).map(|i| (i, 0)).collect(),
        };
        let mut out = Vec::with_capacity(shown.len());
        for (i, d) in shown {
            let Some(p) = self.posts.get(i) else { continue };
            let path = vec![p.no];
            // Replies are indented by how far down from the conversation's post they are.
            let depth = d.clamp(0, MAX_DEPTH as i32 - 1) as u8;
            out.push(Entry { post: i, depth, path: path.clone() });
            self.push_replies(&mut out, i, &path, depth + 1);
        }
        self.entries = out;
        self.layout = None;
        match path.and_then(|p| self.entries.iter().position(|e| e.path == p)) {
            Some(e) => self.set_cursor(e),
            // The selected post, if it's still shown; else the next one that is (or the last).
            None => match self.entries.iter().position(|e| e.path.len() == 1 && e.post >= self.selected) {
                Some(e) => self.set_cursor(e),
                None if !self.entries.is_empty() => self.set_cursor(self.entries.len() - 1),
                None => {
                    self.cursor = 0;
                    self.selected = 0;
                }
            },
        }
    }

    fn push_replies(&self, out: &mut Vec<Entry>, post: usize, path: &[u64], depth: u8) {
        if depth > MAX_DEPTH || !self.expanded.contains(path) {
            return;
        }
        for no in self.backlinks.get(post).into_iter().flatten() {
            // A post can't contain itself (quote loops).
            let Some(&j) = self.index.get(no).filter(|_| !path.contains(no)) else { continue };
            let mut p = path.to_vec();
            p.push(*no);
            out.push(Entry { post: j, depth, path: p.clone() });
            self.push_replies(out, j, &p, depth + 1);
        }
    }

    /// `e`: show or hide the selected entry's replies under it. Returns what happened.
    pub(super) fn toggle_expanded(&mut self) -> Result<bool, &'static str> {
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

    /// New posts whose entries start below the screen, and how many of those quote yours.
    pub fn new_below(&self) -> (usize, usize) {
        let Some(l) = &self.layout else { return (0, 0) };
        let bottom = self.scroll + self.viewport;
        let below = self.entries.iter().enumerate().filter(|&(e, x)| l.starts.get(e).is_some_and(|&s| s >= bottom) && self.is_new(x.post));
        below.fold((0, 0), |(n, yours), (_, x)| {
            let to_you = self.posts.get(x.post).is_some_and(|p| p.quotes.iter().any(|q| self.mine.contains(q)));
            (n + 1, yours + usize::from(to_you))
        })
    }

    /// The first new post's entry, when posts read before come above it: the unread line
    /// goes before it. (It's drawn in the gap above the entry, so the layout is the same.)
    pub fn unread_line(&self) -> Option<usize> {
        if self.new_after == 0 {
            return None;
        }
        self.entries.iter().position(|e| e.path.len() == 1 && self.is_new(e.post)).filter(|&e| e > 0)
    }

    pub(super) fn top_anchor(&self) -> Option<(usize, usize)> {
        let l = self.layout.as_ref()?;
        let top = l.starts.partition_point(|&s| s <= self.scroll).saturating_sub(1);
        Some((top, self.scroll - l.starts.get(top)?))
    }

    /// Select a post's top-level entry. A post outside the conversation shown leaves it.
    pub fn select(&mut self, i: usize) {
        let i = i.min(self.posts.len().saturating_sub(1));
        if !self.in_view(i) {
            self.leave_conversation();
        }
        // A post without files, with only those shown: all of them again.
        if !self.in_view(i) {
            self.media = Media::All;
            self.rebuild_entries();
            self.set_search(self.search.clone());
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
        if let Some(l) = self.layout.as_ref().filter(|l| l.exact.get(e) == Some(&true))
            && let (Some(&start), Some(&end)) = (l.starts.get(e), l.starts.get(e + 1))
        {
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

    pub(super) fn select_entry(&mut self, e: usize) {
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

    /// Scroll the selected entry into view, `how`.
    pub fn scroll_to(&mut self, how: Reveal) {
        let e = self.entry();
        let Some((&start, &end, len)) = self.layout.as_ref().and_then(|l| Some((l.starts.get(e)?, l.starts.get(e + 1)?, l.len()))) else { return };
        let view = self.viewport;
        let m = if how == Reveal::Visible { 0 } else { self.margin_lines() };
        match how {
            Reveal::Bottom => self.scroll = end.saturating_sub(view),
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
    pub(super) fn scroll_lines(&mut self, delta: isize) {
        let Some(l) = &self.layout else { return };
        let max = l.len().saturating_sub(self.viewport);
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
        let top = l.starts.partition_point(|&s| s <= self.scroll).saturating_sub(1);
        // Prefer an entry whose header is on screen.
        let e = if l.starts.get(top).is_some_and(|&s| s < self.scroll) && top + 1 < self.entries.len() && l.starts.get(top + 1).is_some_and(|&s| s < self.scroll + self.viewport) {
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
