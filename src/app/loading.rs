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
        let Some(s) = self.sites.get(site) else { return };
        if complete && s.cfg.boards.is_none() {
            let name = s.cfg.name.clone();
            if let Err(e) = self.store.save_boards(&name, &boards, self.clock.now()) {
                self.error(format!("Couldn't save the board list: {e:#}"));
            }
        }
        if let Some(s) = self.sites.get_mut(site) {
            s.boards = Some(boards);
        }
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
            Err(e) if !stale => self.load_failed(&http::plain(&e)),
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
                && let Some(i) = self.row_of(View::Catalog, &RowKey::Thread(no))
            {
                self.tab.catalog_list.state.select(Some(i));
            }
        }
    }

    /// The catalog's pages loaded so far; more are coming. The selected thread, if it isn't
    /// among them yet, is selected once it comes (not the one the selection is moved to).
    pub(super) fn catalog_partial(&mut self, posts: Vec<Post>) {
        self.tab.catalog_cached = None;
        let keep = self.tab.pending_catalog.or_else(|| self.selected_catalog_no());
        self.show_catalog(posts);
        self.tab.pending_catalog = keep.filter(|&no| self.row_of(View::Catalog, &RowKey::Thread(no)).is_none());
    }

    pub(super) fn catalog_arrived(&mut self, res: Result<Vec<Post>>) {
        self.tab.loading = None;
        match res {
            Ok(posts) => {
                self.tab.catalog_cached = None;
                self.show_catalog(posts);
                self.catalog_seen();
                if let Some(no) = self.tab.pending_catalog.take()
                    && let Some(i) = self.row_of(View::Catalog, &RowKey::Thread(no))
                {
                    self.tab.catalog_list.state.select(Some(i));
                }
                let len = self.visible_catalog().len();
                self.tab.catalog_list.clamp(len);
            }
            Err(e) if http::is_not_found(&e) => {
                let board = self.tab.board.as_ref().map_or_else(String::new, |b| b.uri.clone());
                self.load_failed(&format!("There's no /{board}/ on {}", self.current_site().cfg.name));
            }
            Err(e) => self.load_failed(&http::plain(&e)),
        }
        // A thread to select that never came isn't waited for by the next load.
        self.tab.pending_catalog = None;
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
            Err(e) if http::is_not_found(&e) => {
                let key = self.tab.board.as_ref().map(|b| self.key(&b.uri, self.tab.pending_thread.unwrap_or_default()));
                if key.as_ref().is_some_and(|k| self.store.mark_dead(k)) {
                    self.save();
                }
                if restoring {
                    // Last session's thread is gone: its catalog instead.
                    self.tab.view = View::Catalog;
                    self.load_catalog();
                    self.info("The thread you had open last time is gone (archived or deleted)");
                } else if let Some(key) = key {
                    self.thread_gone(&key);
                }
            }
            Err(e) => self.load_failed(&http::plain(&e)),
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
        // Asked for: refreshes start over at the interval set.
        self.tab.thread_quiet = 0;
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

    /// The selected catalog thread's number, when the catalog is shown.
    fn selected_catalog_no(&self) -> Option<u64> {
        self.selected_index().filter(|_| self.tab.view == View::Catalog).and_then(|i| self.tab.catalog.get(i)).map(|p| p.no)
    }

    /// Show catalog threads, keeping the selected thread selected (by number), or the one
    /// waiting to be.
    pub(super) fn show_catalog(&mut self, posts: Vec<Post>) {
        let selected = self.tab.pending_catalog.or_else(|| self.selected_catalog_no());
        let site = self.sites.get(self.tab.catalog_site).map_or(String::new(), |s| s.cfg.name.clone());
        self.tab.catalog_marks = self.catalog_marks_for(&site, &posts, |p| self.board_of(p));
        self.tab.catalog = posts;
        if let Some(i) = selected.and_then(|no| self.row_of(View::Catalog, &RowKey::Thread(no))) {
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
        let old = self.tab.thread.take().filter(|t| t.no == no && t.board == board);
        // Fetched again: posts shown before that it leaves out were deleted, and stay.
        let (posts, deleted, shrank) = match &old {
            Some(old) if copy.is_none() && !replacing => thread_view::keep_deleted(old, posts),
            _ => (posts, HashSet::new(), false),
        };
        let mut tv = ThreadView::new(board, no, posts);
        tv.marks = self.thread_marks(&key.site, &tv);
        tv.margin = self.scroll_margin;
        tv.deleted = deleted;
        tv.shrinks = old.as_ref().map_or(0, |o| o.shrinks) + u32::from(shrank);
        // On a refresh, the newest post already shown.
        let mut shown_max = None;
        // Reading the end: the last entry shown, to go on from once the posts are marked.
        let mut follow = None;
        match old {
            // On refresh, keep the selected post and what's at the top of the view.
            Some(old) => {
                shown_max = old.posts.iter().filter(|p| !old.deleted.contains(&p.no)).map(|p| p.no).max().filter(|_| !replacing);
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
            self.fetched_thread(&key, &tv.live_posts(), shown_max);
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
    fn fetched_thread(&mut self, key: &ThreadKey, posts: &[Post], shown_max: Option<u64>) {
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
        if let Some(w) = self.store.watched_mut(key) {
            generals::note_limit(w, posts);
        }
        self.keep_copy(key, posts, false);
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
        let open = self.tab.thread.as_ref().filter(|_| self.tab.view == View::Thread && self.tab.saved().is_none()).map(|t| self.key(&t.board, t.no));
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
            && self.tab.loading.is_none()
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
            if let (Ok(posts), Some(p)) = (&res, &pages)
                && !posts.is_empty()
            {
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
            Ok(posts) if is_open => {
                let newest = |app: &App| app.tab.thread.as_ref().map_or(0, |t| max_no(&t.live_posts()));
                let before = newest(self);
                self.set_thread(posts);
                self.tab.thread_quiet = if newest(self) > before { 0 } else { self.tab.thread_quiet.saturating_add(1) };
            }
            Ok(posts) => {
                let subject = thread_subject(&posts);
                let prev = self.notified_max.get(&key).copied();
                // Quiet: nothing past what the last refresh found (the first one of the
                // session starts over).
                let quiet = prev.is_some_and(|m| max_no(&posts) <= m);
                let Some(w) = self.store.watched_mut(&key) else { return };
                let max_no = max_no(&posts);
                if w.last_seen == 0 {
                    w.last_seen = max_no;
                }
                let last_seen = w.last_seen;
                // Hidden posts (by a filter, a hidden word, by hand, or as replies to those)
                // aren't new or replies to you: the thread passes them over when it's open.
                // What decides it is kept, to count them again when what's hidden changes.
                let fresh = with_ancestry(&posts, |p| p.no > last_seen);
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
                generals::note_limit(w, &posts);
                if w.subject.is_empty() {
                    w.subject = subject;
                }
                let n = if quiet { self.watched_quiet.get(&key).map_or(1, |n| n.saturating_add(1)) } else { 0 };
                self.watched_quiet.insert(key.clone(), n);
                self.keep_copy(&key, &posts, false);
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
