//! What's hidden, decided here and nowhere else: filters, hidden words, hiding by hand and
//! hiding replies to hidden posts. Everything else gets the answer as `Marks`, with `Z` and
//! (You) already applied. Whatever changes what's hidden says so with a `Changed`, which
//! only `App::rehide` takes: it marks every tab again and recounts Watched.

use std::collections::{HashMap, HashSet};

use super::{App, ThreadView};
use crate::filter::{Filters, Hidden, Mark};
use crate::model::Post;
use crate::store::ThreadKey;

/// What hiding says about a list of threads or posts (by index), as shown: whether hidden
/// ones are (`Z`), and which posts are yours.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Marks {
    marks: Vec<Mark>,
    show_hidden: bool,
    mine: HashSet<u64>,
}

impl Marks {
    /// Whether it's shown: not hidden, or hidden ones are.
    pub fn shown(&self, i: usize) -> bool {
        self.show_hidden || self.why_hidden(i).is_none()
    }

    /// Why it's hidden, whether hidden ones are shown or not.
    pub fn why_hidden(&self, i: usize) -> Option<&Hidden> {
        self.marks.get(i)?.hidden.as_ref()
    }

    /// The label of the filter that highlights it.
    pub fn highlight(&self, i: usize) -> Option<&str> {
        self.marks.get(i)?.highlight.as_deref()
    }

    /// A `top` filter puts it first.
    pub fn top(&self, i: usize) -> bool {
        self.marks.get(i).is_some_and(|m| m.top)
    }

    pub fn is_mine(&self, no: u64) -> bool {
        self.mine.contains(&no)
    }

    pub fn mine(&self) -> &HashSet<u64> {
        &self.mine
    }

    pub fn hidden_count(&self) -> usize {
        self.marks.iter().filter(|m| m.hidden.is_some()).count()
    }

    /// How many the filter (or hidden word) with this label hides.
    pub fn hidden_by(&self, label: &str) -> usize {
        self.marks.iter().filter(|m| m.hidden.as_ref().and_then(Hidden::filter) == Some(label)).count()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    #[cfg(test)]
    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Why each is hidden.
    #[cfg(test)]
    pub fn all_hidden(&self) -> Vec<Option<Hidden>> {
        self.marks.iter().map(|m| m.hidden.clone()).collect()
    }

    #[cfg(test)]
    pub fn from_marks(marks: Vec<Mark>) -> Self {
        Self { marks, ..Default::default() }
    }
}

/// What's hidden changed: give it to `App::rehide`.
#[must_use = "pass it to App::rehide, which re-derives every tab's and watched thread's hiding"]
pub struct Changed<T>(T);

impl<T> Changed<T> {
    pub(crate) fn new(t: T) -> Self {
        Self(t)
    }

    /// The value, without marking anything again (tests of the store alone).
    #[cfg(test)]
    pub fn unchecked(self) -> T {
        self.0
    }
}

/// What decides what's hidden, besides what the store keeps.
pub struct Hiding {
    filters: Filters,
    /// `recursive_hiding`: in a thread, replies to a hidden post are hidden with it.
    recursive: bool,
    /// `Z`: hidden threads and posts are shown (dimmed) instead of left out.
    show: bool,
}

impl Hiding {
    pub fn new(filters: Filters, recursive: bool) -> Self {
        Self { filters, recursive, show: false }
    }

    pub fn filters(&self) -> &Filters {
        &self.filters
    }

    pub fn recursive(&self) -> bool {
        self.recursive
    }

    pub fn show(&self) -> bool {
        self.show
    }

    pub fn set_filters(&mut self, f: Filters) -> Changed<()> {
        self.filters = f;
        Changed(())
    }

    pub fn set_recursive(&mut self, on: bool) -> Changed<()> {
        self.recursive = on;
        Changed(())
    }

    /// Show hidden ones, or leave them out again; returns whether they're shown now.
    pub fn toggle_show(&mut self) -> Changed<bool> {
        self.show = !self.show;
        Changed(self.show)
    }
}

/// Which of `posts` are the OP, the ones `keep` picks, and the posts they quote, on up: all
/// that decides whether those are hidden. `links`: a post's number and what it quotes.
pub fn ancestry<P>(posts: &[P], links: impl Fn(&P) -> (u64, &[u64]), keep: impl Fn(&P) -> bool) -> Vec<bool> {
    let mut taken = vec![false; posts.len()];
    let mut stack: Vec<usize> = posts.iter().enumerate().filter(|(_, p)| keep(p)).map(|(i, _)| i).collect();
    if stack.is_empty() {
        return taken;
    }
    let index: HashMap<u64, usize> = posts.iter().enumerate().map(|(i, p)| (links(p).0, i)).collect();
    stack.push(0);
    while let Some(i) = stack.pop() {
        if taken.get_mut(i).is_none_or(|t| std::mem::replace(t, true)) {
            continue;
        }
        stack.extend(posts.get(i).into_iter().flat_map(|p| links(p).1).filter_map(|q| index.get(q).copied()));
    }
    taken
}

/// The posts `ancestry` takes. Nothing when `keep` picks none.
pub fn with_ancestry(posts: &[Post], keep: impl Fn(&Post) -> bool) -> Vec<Post> {
    let taken = ancestry(posts, |p| (p.no, &p.quotes), keep);
    posts.iter().zip(taken).filter(|(_, t)| *t).map(|(p, _)| p.clone()).collect()
}

impl App {
    /// What filters and hiding by hand say about posts (on the board `board_of` gives each).
    fn post_marks(&self, site: &str, posts: &[Post], is_op: impl Fn(usize, &Post) -> bool, board_of: impl Fn(&Post) -> String) -> Vec<Mark> {
        let mut by_hand: HashMap<String, HashSet<u64>> = HashMap::new();
        let mark = |(i, p): (usize, &Post)| {
            let board = board_of(p);
            let mut m = self.hiding.filters.check(site, &board, p, is_op(i, p));
            if m.hidden.is_none() && by_hand.entry(board).or_insert_with_key(|b| self.store.hidden_on(site, b)).contains(&p.no) {
                m.hidden = Some(Hidden::ByHand);
            }
            m
        };
        posts.iter().enumerate().map(mark).collect()
    }

    /// A thread's posts' marks, replies to hidden ones included (`recursive_hiding`, or a
    /// `recursive` filter).
    fn spread_marks(&self, site: &str, t: &ThreadView, is_op: impl Fn(usize, &Post) -> bool) -> Vec<Mark> {
        let mut marks = self.post_marks(site, &t.posts, is_op, |_| t.board.clone());
        let all = self.hiding.recursive;
        crate::filter::spread_hiding(&mut marks, &t.posts, &t.index, &t.backlinks, |m| all || m.recursive);
        marks
    }

    /// A catalog's threads' marks.
    pub(super) fn catalog_marks_for(&self, site: &str, posts: &[Post], board_of: impl Fn(&Post) -> String) -> Marks {
        Marks { marks: self.post_marks(site, posts, |_, _| true, board_of), show_hidden: self.hiding.show, mine: HashSet::new() }
    }

    /// A thread's posts' marks, and which are yours.
    pub(super) fn thread_marks(&self, site: &str, t: &ThreadView) -> Marks {
        let key = ThreadKey { site: site.to_string(), board: t.board.clone(), no: t.no };
        let mine = self.store.watched(&key).map(|w| w.mine().iter().copied().collect()).unwrap_or_default();
        Marks { marks: self.spread_marks(site, t, |i, _| i == 0), show_hidden: self.hiding.show, mine }
    }

    /// Mark the search's results not marked yet. An archive's each alone; a saved copy's
    /// with the posts they quote, on up, so a reply hidden with a hidden post is here too.
    pub(super) fn mark_hits(&mut self) {
        let Some(mut s) = self.tab.search.take() else { return };
        let from = s.marks.marks.len();
        let marks = |site: &str, t: &ThreadView| self.spread_marks(site, t, |_, p| p.no == t.no);
        let mark = |t: &ThreadView, marks: &[Mark], no: u64| t.index.get(&no).and_then(|&i| marks.get(i)).cloned().unwrap_or_default();
        let mut found = Vec::new();
        match &s.saved {
            None => {
                let site = &self.current_site().cfg.name;
                for (thread, p) in s.hits.iter().skip(from) {
                    let t = ThreadView::new(p.board.clone().unwrap_or_else(|| s.board.clone()), *thread, vec![p.clone()]);
                    found.push(mark(&t, &marks(site, &t), p.no));
                }
            }
            Some(saved) => {
                let mut first = 0;
                for (t, n) in &saved.copies {
                    let end = first + n;
                    if let Some(key) = saved.keys.get(first).filter(|_| end > from) {
                        let marks = marks(&key.site, t);
                        found.extend(s.hits.iter().take(end).skip(from.max(first)).map(|(_, p)| mark(t, &marks, p.no)));
                    }
                    first = end;
                }
            }
        }
        s.marks.marks.extend(found);
        s.marks.show_hidden = self.hiding.show;
        self.tab.search = Some(s);
    }

    /// A watched thread's new posts (numbered past the number given) and those replying to
    /// yours, leaving out hidden ones whether `Z` shows them or not. `posts`: those new posts
    /// with all that decides their hiding (`with_ancestry`).
    pub(super) fn unread_counter(&self, key: &ThreadKey, posts: &[Post]) -> impl Fn(u64) -> (usize, usize) + use<> {
        let t = ThreadView::new(key.board.clone(), key.no, posts.to_vec());
        let marks = self.thread_marks(&key.site, &t);
        move |after| {
            let new: Vec<&Post> = t.posts.iter().enumerate().filter(|(i, p)| p.no > after && marks.why_hidden(*i).is_none()).map(|(_, p)| p).collect();
            let replies = new.iter().filter(|p| p.quotes.iter().any(|&q| marks.is_mine(q))).count();
            (new.len(), replies)
        }
    }

    /// The catalog's threads shown, by index.
    pub fn shown_catalog(&self) -> impl Iterator<Item = (usize, &Post)> {
        self.tab.catalog.iter().enumerate().filter(|&(i, _)| self.tab.catalog_marks.shown(i))
    }

    /// What's hidden changed: mark every tab's catalog, thread and search results again,
    /// and recount the watched threads' new posts.
    pub fn rehide<T>(&mut self, change: impl FnOnce(&mut Self) -> Changed<T>) -> T {
        let Changed(t) = change(self);
        let active = self.active;
        for i in (0..self.tabs.len()).filter(|&i| i != active).chain([active]) {
            self.switch_tab(i);
            self.rehide_tab();
        }
        let counts: Vec<_> = self.store.all_watched().iter().filter(|w| !w.fresh.is_empty()).map(|w| (w.key.clone(), self.unread_counter(&w.key, &w.fresh)(w.last_seen))).collect();
        for (key, (unread, replies)) in counts {
            if let Some(w) = self.store.watched_mut(&key) {
                (w.unread, w.replies) = (unread, replies);
            }
        }
        t
    }

    /// Mark every tab again, as any change does.
    #[cfg(test)]
    pub fn remark(&mut self) {
        self.rehide(|_| Changed(()));
    }

    fn rehide_tab(&mut self) {
        // Each by the site it's from: a search moves the tab to the archive, leaving them.
        let site_name = |i: usize| self.sites.get(i).map_or(String::new(), |s| s.cfg.name.clone());
        let (catalog_site, thread_site) = (site_name(self.tab.catalog_site), site_name(self.tab.thread_site));
        self.tab.catalog_marks = self.catalog_marks_for(&catalog_site, &self.tab.catalog, |p| self.board_of(p));
        if let Some(t) = &self.tab.thread {
            let marks = self.thread_marks(&thread_site, t);
            if let Some(t) = self.tab.thread.as_mut().filter(|t| t.marks != marks) {
                t.marks = marks;
                t.layout = None;
                // What's hidden changed, so what a search finds may have.
                if !t.search.is_empty() {
                    t.set_search(t.search.clone());
                }
                // A post just collapsed (hidden) has no parts to focus.
                if t.focus.as_ref().is_some_and(|f| !t.parts_of(t.entry()).contains(f)) {
                    t.focus = None;
                }
            }
        }
        if let Some(s) = &mut self.tab.search {
            s.marks = Marks::default();
        }
        self.mark_hits();
        let (catalog, hits) = (self.visible_catalog().len(), self.visible_hits().len());
        self.tab.catalog_list.clamp(catalog);
        self.tab.search_list.clamp(hits);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestry_is_the_op_the_kept_and_what_they_quote() {
        let post = |no, quotes: &[u64]| Post { no, quotes: quotes.to_vec(), ..Default::default() };
        let posts = [post(1, &[]), post(2, &[]), post(3, &[2]), post(4, &[3, 99]), post(5, &[1]), post(6, &[5])];
        let nos = |v: Vec<Post>| v.iter().map(|p| p.no).collect::<Vec<_>>();
        assert_eq!(nos(with_ancestry(&posts, |p| p.no == 4)), [1, 2, 3, 4]);
        assert_eq!(nos(with_ancestry(&posts, |p| p.no > 5)), [1, 5, 6]);
        assert!(with_ancestry(&posts, |p| p.no > 6).is_empty());
    }
}
