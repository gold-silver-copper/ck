//! Loading boards, catalogs and threads (for the tab, or in the background), showing what
//! arrives, and keeping open and watched threads refreshed.

use super::*;

/// A site's answer for a thread whole enough to count, visit and keep: only `App::accept`
/// makes one (and `App::shown_whole`, of what's shown).
pub struct Whole(Thread);

impl Whole {
    pub fn posts(&self) -> &[Post] {
        self.0.posts()
    }

    pub fn into_posts(self) -> Vec<Post> {
        self.0.into_posts()
    }

    /// Tests' shorthand: `t` taken as whole.
    #[cfg(test)]
    pub(crate) fn assumed(t: Thread) -> Whole {
        Whole(t)
    }
}

impl App {
    /// Run `job` on a thread, as the tab's load (shown as `label`, to do `then` once it's
    /// answered), then `apply` what it came to in the load's tab, if it's still the tab's
    /// load. The job gets the request id and a sender for partial results.
    pub(super) fn spawn<T: Send + 'static>(
        &mut self,
        label: String,
        then: Then,
        job: impl FnOnce(&dyn Backend, u64, &Sender<Msg>) -> Result<T> + Send + 'static,
        apply: impl FnOnce(&mut App, Result<T>) + Send + 'static,
    ) {
        self.start(label, then, job, move |id, res| Msg::answer(id, res, apply));
    }

    /// `spawn`, with `wrap` making the message its result is sent back in.
    fn start<T: Send + 'static>(
        &mut self,
        label: String,
        then: Then,
        job: impl FnOnce(&dyn Backend, u64, &Sender<Msg>) -> Result<T> + Send + 'static,
        wrap: impl FnOnce(u64, Result<T>) -> Msg + Send + 'static,
    ) {
        self.next_id += 1;
        let id = self.next_id;
        self.tab.begin(id, label, then);
        let backend = self.current_site().backend.clone();
        let tx = self.tx.clone();
        self.footer.clear_seen();
        std::thread::spawn(move || {
            let res = crate::guard::result(|| job(&*backend, id, &tx));
            let cached = http::take_cached_age();
            let _ = tx.send(wrap(id, res));
            if let Some(age) = cached {
                let _ = tx.send(Msg::request(id, move |app| app.up_to_date(age)));
            }
        });
    }

    /// Show a site's boards; a complete list is also saved for next time.
    pub(super) fn set_boards(&mut self, site: usize, boards: Vec<Board>, complete: bool) {
        let Some(s) = self.sites.get(site) else { return };
        if complete && s.cfg.boards.is_none() {
            let name = s.cfg.name.clone();
            if let Err(e) = self.store.save_boards(&name, &boards, self.clock.now()) {
                self.error(e.context("Couldn't save the board list"));
            }
        }
        if let Some(s) = self.sites.get_mut(site) {
            s.boards = Some(boards);
        }
        if complete {
            self.note_titles(site);
        }
    }

    /// Refresh a saved board list at background priority, without a spinner.
    pub(super) fn refresh_boards_in_background(&mut self, site: usize) {
        let Some(backend) = self.sites.get(site).map(|s| s.backend.clone()) else { return };
        let now = self.clock.instant();
        if self.boards_tried.get(&site).is_some_and(|t| now.saturating_duration_since(*t) < http::MIN_REFETCH)
            || !self.boards_refreshing.insert(site)
        {
            return;
        }
        self.boards_tried.insert(site, now);
        let later = self.later();
        std::thread::spawn(move || {
            let res = crate::guard::result(|| http::background_at(now, || backend.boards(&|_| {})));
            later.run(move |app| {
                app.boards_refreshing.remove(&site);
                // A failed background refresh keeps the saved list; there's nothing to say.
                if let Ok(b) = res {
                    app.set_boards(site, b, true);
                }
            });
        });
    }

    pub(super) fn load_boards(&mut self) {
        let site = self.tab.site;
        let label = format!("Loading boards for {}", self.current_site().cfg.name);
        // A finished list is worth keeping even once the tab has moved on.
        self.start(
            label,
            Then::Show,
            move |b, id, tx| {
                b.boards(&|so_far| {
                    let so_far = so_far.to_vec();
                    let _ = tx.send(Msg::request(id, move |app| app.set_boards(site, so_far, false)));
                })
            },
            move |id, res| Msg::kept(id, move |app| app.boards_arrived(id, site, res)),
        );
    }

    /// The request was answered from the cache without hitting the network.
    fn up_to_date(&mut self, age: Duration) {
        self.footer.offer(format!("Up to date (checked {}s ago)", age.as_secs()));
    }

    /// A site's board list arrived for request `id`; it's shown even if the tab has moved on.
    pub(super) fn boards_arrived(&mut self, id: u64, site: usize, res: Result<Vec<Board>>) {
        let current = self.tab.answered(id);
        match res {
            Ok(b) => self.set_boards(site, b, true),
            Err(e) if current => self.load_failed(e),
            Err(e) => self.error(e),
        }
    }

    /// The last copy kept of the catalog being fetched, fetched at `fetched`: shown only
    /// before anything fetched has arrived.
    fn cached_catalog_arrived(&mut self, posts: Vec<Post>, fetched: i64) {
        if self.tab.catalog.is_empty() && self.tab.place_view() == View::Catalog {
            self.show_catalog(posts);
            self.tab.catalog_cached = Some(tabs::Offline { saved: fetched, dead: false });
        }
    }

    pub(super) fn catalog_arrived(&mut self, res: Result<Vec<Post>>) {
        match res {
            Ok(posts) => {
                self.show_catalog(posts);
                self.catalog_seen();
            }
            Err(e) if http::is_not_found(&e) => {
                // Said of the board and site it was asked on (only a load writes them).
                let board = self.tab.catalog_board.clone().unwrap_or_default();
                let site = self.sites.get(self.tab.catalog_site).map_or(String::new(), |s| s.cfg.name.clone());
                self.load_failed(format!("There's no /{board}/ on {site}"));
            }
            Err(e) => self.load_failed(e),
        }
    }

    /// The last copy kept of the thread being fetched, fetched at `fetched`: shown only
    /// before anything fetched (or saved) is.
    fn cached_thread_arrived(&mut self, t: Thread, fetched: i64) {
        if self.tab.thread.is_none() && self.tab.place_view() == View::Thread && self.tab.saved().is_none() {
            self.set_cached_thread(t, fetched);
        }
    }

    /// The thread `key` asked for (its 404 marks it dead) came to `res`, checked as an answer
    /// for it (`Thread::answer`).
    pub(super) fn thread_arrived(&mut self, key: &ThreadKey, res: Result<Thread>) {
        let open = self.tab.opened();
        self.tab.thread_checked = self.clock.instant();
        match res {
            // The thread asked for, on the board and site the tab still shows (a key from
            // when it was asked, never from where the tab is now).
            Ok(t) if self.tab.board.as_ref().is_some_and(|b| self.key(&b.uri, t.no()) == *key) => self.show_thread(t, None, open),
            Ok(_) => {}
            Err(e) if http::is_not_found(&e) => {
                if self.store.mark_dead(key) {
                    self.save();
                }
                if open.restoring {
                    // Last session's thread is gone: its catalog instead.
                    self.tab.navigate(View::Catalog);
                    self.load_catalog();
                    self.info("The thread you had open last time is gone (archived or deleted)");
                } else {
                    self.thread_gone(key);
                }
            }
            Err(e) => self.load_failed(e),
        }
    }

    pub(super) fn load_catalog(&mut self) {
        let Some(board) = self.tab.board.clone() else { return };
        self.tab.catalog_board = Some(board.uri.clone());
        self.tab.catalog_site = self.tab.site;
        self.tab.catalog_of = Some(board.clone());
        self.apply_board_sort();
        // Opening it (nothing shown yet): its last copy shows while it loads.
        let pages = self.pages.clone();
        let read = pages.clone().filter(|_| self.tab.catalog.is_empty());
        let (site, now) = (self.current_site().cfg.name.clone(), self.clock.now());
        let label = format!("Loading /{}/", board.uri);
        let job = move |b: &dyn Backend, id, tx: &Sender<Msg>| {
            let kept = read.and_then(|p| p.read(&site, &board.uri, None));
            if let Some((copies, fetched)) = &kept {
                http::seed(copies);
                if let Ok(posts) = http::from_copies(copies, || b.catalog(&board.uri, &|_| {}))
                    && !posts.is_empty()
                {
                    let fetched = *fetched;
                    let _ = tx.send(Msg::request(id, move |app| app.cached_catalog_arrived(posts, fetched)));
                }
            }
            let (res, copies) = http::recording(|| {
                b.catalog(&board.uri, &|so_far| {
                    let so_far = so_far.to_vec();
                    let _ = tx.send(Msg::request(id, move |app| app.show_catalog(so_far)));
                })
            });
            if let (Ok(posts), Some(p)) = (&res, &pages)
                && !posts.is_empty()
            {
                p.write(&site, &board.uri, None, &copies, now);
            }
            res
        };
        self.spawn(label, Then::Show, job, App::catalog_arrived);
    }

    pub(super) fn load_thread(&mut self, no: u64, open: Opening) {
        let Some(board) = self.tab.board.clone() else { return };
        // Opening it (not reloading what's shown): its last copy shows while it loads.
        let opening = self.tab.thread.as_ref().is_none_or(|t| t.no != no || t.board != board.uri);
        self.tab.pending_thread = Some(no);
        self.tab.archive_offer = None;
        self.tab.saved_offer = None;
        if opening || self.tab.saved().is_some() {
            self.tab.copy = None;
        }
        self.tab.thread_checked = self.clock.instant();
        // Asked for: refreshes start over at the interval set.
        self.tab.thread_quiet = 0;
        let key = self.key(&board.uri, no);
        // A watched thread's saved copy is its copy (kept once, not in the page cache too).
        let saved = self.store.saved(&key).map(|m| m.saved);
        let watched = self.store.watched(&key).is_some();
        // Shown once the load is the tab's, opened as it says.
        let copy = saved.filter(|_| opening && watched).and_then(|at| Some((self.store.load_saved(&key).ok().and_then(Thread::saved)?, at)));
        let pages = self.pages.clone().filter(|_| !watched);
        let read = pages.clone().filter(|_| opening && self.tab.thread.is_none());
        let (site, now) = (key.site.clone(), self.clock.now());
        let job = move |b: &dyn Backend, id, tx: &Sender<Msg>| {
            let kept = read.and_then(|p| p.read(&site, &board.uri, Some(no)));
            if let Some((copies, fetched)) = &kept {
                http::seed(copies);
                if let Ok(t) = http::from_copies(copies, || b.thread(&board.uri, no)) {
                    let fetched = *fetched;
                    let _ = tx.send(Msg::request(id, move |app| app.cached_thread_arrived(t, fetched)));
                }
            }
            let (res, copies) = http::recording(|| b.thread(&board.uri, no));
            if let (Ok(_), Some(p)) = (&res, &pages) {
                p.write(&site, &board.uri, Some(no), &copies, now);
            }
            res
        };
        self.spawn(format!("Loading thread {no}"), Then::Thread { open }, job, move |app, r| app.thread_arrived(&key, r));
        if let Some((t, at)) = copy {
            self.set_cached_thread(t, at);
        }
    }

    /// Show catalog threads as fetched (all, or the pages so far; a kept copy is marked after).
    pub(super) fn show_catalog(&mut self, posts: Vec<Post>) {
        self.tab.catalog_cached = None;
        let site = self.sites.get(self.tab.catalog_site).map_or(String::new(), |s| s.cfg.name.clone());
        self.tab.catalog_marks = self.catalog_marks_for(&site, &posts, |p| self.board_of(p));
        self.tab.catalog = posts;
    }

    /// Show posts as thread fetched, opened plainly (tests' shorthand for an answer of the
    /// thread they start).
    #[cfg(test)]
    pub(crate) fn set_thread(&mut self, posts: Vec<Post>) {
        if let Some(t) = posts.first().map(|p| p.no).and_then(|no| Thread::answer(no, posts).ok()) {
            self.show_thread(t, None, Opening::default());
        }
    }

    /// Show the last copy kept of a thread being fetched, fetched at `at`, opened as the
    /// load says. It's not a visit.
    pub(super) fn set_cached_thread(&mut self, t: Thread, at: i64) {
        let open = self.tab.opening();
        self.show_thread(t, Some(ThreadCopy::Cached(tabs::Offline { saved: at, dead: false })), open);
    }

    /// How many posts a thread was last known with, whole: shown (`shown`), or counted while
    /// watched.
    fn known(&self, key: &ThreadKey, shown: usize) -> usize {
        shown.max(self.store.watched(key).map_or(0, |w| w.posts))
    }

    /// The one place a site's answer for a thread becomes whole enough to count, visit and
    /// keep: not when it has fewer than half the posts last known (`known`), when it comes
    /// back with that number. The answer after one cut short is judged against it, so a real
    /// mass deletion gets through on the next refresh.
    fn accept(&mut self, key: &ThreadKey, t: Thread, shown: usize) -> Result<Whole, (Thread, usize)> {
        let known = self.known(key, shown);
        if shrank(self.short.remove(key).unwrap_or(known), t.posts().len()) {
            self.short.insert(key.clone(), t.posts().len());
            return Err((t, known));
        }
        Ok(Whole(t))
    }

    /// What `view` shows of the thread as the site has it, to keep a copy of, unless it's an
    /// answer cut short (`known`). Not an answer: nothing is remembered.
    pub(super) fn shown_whole(&self, key: &ThreadKey, view: &ThreadView) -> Option<Whole> {
        view.live().filter(|t| !shrank(self.known(key, view.known), t.posts().len())).map(Whole)
    }

    /// Show a thread: fetched (`None`), or a copy. Opening it (not refreshing what's shown),
    /// as `open` says.
    pub(super) fn show_thread(&mut self, t: Thread, copy: Option<ThreadCopy>, open: Opening) {
        // Posts replacing the copy shown while they loaded (fetched ones, or a saved copy): a
        // first open, as far as visits go (the copy wasn't one).
        let replacing = self.tab.cached().is_some() && !matches!(copy, Some(ThreadCopy::Cached(_)));
        // A saved copy is marked at once, a cached one once it's shown; fetched posts end a
        // cached copy, not a saved one.
        match copy {
            Some(ThreadCopy::Saved(_)) => self.tab.copy = copy,
            Some(ThreadCopy::Cached(_)) => self.tab.copy = None,
            None if replacing => self.tab.copy = None,
            None => {}
        }
        let Some(board) = self.tab.board.as_ref().map(|b| b.uri.clone()) else { return };
        let no = t.no();
        let key = self.key(&board, no);
        let old = self.tab.thread.take().filter(|t| t.no == no && t.board == board);
        // On a refresh, the newest post already shown.
        let shown_max = old.as_ref().filter(|_| !replacing).and_then(|o| o.posts.iter().filter(|p| !o.deleted.contains(&p.no)).map(|p| p.no).max());
        // Fetched again over what's shown (not a copy shown while it loaded).
        let refresh = old.as_ref().filter(|_| copy.is_none() && !replacing);
        // What was seen before this visit.
        let last_seen = self.store.last_seen(&key);
        // A cut-short answer from before is for the refresh after it, not for opening it again.
        if refresh.is_none() {
            self.short.remove(&key);
        }
        // A saved or cached copy isn't a visit, and isn't saved again; nor is an answer cut
        // short, which is shown as it came.
        let fetched = if copy.is_some() || self.tab.copy.is_some() { Err((t, 0)) } else { self.accept(&key, t, refresh.map_or(0, |o| o.known)) };
        let (posts, deleted, had) = match fetched {
            Ok(w) => {
                self.fetched_thread(&key, &w, shown_max);
                // Fetched again: posts shown before that it leaves out were deleted, and stay.
                let (posts, deleted) = thread_view::keep_deleted(refresh, w);
                (posts, deleted, 0)
            }
            Err((t, had)) => {
                if had > 0 {
                    self.info(format!("The site sent {} of the {had} posts known; shown as it came, and not kept", t.posts().len()));
                }
                (t.into_posts(), HashSet::new(), had)
            }
        };
        let mut tv = ThreadView::new(board, no, posts);
        tv.marks = self.thread_marks(&key.site, &tv);
        tv.margin = self.scroll_margin;
        tv.deleted = deleted;
        tv.known = had.max(tv.posts.len().saturating_sub(tv.deleted.len()));
        // Reading the end: the last entry shown, to go on from once the posts are marked.
        let mut follow = None;
        match old {
            // On refresh, keep the selected post and what's at the top of the view.
            Some(old) => {
                // Reading the end: posts this brings come into view.
                if self.follow_new_posts && !replacing && old.at_end() {
                    follow = old.entries.last().map(|e| e.path.clone());
                }
                tv.selected = old.current().and_then(|p| tv.index.get(&p.no)).copied().unwrap_or(0);
                // Expanded replies, the selected entry and the one at the top stay put.
                tv.expanded.clone_from(&old.expanded);
                // A conversation stays, with any new replies that belong in it, and so do
                // the posts shown by their files.
                tv.conversation.clone_from(&old.conversation);
                tv.media = old.media;
                let cursor_path = old.entries.get(old.entry()).map(|e| e.path.clone());
                tv.rebuild_entries();
                if let Some(e) = cursor_path.and_then(|p| tv.entry_of(&p)) {
                    tv.set_cursor(e);
                }
                tv.anchor = old.top_anchor().and_then(|(i, off)| Some((tv.entry_of(&old.entries.get(i)?.path)?, off)));
                tv.scroll = old.scroll;
                tv.viewport = old.viewport;
                tv.cache = old.cache;
                tv.cache_width = old.cache_width;
                tv.estimates = old.estimates;
                tv.jumps = old.jumps;
                tv.new_after = old.new_after;
                tv.revealed = old.revealed;
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
                tv.new_after = last_seen;
                if let Some(i) = open.select.and_then(|no| tv.index.get(&no).copied()) {
                    tv.selected = i;
                }
                // A conversation that was open last time.
                if let Some(&i) = open.conversation.and_then(|no| tv.index.get(&no)) {
                    let selected = tv.selected;
                    tv.selected = i;
                    if tv.enter_conversation().is_ok() && tv.in_view(selected) {
                        tv.select(selected);
                    }
                }
            }
        }
        self.tab.thread = Some(tv);
        self.tab.thread_site = self.tab.site;
        // Following: the first new entry that isn't hidden, revealed as `j` would.
        if let Some(last) = follow
            && let Some(t) = &mut self.tab.thread
            && let Some(at) = t.entry_of(&last)
            && let Some(next) = t.entries.iter().enumerate().skip(at + 1).find(|(_, e)| !t.is_collapsed(e.post)).map(|(i, _)| i)
        {
            t.set_cursor(next);
            t.reveal = Some(Reveal::Step);
        }
        if let Some(c @ ThreadCopy::Cached(_)) = copy {
            self.tab.copy = Some(c);
        }
    }

    /// A thread's posts arrived: note the visit, and keep a copy if it's watched.
    fn fetched_thread(&mut self, key: &ThreadKey, t: &Whole, shown_max: Option<u64>) {
        let posts = t.posts();
        // Just fetched: a watched thread's next background refresh counts from now.
        self.watched_checked.insert(key.clone(), self.clock.instant());
        let max_no = max_no(posts);
        let subject = thread_subject(posts);
        // A visit is an open, or a refresh that brought new posts.
        if shown_max.is_none_or(|m| max_no > m) {
            self.store.visit(key, &subject, posts.len(), max_no, self.clock.now());
            self.watched_quiet.remove(key);
        }
        self.store.opened(&key.site, &key.board, key.no, posts.len().saturating_sub(1) as u32, self.clock.now());
        // A watched thread's count (new posts or not: it's what the next answer is judged
        // against) and copy are kept.
        if let Some(w) = self.store.watched_mut(key) {
            w.posts = posts.len();
            generals::note_limit(w, posts);
            self.keep_copy(key, t);
        }
        self.save();
    }

    /// `every`, stretched for `quiet` refreshes in a row that brought nothing (see
    /// `QUIET_TIMES`).
    fn stretched(&self, every: Duration, quiet: u32) -> Duration {
        if !self.refresh_backoff {
            return every;
        }
        let cap = every.saturating_mul(QUIET_TIMES).min(QUIET_MAX).max(every);
        let mut d = every;
        for _ in 0..quiet {
            if d >= cap {
                break;
            }
            d = d.saturating_mul(3) / 2;
        }
        d.min(cap)
    }

    /// How long the open thread waits between refreshes now.
    pub(super) fn thread_every(&self) -> Duration {
        self.stretched(self.refresh_thread, self.tab.thread_quiet)
    }

    /// How long a watched thread waits between refreshes now.
    pub(super) fn watched_every(&self, key: &ThreadKey) -> Duration {
        self.stretched(self.refresh_watched, self.watched_quiet.get(key).copied().unwrap_or(0))
    }

    /// Start background refreshes that are due: the open thread every `refresh_thread`, and
    /// each watched thread every `refresh_watched` (longer while they're quiet), a couple at
    /// a time.
    pub(super) fn background(&mut self) {
        self.know_nsfw(self.tab.site);
        // A saved copy open isn't refreshed (a watched thread still is, below, unless it's dead).
        // Behind the settings it's refreshed as a watched thread is (counted, not visited).
        let open = self.tab.thread.as_ref().filter(|_| self.tab.view() == View::Thread && self.tab.saved().is_none()).map(|t| self.key(&t.board, t.no));
        let now = self.clock.instant();
        // Pages of a board none of whose watched threads has been refreshed for two of the
        // longest rounds (all unwatched, say) are too old to show.
        let old = self.refresh_watched.max(QUIET_MAX).saturating_mul(2);
        let asked = &self.pages_asked;
        self.board_pages.retain(|at, _| asked.get(at).is_some_and(|t| now.saturating_duration_since(*t) <= old));
        // Fetched for any tab (or as a watched thread) counts too.
        let fetched_since = |key: &ThreadKey, every: Duration| {
            self.watched_checked.get(key).is_none_or(|t| now.saturating_duration_since(*t) >= every)
        };
        let every = self.thread_every();
        if let Some(key) = &open
            && self.tab.loading().is_none()
            && now.saturating_duration_since(self.tab.thread_checked) >= every
            && fetched_since(key, every)
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
        let due = self.store.all_watched().iter().find(|w| {
            !w.status.is_dead()
                && Some(&w.key) != open.as_ref()
                && !self.refreshing.contains(&w.key)
                && self.watched_checked.get(&w.key).is_none_or(|t| now.saturating_duration_since(*t) >= self.watched_every(&w.key))
        });
        if let Some(key) = due.map(|w| w.key.clone()) {
            self.watched_checked.insert(key.clone(), self.clock.instant());
            self.refresh_in_background(key);
        }
        self.check_generals(self.clock.instant());
    }

    pub(super) fn refresh_in_background(&mut self, key: ThreadKey) {
        let Some(site) = self.site_named(&key.site) else { return };
        let backend = site.backend.clone();
        if self.store.watched(&key).is_some() {
            self.refresh_pages(&key.site, &key.board, backend.clone());
        }
        let later = self.later();
        self.refreshing.insert(key.clone());
        // The open thread's copy is kept up to date too (a watched one's is its saved copy).
        let pages = self.pages.clone().filter(|_| self.store.watched(&key).is_none());
        let (now, asked) = (self.clock.now(), self.clock.instant());
        std::thread::spawn(move || {
            let (res, copies) = http::recording(|| crate::guard::result(|| http::background_at(asked, || backend.thread(&key.board, key.no))));
            if let (Ok(_), Some(p)) = (&res, &pages) {
                p.write(&key.site, &key.board, Some(key.no), &copies, now);
            }
            later.run(move |app| app.refreshed(key, res));
        });
    }

    /// Ask where a watched thread's board has its threads (`thread_pages`), along with the
    /// thread's refresh: once a `refresh_watched` round for each board, however many of its
    /// threads are watched.
    fn refresh_pages(&mut self, site: &str, board: &str, backend: Arc<dyn Backend>) {
        let at = (site.to_string(), board.to_string());
        let now = self.clock.instant();
        if self.pages_asking.contains(&at) || self.pages_asked.get(&at).is_some_and(|t| now.saturating_duration_since(*t) < self.refresh_watched) {
            return;
        }
        self.pages_asked.insert(at.clone(), now);
        self.pages_asking.insert(at.clone());
        let later = self.later();
        std::thread::spawn(move || {
            let res = crate::guard::result(|| http::background_at(now, || backend.thread_pages(&at.1)));
            later.run(move |app| {
                app.pages_asking.remove(&at);
                match res {
                    Ok(Some(pages)) => {
                        app.board_pages.insert(at, pages);
                    }
                    // The engine can't tell, or the site doesn't: nothing to show. Other
                    // failures keep what was known, until the next round.
                    Ok(None) => {
                        app.board_pages.remove(&at);
                    }
                    Err(e) if http::is_not_found(&e) => {
                        app.board_pages.remove(&at);
                    }
                    Err(_) => {}
                }
            });
        });
    }

    /// The index page a thread is on, and how many pages its board has, as last asked.
    pub fn thread_page(&self, key: &ThreadKey) -> Option<(u32, u32)> {
        let pages = self.board_pages.get(&(key.site.clone(), key.board.clone()))?;
        pages.page.get(&key.no).map(|&p| (p, pages.of))
    }

    pub(super) fn refreshed(&mut self, key: ThreadKey, res: Result<Thread>) {
        self.refreshing.remove(&key);
        let is_open = self.tab.view() == View::Thread
            && self.tab.saved().is_none()
            && self.tab.thread.as_ref().is_some_and(|t| self.key(&t.board, t.no) == key)
            && self.current_site().cfg.name == key.site;
        if is_open {
            // Count the interval from the response, so the next refresh is past the HTTP cache window.
            self.tab.thread_checked = self.clock.instant();
        }
        match res {
            Ok(t) if is_open => {
                let newest = |app: &App| app.tab.thread.as_ref().map_or(0, |t| max_no(&t.live_posts()));
                let before = newest(self);
                self.show_thread(t, None, Opening::default());
                self.tab.thread_quiet = if newest(self) > before { 0 } else { self.tab.thread_quiet.saturating_add(1) };
            }
            Ok(t) => {
                // Cut short: counts, notes and the copy stay as they were.
                let Ok(t) = self.accept(&key, t, 0) else { return };
                let posts = t.posts();
                let subject = thread_subject(posts);
                let prev = self.notified_max.get(&key).copied();
                // Quiet: nothing past what the last refresh found (the first one of the
                // session starts over).
                let quiet = prev.is_some_and(|m| max_no(posts) <= m);
                let Some(w) = self.store.watched_mut(&key) else { return };
                let max_no = max_no(posts);
                if w.last_seen == 0 {
                    w.last_seen = max_no;
                }
                let last_seen = w.last_seen;
                // Hidden posts (by a filter, a hidden word, by hand, or as replies to those)
                // aren't new or replies to you: the thread passes them over when it's open.
                // What decides it is kept, to count them again when what's hidden changes.
                let fresh = with_ancestry(posts, |p| p.no > last_seen);
                let count = self.unread_counter(&key, &fresh);
                let (unread, replies) = count(last_seen);
                // Tell about posts newer than this session's last refresh (not on the first
                // one, which may find posts from long ago).
                let (new, new_replies) = prev.map_or((0, 0), |m| count(last_seen.max(m)));
                // Those a `notify` filter catches, hidden or not: asked for by name.
                let past = |p: &&Post| p.no > last_seen && prev.is_some_and(|m| p.no > m);
                let caught: Vec<String> = posts.iter().filter(past).filter_map(|p| self.hiding.filters().check(&key.site, &key.board, p, p.no == key.no).notify).collect();
                let Some(w) = self.store.watched_mut(&key) else { return };
                // Found: live, whatever it was.
                w.refreshed((unread, replies), fresh);
                let note = Note {
                    key: key.clone(),
                    subject: if w.subject.is_empty() { subject.clone() } else { w.subject.clone() },
                    new,
                    replies: new_replies,
                    caught: caught.len(),
                    filter: caught.into_iter().next().unwrap_or_default(),
                };
                w.posts = posts.len();
                generals::note_limit(w, posts);
                if w.subject.is_empty() {
                    w.subject = subject;
                }
                let n = if quiet { self.watched_quiet.get(&key).map_or(1, |n| n.saturating_add(1)) } else { 0 };
                self.watched_quiet.insert(key.clone(), n);
                self.keep_copy(&key, &t);
                self.notified_max.insert(key, max_no);
                if note.new > 0 || note.caught > 0 {
                    self.notes_since.get_or_insert_with(|| self.clock.instant());
                    self.notes.push(note);
                }
                self.save();
            }
            Err(e) if http::is_not_found(&e) => {
                if self.store.mark_dead(&key) {
                    self.save();
                }
                if is_open {
                    self.thread_gone(&key);
                }
            }
            // Other failures (network, rate limits) just wait for the next round.
            Err(e) if is_open => self.error(e),
            Err(_) => {}
        }
    }
}
