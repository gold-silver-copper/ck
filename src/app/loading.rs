//! Loading boards, catalogs and threads (for the tab, or in the background), showing what
//! arrives, and keeping open and watched threads refreshed.

use super::*;

impl App {
    /// Run `job` on a thread, as the current request (shown as `label`), then `apply` what
    /// it came to in the request's tab. The job gets the request id and a sender for partial
    /// results.
    pub(super) fn spawn<T: Send + 'static>(
        &mut self,
        label: String,
        job: impl FnOnce(&dyn Backend, u64, &Sender<Msg>) -> Result<T> + Send + 'static,
        apply: impl FnOnce(&mut App, Result<T>) + Send + 'static,
    ) {
        self.start(label, job, move |id, res| Msg::request(id, move |app| apply(app, res)));
    }

    /// `spawn`, with `wrap` making the message its result is sent back in.
    fn start<T: Send + 'static>(
        &mut self,
        label: String,
        job: impl FnOnce(&dyn Backend, u64, &Sender<Msg>) -> Result<T> + Send + 'static,
        wrap: impl FnOnce(u64, Result<T>) -> Msg + Send + 'static,
    ) {
        self.next_id += 1;
        let id = self.next_id;
        self.tab.req = Some(id);
        let backend = self.current_site().backend.clone();
        let tx = self.tx.clone();
        self.tab.loading = Some(label);
        self.tab.failed = None;
        self.status = None;
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
    pub(super) fn refresh_boards_in_background(&mut self, site: usize) {
        let now = self.clock.instant();
        if self.boards_tried.get(&site).is_some_and(|t| now.saturating_duration_since(*t) < http::MIN_REFETCH)
            || !self.boards_refreshing.insert(site)
        {
            return;
        }
        self.boards_tried.insert(site, now);
        let backend = self.sites[site].backend.clone();
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
        if self.status.is_none() {
            self.info(format!("Up to date (checked {}s ago)", age.as_secs()));
        }
    }

    /// A site's board list arrived for request `id`; it's shown even if the tab has moved on.
    pub(super) fn boards_arrived(&mut self, id: u64, site: usize, res: Result<Vec<Board>>) {
        let stale = self.tab.req != Some(id);
        if !stale {
            self.tab.loading = None;
        }
        match res {
            Ok(b) => self.set_boards(site, b, true),
            Err(e) if !stale => self.load_failed(http::plain(&e)),
            Err(e) => self.error(http::plain(&e)),
        }
    }

    /// The last copy kept of the catalog being fetched, fetched at `fetched`: shown only
    /// before anything fetched has arrived.
    fn cached_catalog_arrived(&mut self, posts: Vec<Post>, fetched: i64) {
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

    /// The catalog's pages loaded so far; more are coming.
    pub(super) fn catalog_partial(&mut self, posts: Vec<Post>) {
        self.tab.catalog_cached = None;
        self.show_catalog(posts);
    }

    pub(super) fn catalog_arrived(&mut self, res: Result<Vec<Post>>) {
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
            Err(e) if http::is_not_found(&e) => {
                let board = self.tab.board.as_ref().map_or_else(String::new, |b| b.uri.clone());
                self.load_failed(format!("There's no /{board}/ on {}", self.current_site().cfg.name));
            }
            Err(e) => self.load_failed(http::plain(&e)),
        }
    }

    /// The last copy kept of the thread being fetched, fetched at `fetched`: shown only
    /// before anything fetched (or saved) is.
    fn cached_thread_arrived(&mut self, posts: Vec<Post>, fetched: i64) {
        if self.tab.thread.is_none() && self.tab.view == View::Thread && self.tab.saved().is_none() {
            self.set_cached_thread(posts, fetched);
        }
    }

    pub(super) fn thread_arrived(&mut self, res: Result<Vec<Post>>) {
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
                let Some(key) = self.tab.board.as_ref().map(|b| self.key(&b.uri, self.tab.pending_thread.unwrap_or_default())) else {
                    return;
                };
                if let Some(w) = self.store.watched_mut(&key) {
                    w.dead = true;
                    self.save();
                }
                self.store.saved_dead(&key);
                self.thread_gone(&key);
            }
            Err(e) => self.load_failed(http::plain(&e)),
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
                    let _ = tx.send(Msg::request(id, move |app| app.catalog_partial(so_far)));
                })
            });
            if let (Ok(posts), Some(p)) = (&res, &pages)
                && !posts.is_empty()
            {
                p.write(&site, &board.uri, None, &copies, now);
            }
            res
        };
        self.spawn(label, job, App::catalog_arrived);
    }

    pub(super) fn load_thread(&mut self, no: u64) {
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
                    let fetched = *fetched;
                    let _ = tx.send(Msg::request(id, move |app| app.cached_thread_arrived(posts, fetched)));
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
        self.spawn(format!("Loading thread {no}"), job, App::thread_arrived);
    }

    /// Show catalog threads, keeping the selected thread selected (by number).
    pub(super) fn show_catalog(&mut self, posts: Vec<Post>) {
        let selected = self.selected_index().filter(|_| self.tab.view == View::Catalog).and_then(|i| self.tab.catalog.get(i)).map(|p| p.no);
        self.tab.catalog = posts;
        self.remark_catalog();
        if let Some(i) = selected.and_then(|no| self.visible_catalog().iter().position(|&k| self.tab.catalog[k].no == no)) {
            self.tab.catalog_list.state.select(Some(i));
        }
        let len = self.visible_catalog().len();
        self.tab.catalog_list.clamp(len);
    }

    /// Show a thread's posts as fetched (replacing a copy shown meanwhile).
    pub(super) fn set_thread(&mut self, posts: Vec<Post>) {
        self.show_thread(posts, None);
    }

    /// Show the last copy kept of a thread being fetched, fetched at `at`. It's not a visit.
    pub(super) fn set_cached_thread(&mut self, posts: Vec<Post>, at: i64) {
        self.show_thread(posts, Some(ThreadCopy::Cached(tabs::Offline { saved: at, dead: false })));
    }

    /// Show a thread's posts: fetched (`None`), or a copy.
    pub(super) fn show_thread(&mut self, posts: Vec<Post>, copy: Option<ThreadCopy>) {
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
                // Jumps back and revealed spoilers by post number, since indices can shift.
                let moved = |i: &usize| old.posts.get(*i).and_then(|p| tv.index.get(&p.no)).copied();
                tv.jumps = old.jumps.iter().filter_map(moved).collect();
                tv.new_after = old.new_after;
                tv.revealed = old.revealed.iter().filter_map(moved).collect();
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
        if copy.is_none() && self.tab.copy.is_none() {
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
        if let Some(c @ ThreadCopy::Cached(_)) = copy {
            self.tab.copy = Some(c);
        }
    }

    /// A thread's posts arrived: note the visit, and keep a copy if it's watched.
    fn fetched_thread(&mut self, key: &ThreadKey, posts: &[Post], shown_max: Option<u64>) {
        // Just fetched: a watched thread's next background refresh counts from now.
        self.watched_checked.insert(key.clone(), self.clock.instant());
        let max_no = max_no(posts);
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

    /// Start background refreshes that are due: the open thread every `refresh_thread`, and
    /// each watched thread every `refresh_watched`, a couple at a time.
    pub(super) fn background(&mut self) {
        self.know_nsfw(self.tab.site);
        // A saved copy open isn't refreshed (a watched thread still is, below, unless it's dead).
        let open = self.tab.thread.as_ref().filter(|_| self.tab.view == View::Thread && self.tab.saved().is_none()).map(|t| self.key(&t.board, t.no));
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
        let Some(site) = self.site_named(&key.site) else { return };
        let backend = site.backend.clone();
        let later = self.later();
        self.refreshing.insert(key.clone());
        // The open thread's copy is kept up to date too (a watched one's is its saved copy).
        let pages = self.pages.clone().filter(|_| self.store.watched(&key).is_none());
        let (now, asked) = (self.clock.now(), self.clock.instant());
        std::thread::spawn(move || {
            let (res, copies) = http::recording(|| crate::guard::result(|| http::background_at(asked, || backend.thread(&key.board, key.no))));
            if let (Ok(posts), Some(p)) = (&res, &pages)
                && !posts.is_empty()
            {
                p.write(&key.site, &key.board, Some(key.no), &copies, now);
            }
            later.run(move |app| app.refreshed(key, res));
        });
    }

    pub(super) fn refreshed(&mut self, key: ThreadKey, res: Result<Vec<Post>>) {
        self.refreshing.remove(&key);
        let is_open = self.tab.view == View::Thread
            && self.tab.saved().is_none()
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
                let max_no = max_no(&posts);
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
                    self.notes_since.get_or_insert(self.clock.instant());
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
}
